//! `Sim::step` plus its tick-time helpers (`process_kills`,
//! `spawn_boss`). Split out of `sim/mod.rs`. Pure `impl Sim`
//! block — every method is defined on `Sim` and migrated
//! here verbatim. Glob-imports `super::*` because `step()` is
//! the simulation's central pump and touches nearly every
//! sibling submodule.

#![allow(unused_imports)]
use super::*;

impl Sim {
    /// Advance the simulation by one fixed timestep. `tick` is the
    /// server's current monotonic tick counter — channel ticks
    /// stamp it into their `WorldEvent::ChannelTick` so clients can
    /// interpolate against snapshot timing.
    pub fn step(&mut self, dt: f32, tick: NetTick) {
        // Cache the live tick so non-step paths (loot drop /
        // pickup, persistence hooks) can reason about
        // tick-relative deadlines without threading `tick`
        // through every call site.
        self.current_tick = tick;
        // Bump the meter clock first so the elapsed reading
        // sent in this tick's broadcast covers everything that
        // happens below.
        self.meters.elapsed += dt;
        // 1. Players: ingest inputs, integrate motion.
        player::apply_inputs(&mut self.world, &self.sessions, &mut self.pending_inputs);
        player::integrate_motion(&mut self.world, &self.floor, dt);

        // 2. Enemies: AI tick (queues melee damage + ranged
        //    shot requests), then integrate motion and spawn
        //    any caster bolts the AI asked for.
        let player_targets = player::target_positions(&self.world);
        let damage_mult = FloorConfig::for_floor(self.floor_index).enemy_damage_mult;
        let ai_outcome = enemies::tick_ai(
            &mut self.world,
            &self.floor,
            &player_targets,
            damage_mult,
            dt,
        );
        let melee_damage = ai_outcome.melee_damage;
        // Pipe through any wire events the AI tick produced
        // (currently telegraph SFX cues). Done here so they
        // ride out on the same frame's snapshot, before the
        // ability dispatch below pushes its own events.
        self.pending_events.extend(ai_outcome.events);
        // Apply vampiric heal-back from elite mods. Clamped to
        // hp_max and accompanied by a Heal event so clients see
        // floating-green numbers off the affected enemy. No
        // sound / VFX wiring yet — the event alone is enough.
        for (entity, amount) in ai_outcome.vampiric_heals {
            if amount <= 0.0 {
                continue;
            }
            if let Ok(mut en) = self.world.get::<&mut enemies::ServerEnemy>(entity) {
                if en.is_dying() {
                    continue;
                }
                let healed = (en.hp + amount).min(en.hp_max) - en.hp;
                if healed > 0.0 {
                    en.hp += healed;
                    let pos = en.k.position;
                    let nid = en.net_id;
                    drop(en);
                    self.pending_events
                        .push(rift_net::messages::WorldEvent::Heal {
                            caster: nid,
                            target: nid,
                            amount: healed,
                            over_time: false,
                            position: pos.to_array(),
                        });
                }
            }
        }
        // Unified enemy ability cast pipeline. Every enemy
        // attack flows through this single stream:
        //   * `Start` events translate into `AbilityCast` wire
        //     events so clients can play the telegraph.
        //   * `Resolve` events run authoritative effects by
        //     building a [`ability::CombatIntent::Ai`] and
        //     pushing it through `submit` + `dispatch`.
        //     Summons go through a local queue so net-id
        //     allocation stays owned by Sim.
        let mut summon_queue: Vec<(glam::Vec3, rift_game::monsters::MonsterRole, f32)> = Vec::new();
        let mut melee_from_resolves: Vec<combat_ctx::PlayerHit> = Vec::new();
        // Stand-in tables for the AI submit gate — AI casts
        // don't read sessions / cooldowns but the kernel
        // signature is uniform.
        let ai_sessions: HashMap<ClientId, Entity> = HashMap::new();
        let mut ai_cooldowns: ability::CooldownTable = HashMap::new();
        for cast in ai_outcome.casts {
            match cast {
                enemies::EnemyCast::Start {
                    owner,
                    ability_id,
                    origin,
                    target,
                    dir_x,
                    dir_y,
                } => {
                    self.pending_events
                        .push(rift_net::messages::WorldEvent::AbilityCast {
                            caster: owner,
                            ability: ability_id.raw() as u16,
                            origin: origin.to_array(),
                            dir: [dir_x, dir_y],
                            target: Some(target.to_array()),
                            start_tick: tick,
                        });
                }
                enemies::EnemyCast::Resolve {
                    owner,
                    attacker_kind,
                    ability_id,
                    origin,
                    aim,
                    damage_mult,
                    crit_chance,
                    crit_damage,
                    param_a,
                } => {
                    let intent = ability::CombatIntent::Ai {
                        caster: owner,
                        attacker_kind,
                        ability_id,
                        origin,
                        aim,
                        damage_mult,
                        crit_chance,
                        crit_damage,
                        param_a,
                    };
                    let Some(accepted) = ability::submit(
                        &self.world,
                        &ai_sessions,
                        &mut ai_cooldowns,
                        &self.floor,
                        intent,
                    ) else {
                        continue;
                    };
                    // Persistent AoE zones queued by enemy
                    // casts go into the same `self.aoe_zones`
                    // pool as player-cast zones; the unified
                    // `tick_aoe` branches on `team` to pick
                    // its target list.
                    let mut player_heals_unused: Vec<(Entity, f32)> = Vec::new();
                    let mut sinks = ability::DispatchSinks {
                        aoe_zones: &mut self.aoe_zones,
                        events: &mut self.pending_events,
                        next_projectile_net_id: &mut self.next_projectile_net_id,
                        player_damage: &mut melee_from_resolves,
                        player_heals: &mut player_heals_unused,
                        summons: &mut summon_queue,
                        player_targets: &player_targets,
                        melee_swings: &mut self.pending_melee_swings,
                    };
                    ability::dispatch(&mut self.world, accepted, &mut sinks, tick);
                    debug_assert!(
                        player_heals_unused.is_empty(),
                        "enemy cast emitted player-heal rows",
                    );
                }
            }
        }
        // Drain any summons queued during cast resolves into
        // real enemy entities. Net-ids come from the same
        // allocator the floor packs use so clients see them as
        // ordinary enemies.
        for (pos, role, hp_mult) in &summon_queue {
            enemies::spawn_summon(
                &mut self.world,
                *pos,
                *role,
                *hp_mult,
                self.floor_index,
                &mut self.next_enemy_net_id,
            );
        }
        // Merge the two damage queues (AI melee + cast
        // resolves) into one for the player-damage pass.
        let mut melee_damage = melee_damage;
        melee_damage.extend(melee_from_resolves);
        enemies::integrate_motion(&mut self.world, &self.floor, dt);

        // 3. Apply queued enemy → player melee damage. Players
        //    crossing 0 hp emit a `Death` event and queue a
        //    `(client_id, net_id)` entry for the main loop. The
        //    first death on a non-hub floor also arms the
        //    auto-respawn timer.
        //
        //    `proc_cast_queue` collects free-cast requests from
        //    OnDodge / OnLowHealth / OnHit `ProcAction::CastAbility`
        //    procs (Mirrorglass Amulet pool). It's appended to
        //    by `apply_player_damage` and by the projectile-hit
        //    loop (via `CombatCtx::proc_casts`); drained at the
        //    end of the step so the cast pipeline runs against
        //    a clean world borrow.
        let mut proc_cast_queue: Vec<(Entity, super::procs::ProcCastRequest)> = Vec::new();
        damage::apply_player_damage(
            &mut self.world,
            &mut self.pending_events,
            &mut self.pending_player_deaths,
            &mut self.meters,
            &mut self.aoe_zones,
            &mut self.next_projectile_net_id,
            tick,
            melee_damage,
            &mut proc_cast_queue,
        );
        self.check_party_wipe();

        // 4. Tick ability cooldowns.
        ability::tick_cooldowns(&mut self.cooldowns, dt);

        // 4b. Tick essence regen for every player. Runs after
        //     channels / casts have already drained for the
        //     frame so the post-spend pause is honoured before
        //     any regen happens.
        for (_e, p) in self.world.query_mut::<&mut player::ServerPlayer>() {
            p.tick_resource(dt);
            p.tick_health_regen(dt);
        }

        // 5. Snapshot enemies for collision queries, then run
        //    projectiles + AoE zones + channels against them.
        //    All damage paths share one `CombatCtx` so DoT and
        //    direct kills both run through `loot::finalise_kills`
        //    (which emits `Death`, rolls drops, and despawns).
        let enemies = enemies::snapshot_for_collision(&self.world);

        let mut kills: Vec<combat_ctx::KillInfo> = Vec::new();
        // Sinks for the elite-affix flow. `thorns_back` is
        // drained into `enemy_player_damage` after the CombatCtx
        // scope so reflected damage runs through the same
        // `apply_player_damage` chokepoint as enemy melee /
        // bolt damage. `death_aoe_zones` are appended to
        // `self.aoe_zones` so the next tick of `tick_aoe`
        // picks them up.
        let mut thorns_back: Vec<combat_ctx::PlayerHit> = Vec::new();
        let mut death_aoe_zones: Vec<projectile::ServerAoeZone> = Vec::new();
        let mut meter_events: Vec<combat_ctx::MeterEvent> = Vec::new();
        let mut ctx = combat_ctx::CombatCtx {
            events: &mut self.pending_events,
            next_loot_net_id: &mut self.next_loot_net_id,
            tick,
            floor_index: self.floor_index,
            kills: &mut kills,
            player_damage_back: &mut thorns_back,
            meter_events: &mut meter_events,
            death_aoe_zones: &mut death_aoe_zones,
            next_projectile_net_id: &mut self.next_projectile_net_id,
            proc_casts: &mut proc_cast_queue,
            share_window_ticks: SHARE_WINDOW_TICKS,
        };
        // Resolve queued player melee swings against the live
        // enemy snapshot. Each swing fires `apply_hits_to_enemies`
        // with the same hit pipeline projectiles / channels
        // use (aggro, on-hit procs, kills, loot). Drained before
        // `projectile::tick` so the swing damage / death events
        // appear ahead of any projectile hits this tick — feels
        // right for the LMB swing being the most immediate
        // input → outcome chain.
        if !self.pending_melee_swings.is_empty() {
            let mut swing_hits: Vec<projectile::Hit> = Vec::new();
            for swing in self.pending_melee_swings.drain(..) {
                let r2 = swing.radius * swing.radius;
                let aim_xz = glam::Vec2::new(swing.aim.x, swing.aim.z);
                let aim_len2 = aim_xz.length_squared();
                if aim_len2 < 1.0e-6 {
                    continue;
                }
                let aim_n = aim_xz / aim_len2.sqrt();
                let half_arc_cos = (swing.arc_radians * 0.5).cos();
                for (en_entity, en_pos, en_net_id, _en_radius) in &enemies {
                    let dx = en_pos.x - swing.origin.x;
                    let dz = en_pos.z - swing.origin.z;
                    let d2 = dx * dx + dz * dz;
                    if d2 > r2 || d2 < 1.0e-6 {
                        // Out of range, or the caster's own
                        // tile — the latter shouldn't happen
                        // for enemies vs players but guards
                        // against a divide-by-zero on the
                        // bearing check below.
                        continue;
                    }
                    let inv = 1.0 / d2.sqrt();
                    let dot = (dx * aim_n.x + dz * aim_n.y) * inv;
                    if dot < half_arc_cos {
                        continue;
                    }
                    swing_hits.push(projectile::Hit {
                        enemy: *en_entity,
                        enemy_net_id: *en_net_id,
                        enemy_pos: *en_pos,
                        attacker: swing.caster_net_id,
                        ability_id: swing.ability_id,
                        damage: swing.damage,
                        crit_chance: swing.crit_chance,
                        crit_damage: swing.crit_damage,
                        crit_seed: projectile::hit_seed(
                            ctx.tick,
                            *en_net_id,
                            swing.caster_net_id,
                            swing.ability_id.raw() as u64,
                        ),
                        apply_debuff: None,
                        // Swing impulse points from caster
                        // outward along the aim direction so
                        // the blood splash kicks away from
                        // the swinger.
                        hit_dir: glam::Vec3::new(aim_n.x, 0.0, aim_n.y),
                    });
                }
            }
            if !swing_hits.is_empty() {
                projectile::apply_hits_to_enemies(
                    &mut self.world,
                    &self.floor,
                    swing_hits,
                    &mut ctx,
                );
            }
        }
        // Unified projectile tick — handles both player→enemy
        // and enemy→player bolts (distinguished by `Team`).
        // Enemy-team hits are returned as `(player, damage)`
        // rows for the player-damage path below; the player-
        // team path runs through `CombatCtx` like before, so
        // event ordering stays consistent.
        let enemy_proj_damage = projectile::tick(
            &mut self.world,
            &self.floor,
            &enemies,
            &player_targets,
            &mut ctx,
            dt,
        );
        let enemy_aoe_damage = projectile::tick_aoe(
            &mut self.world,
            &self.floor,
            &mut self.aoe_zones,
            &enemies,
            &player_targets,
            &mut ctx,
            dt,
        );
        let enemy_channel_damage = channel::tick(
            &mut self.world,
            &self.floor,
            &enemies,
            &player_targets,
            &mut ctx,
            tick,
            dt,
        );

        // 6. Tick debuff stacks: decay durations, fire DoT damage,
        //    drop expired entries. Runs last so DoT events ride
        //    out on this frame's snapshot. Player DoT damage is
        //    returned and merged into the enemy-side damage
        //    pile so it goes through the single
        //    `apply_player_damage` route below.
        let player_dot_damage = effect::tick(&mut self.world, &mut ctx, dt);

        // Apply the enemy-projectile damage collected before the
        // `CombatCtx` scope. Done here, after `ctx` is dropped, so
        // the player-damage path can borrow `pending_events` /
        // `pending_player_deaths` without aliasing. Enemy-team
        // AoE rows + player DoT rows are merged in so the same
        // event-ordering rules apply uniformly.
        let mut enemy_player_damage = enemy_proj_damage;
        enemy_player_damage.extend(enemy_aoe_damage);
        enemy_player_damage.extend(enemy_channel_damage);
        enemy_player_damage.extend(player_dot_damage);
        // Merge thorns reflection back to the attacker into the
        // same player-damage queue. Done after `ctx` drops so
        // the borrows on `pending_events` clear.
        enemy_player_damage.extend(thorns_back);
        // Push EXPLODER death zones into the active zone pool;
        // next tick's `tick_aoe` will run them against players.
        self.aoe_zones.extend(death_aoe_zones);
        if !enemy_player_damage.is_empty() {
            damage::apply_player_damage(
                &mut self.world,
                &mut self.pending_events,
                &mut self.pending_player_deaths,
                &mut self.meters,
                &mut self.aoe_zones,
                &mut self.next_projectile_net_id,
                tick,
                enemy_player_damage,
                &mut proc_cast_queue,
            );
            self.check_party_wipe();
        }

        // Drain proc-driven free casts (Mirrorglass Amulet pool +
        // future OnHit / OnDodge / OnLowHealth `CastAbility`
        // procs). Each request was queued earlier in the step
        // with a borrow context that didn't own a cast
        // dispatcher; dispatching here means the world borrow is
        // clean and the resulting effects (projectiles / zones /
        // channels) are added to the same pools the manual cast
        // path writes to, replicated to clients through the same
        // `WorldEvent` traffic, and credited to the proc owner's
        // meter row.
        if !proc_cast_queue.is_empty() {
            let mut proc_summons: Vec<(Vec3, rift_game::monsters::MonsterRole, f32)> = Vec::new();
            let mut proc_player_damage: Vec<combat_ctx::PlayerHit> = Vec::new();
            let mut proc_player_heals: Vec<(Entity, f32)> = Vec::new();
            for (caster, request) in proc_cast_queue.drain(..) {
                let mut sinks = ability::DispatchSinks {
                    aoe_zones: &mut self.aoe_zones,
                    events: &mut self.pending_events,
                    next_projectile_net_id: &mut self.next_projectile_net_id,
                    player_damage: &mut proc_player_damage,
                    player_heals: &mut proc_player_heals,
                    summons: &mut proc_summons,
                    player_targets: &player_targets,
                    melee_swings: &mut self.pending_melee_swings,
                };
                ability::dispatch_proc_cast(&mut self.world, caster, request, &mut sinks, tick);
            }
            // Apply any player-damage rows produced by proc
            // casts that landed on `AbilityKind::DelayedAoe`
            // shapes (unlikely with today's Mirrorglass pool
            // but cheap to thread). Skip the proc_cast feedback
            // loop — we don't want a proc-cast's hit to spawn
            // another proc-cast this same frame.
            let mut _proc_proc_dummy: Vec<(Entity, super::procs::ProcCastRequest)> = Vec::new();
            if !proc_player_damage.is_empty() {
                damage::apply_player_damage(
                    &mut self.world,
                    &mut self.pending_events,
                    &mut self.pending_player_deaths,
                    &mut self.meters,
                    &mut self.aoe_zones,
                    &mut self.next_projectile_net_id,
                    tick,
                    proc_player_damage,
                    &mut _proc_proc_dummy,
                );
            }
            // Mirrorglass and other current proc-cast pools
            // are damage-only; defer routing of heal /
            // summon sinks until we add a proc that needs
            // them. Asserting empty keeps the contract
            // explicit instead of silently dropping rows.
            debug_assert!(
                proc_player_heals.is_empty(),
                "proc cast queued player heals; healing pipeline not yet routed",
            );
            debug_assert!(
                proc_summons.is_empty(),
                "proc cast queued summons; not yet routed",
            );
        }

        // Fold meter events queued during the CombatCtx scope
        // (player → enemy hits) into the per-instance meters.
        // Done after `ctx` drops so we can re-borrow the world
        // immutably to resolve attacker entity → ClientId.
        for ev in meter_events.drain(..) {
            match ev {
                combat_ctx::MeterEvent::DamageDealt {
                    attacker,
                    ability_id,
                    amount,
                } => {
                    let cid = self
                        .world
                        .get::<&player::ServerPlayer>(attacker)
                        .ok()
                        .map(|p| p.client_id);
                    if let Some(cid) = cid {
                        self.meters.entry(cid).add_damage(ability_id, amount);
                    }
                }
                combat_ctx::MeterEvent::HealingDone {
                    caster,
                    ability_id,
                    amount,
                } => {
                    // HoT ticks land here. Direct heals are
                    // credited inline at the cast site and
                    // never enter the meter_events queue.
                    let cid = self
                        .world
                        .get::<&player::ServerPlayer>(caster)
                        .ok()
                        .map(|p| p.client_id);
                    if let Some(cid) = cid {
                        self.meters.entry(cid).add_healing(ability_id, amount);
                    }
                }
            }
        }

        // 6b. Tick revive shrines after the CombatCtx scope ends so
        //     the borrow on `pending_events` is free. `shrine::tick`
        //     pushes `WorldEvent::PlayersRevived` directly into the
        //     event queue, so the broadcast picks it up this tick.
        shrine::tick(&mut self.world, &mut self.pending_events, dt);

        // 7. Death-fade: tick the death timer on dying enemies and
        //    despawn rows whose timer hit zero. Kept separate from
        //    the kill path so the corpse stays in snapshots long
        //    enough for the client to play its `Death` clip.
        enemies::tick_dying(&mut self.world, dt);

        // 8. Award XP + bump rift progress for every kill this
        //    tick. Boss kills end the floor; non-boss kills push
        //    the progress bar and may trigger the boss spawn.
        if !kills.is_empty() {
            self.process_kills(&kills);
        }

        // 9. Wipe-respawn countdown. `check_party_wipe` arms
        //    this only when every player on a non-hub floor is
        //    dead; the main loop reads it via
        //    [`Self::take_hub_respawn_request`] when it expires
        //    and force-loads everyone back to the hub.
        if let Some(t) = self.hub_respawn_timer.as_mut() {
            *t -= dt;
        }

        // 10. Per-player ghost-rise countdown. Each dead player
        //     ticks their own timer; when it hits 0 they flip
        //     `is_ghost = true` which (a) lets `apply_inputs`
        //     accept movement next tick and (b) makes the
        //     snapshot pipeline drop their row from every other
        //     viewer's outbound snapshot. We also emit a
        //     `PlayerGhosted` event so remote clients can play
        //     a poof VFX at the body's last position instead of
        //     watching the avatar pop out of existence.
        let mut risen: Vec<(NetId, [f32; 3])> = Vec::new();
        for (_e, p) in self.world.query_mut::<&mut player::ServerPlayer>() {
            // Tick legendary-transform internal cooldowns
            // (e.g. `FrostRayShatter`). Cheap fixed-size pass.
            p.transform_cds.tick(dt);
            if let Some(t) = p.ghost_rise_timer.as_mut() {
                *t -= dt;
                if *t <= 0.0 {
                    p.ghost_rise_timer = None;
                    p.is_ghost = true;
                    risen.push((p.net_id, p.k.position.to_array()));
                }
            }
        }
        for (entity, position) in risen {
            self.pending_events
                .push(WorldEvent::PlayerGhosted { entity, position });
        }
    }

    /// Resolve every kill produced by the damage subsystems this
    /// tick. Walks the list once: bumps rift progress for normal
    /// kills, flips `floor_complete` when the boss dies, spawns
    /// the boss when progress hits required, and grants XP to
    /// every connected player. Sets `progress_dirty` whenever the
    /// rift state changes so the main loop broadcasts a fresh
    /// `RiftProgress` next iteration.
    fn process_kills(&mut self, kills: &[combat_ctx::KillInfo]) {
        let mut spawn_boss_now = false;
        for k in kills {
            if k.role == rift_game::monsters::MonsterRole::Boss {
                if !self.rift_progress.boss_killed {
                    self.rift_progress.boss_killed = true;
                    self.rift_progress.floor_complete = true;
                    self.progress_dirty = true;
                    log::info!(
                        "sim: floor {} boss killed — floor complete",
                        self.floor_index
                    );
                }
            } else if !self.rift_progress.boss_spawned {
                if self.rift_progress.required > 0 {
                    let next = (self.rift_progress.progress + 1).min(self.rift_progress.required);
                    if next != self.rift_progress.progress {
                        self.rift_progress.progress = next;
                        self.progress_dirty = true;
                    }
                    if self.rift_progress.progress >= self.rift_progress.required {
                        spawn_boss_now = true;
                    }
                }
            }
        }

        // Grant XP to every connected player. Use their current
        // level for the kill-XP scaling so over-levelled players
        // get diminished returns.
        let monster_level = (self.floor_index as u32).max(1);
        let player_entities: Vec<(ClientId, Entity)> =
            self.sessions.iter().map(|(c, e)| (*c, *e)).collect();
        for (cid, entity) in player_entities {
            let Ok(mut p) = self.world.get::<&mut ServerPlayer>(entity) else {
                continue;
            };
            // Skip dead players, ghosts, and players in the
            // down-pose waiting to rise. Awarding XP here would
            // also trigger a level-up heal that resurrects a
            // player who died on the same tick.
            if p.is_dead_or_ghosting() {
                continue;
            }
            let mut total = 0u64;
            for _ in kills
                .iter()
                .filter(|k| k.role != rift_game::monsters::MonsterRole::Boss)
            {
                total += rift_game::experience::Experience::xp_for_kill(
                    monster_level,
                    p.experience.level,
                );
            }
            // Boss kills are worth a fat lump of XP \u2014 5\u00d7 a
            // normal kill at the floor's monster level.
            for _ in kills
                .iter()
                .filter(|k| k.role == rift_game::monsters::MonsterRole::Boss)
            {
                total += rift_game::experience::Experience::xp_for_kill(
                    monster_level,
                    p.experience.level,
                ) * 5;
            }
            if total == 0 {
                continue;
            }
            let rewards = p.grant_xp(total);
            self.pending_stat_updates.push(StatsUpdate {
                client_id: cid,
                level: p.experience.level,
                xp: p.experience.current_xp,
                xp_to_next: p.experience.xp_to_next_level(),
                total_xp: p.experience.total_xp,
                levelled_up: !rewards.is_empty(),
            });
        }

        if spawn_boss_now {
            self.spawn_boss();
        }
    }

    /// Spawn the floor's boss in the BSP-derived `boss_room_center`.
    /// Higher HP, slower speed, role = `MonsterRole::Boss`.
    /// Idempotent against `rift_progress.boss_spawned`.
    fn spawn_boss(&mut self) {
        if self.rift_progress.boss_spawned {
            return;
        }
        let boss_pos = Vec3::new(
            self.floor.boss_room_center.x,
            0.0,
            self.floor.boss_room_center.z,
        );
        let cfg = FloorConfig::for_floor(self.floor_index);
        let hp = cfg.enemy_health * 8.0 + self.floor_index as f32 * 30.0;
        let speed = cfg.enemy_speed * 0.7;
        let net_id = NetId(self.next_enemy_net_id);
        self.next_enemy_net_id = self.next_enemy_net_id.wrapping_add(1).max(1);
        let enemy = enemies::ServerEnemy {
            net_id,
            role: rift_game::monsters::MonsterRole::Boss,
            k: rift_game::kinematic::Kinematic {
                position: boss_pos,
                velocity: Vec3::ZERO,
                yaw: 0.0,
                aim_yaw: 0.0,
                locomotion: rift_game::kinematic::loco::IDLE,
                vy: 0.0,
                airborne: false,
                ..Default::default()
            },
            target_lock: None,
            speed,
            hp_max: hp,
            hp,
            attack_cooldown: 0.0,
            attack_anim_remaining: 0.0,
            dying_remaining: 0.0,
            ai_phase: enemies::AiPhase::default(),
            crit_chance: 0.0,
            crit_damage: 0.0,
            stagger_remaining: 0.0,
            knockback_remaining: 0.0,
            knockback_velocity: glam::Vec3::ZERO,
            pending_aggro: None,
            threat: std::collections::HashMap::new(),
            elite_mods: 0,
            flank_slot: 0,
            path: Vec::new(),
            path_target_tile: None,
            path_recompute_in: 0.0,
            los_blocked_cached: false,
            los_recheck_in: 0.0,
        };
        self.world.spawn((
            enemy,
            effect::EffectStack::default(),
            enemies::BossState::new(self.floor_index),
        ));
        self.rift_progress.boss_spawned = true;
        self.progress_dirty = true;
        log::info!(
            "sim: floor {} boss spawned at {:?} (hp={hp:.0})",
            self.floor_index,
            boss_pos
        );
    }
}
