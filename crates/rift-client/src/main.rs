use std::{net::SocketAddr, time::Duration};

use glam::Vec3;
use rift_client::game::{EquipRequest, GameState, NetTransitionRequest, StashRequest};
use rift_client::net_client::{ClientProfile, NetClient};
use rift_engine::{App, Input, LoadStatus, Renderer, Window};
struct RiftApp {
    state: GameState,
    /// Optional networking session. `Some` only when the binary was
    /// launched with `--connect`. The rest of `update` runs exactly
    /// the same in both modes — Phase 2 just does the handshake and
    /// log; Phase 3 will start steering the world from snapshots.
    net: Option<NetClient>,
}

impl App for RiftApp {
    fn load_step(&mut self, renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        self.state.load_step(renderer)
    }

    fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // The frame is split into named phases so the dispatch
        // table stays readable as we add features. Order matters:
        //
        //   1. `net_pre_phase` drives the network layer and applies
        //      any server-pushed floor transition before the SP
        //      update sees it.
        //   2. `sync_entities_from_snapshot` writes the latest
        //      snapshot into the ECS so animation / camera /
        //      render-sync (run inside `state.update`) observe
        //      authoritative transforms this frame.
        //   3. `handle_world_events` drains reliable events and
        //      spawns combat-text, blood decals, AoE visuals, etc.
        //   4. `state.update` runs the regular SP frame.
        //   5. `forward_client_commands` ships the user's
        //      intentions (input, casts, pickups, equip swaps).
        //   6. `apply_server_pushed_state` integrates server
        //      replies that are safe to apply after the SP frame
        //      (inventory / equipment / XP / progress / claims).
        if self.net.is_some() {
            self.net_pre_phase(renderer, input, dt);
            // Skip ECS sync while the staged net-transition is
            // running — the world is being rebuilt and any
            // remote-avatar / enemy / loot spawn from the next
            // snapshot would land in a half-built scene.
            if !self.state.is_net_transitioning() {
                self.sync_entities_from_snapshot(renderer, dt);
                self.handle_world_events(renderer);
            }
        }

        self.state.update(renderer, input, dt);

        if self.net.is_some() {
            if !self.state.is_net_transitioning() {
                self.forward_client_commands();
                self.apply_server_pushed_state(renderer);
            }
        }

        // Audio housekeeping: refresh the listener pose from
        // the player's camera, then tick the mixer so finished
        // one-shots are reaped and dead emitters recycle their
        // slots. Done after every gameplay system has had a
        // chance to spawn / move emitters this frame, so the
        // listener pose used for spatialisation matches what
        // the player sees on the rendered frame.
        if let Some(audio) = self.state.audio.as_mut() {
            // Listener at the camera position, oriented along
            // the look vector. Third-person camera puts the
            // ear roughly where the eye is, which keeps stereo
            // panning consistent with screen-space \u2014 a sound
            // on the right of the screen pans right.
            let cam = &renderer.camera;
            let forward = (cam.target - cam.position).normalize_or_zero();
            let orientation = if forward.length_squared() > 1e-6 {
                glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, forward)
            } else {
                glam::Quat::IDENTITY
            };
            audio.set_listener(cam.position, orientation);
            // Push every entity-bound emitter's transform +
            // intensity into the mixer before the tick. Cheap
            // single-query walk; see
            // `rift_client::game::audio::tick_audio_emitters`.
            rift_client::game::audio::tick_audio_emitters(&self.state.world, audio);
            audio.tick();
        }
    }

    fn shutdown(&mut self, renderer: &mut Renderer) {
        self.state.shutdown(renderer);
    }
}

/// Per-phase helpers for [`RiftApp::update`]. Pulled out so each
/// phase's responsibility is named and individually skimmable.
/// Every helper short-circuits when `self.net` is `None`, so SP /
/// offline mode pays nothing for the split.
impl RiftApp {
    /// Drive the network layer and apply any server-driven floor
    /// transition. Has to run before [`Self::sync_entities_from_snapshot`]
    /// so freshly-spawned avatars land in the new world rather
    /// than the old one.
    fn net_pre_phase(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Forward any pending roster lookup the player just
        // confirmed on the account-entry screen. Must happen
        // before `net.step` so the request goes out on the same
        // frame.
        if let Some(account_name) = state.net.roster_request.take() {
            net.request_roster(account_name);
        }

        net.step(Duration::from_secs_f32(dt), Some(input));

        // Drain any roster reply the server has sent and feed it
        // into the character-select screen so it can leave the
        // loading view.
        if let Some(entries) = net.take_roster() {
            state.apply_server_roster(entries);
        }

        // Keep the SP regen path using the server's seed.
        state.net.floor_seed = Some(net.rift_seed());

        // Apply any server-driven floor transition. This sets
        // `app_state = NetEntering(FadeOut)`; the staged tick
        // in `transition::tick_net_entering` walks the phases
        // (one per frame) so the loading overlay presents
        // before the heavy regen runs.
        if let Some(pf) = net.take_pending_floor() {
            state.net.floor_seed = Some(pf.seed);
            // Auto-pivot the chat's active outbound channel
            // to FLOOR (rift) or HUB (hub). The chat itself
            // skips the pivot once the player has manually
            // chosen a channel, so manual picks persist
            // across transitions.
            state.chat.on_floor_changed(pf.is_hub);
            state.apply_net_transition(renderer, pf.index);
            // Snap the freshly-spawned local Player to the
            // server's spawn point so the next snapshot's
            // correction error is small.
            use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
            if let Some((entity, _)) = state
                .world
                .query::<(&Player, &LocalPlayer)>()
                .iter()
                .map(|(e, _)| (e, ()))
                .next()
            {
                if let Ok(mut t) = state.world.get::<&mut Transform>(entity) {
                    t.position = pf.spawn_pos;
                }
            }
        }

        // Resolve the party-frame click intent (a character
        // name set by the UI when the player left-clicked a
        // party frame while a friendly-target ability was
        // armed) into a `NetId` here, where we hold the net
        // session. The combat tick consumes the resolved id
        // on the next frame and confirms the cast — this lag
        // is fine since the player clicked the previous
        // render frame.
        if let Some(name) = state.frame.party_click_target_name.take() {
            state.frame.party_click_target_net_id = net.net_id_for_name(&name);
        }
    }

    /// Reconcile every ECS entity that's driven off the latest
    /// snapshot: the local player's predicted transform, remote
    /// avatars (animation + skinning), enemies, projectiles, and
    /// loot pillars. Must run before `state.update` so the SP
    /// camera + render-sync see authoritative positions this
    /// frame.
    fn sync_entities_from_snapshot(&mut self, renderer: &mut Renderer, dt: f32) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        net.sync_local_player(&mut state.world);
        net.sync_avatars(
            &mut state.world,
            renderer,
            &mut state.anim_cache,
            &mut state.avatar_cosmetics_cache,
        );
        net.sync_enemies(&mut state.world, renderer, &mut state.floor_mgr.monsters);
        net.sync_projectiles(renderer, dt);

        // Spawn loot-pillar visuals from snapshot rows. The
        // `LootDropped` event is the fast path for fresh kills
        // (zero-snapshot latency), but a freshly-joined client
        // also needs to see drops that were already on the
        // floor. `spawn_loot_drop_visual` is idempotent on loot
        // id so the two paths can both fire safely.
        for re in net.remote.values() {
            if let rift_net::messages::EntityKind::Loot { item } = &re.kind {
                // Skip rows whose claim we've already resolved —
                // a snapshot in flight when the server despawned
                // the loot would otherwise resurrect the pillar
                // after `resolve_loot_claim` tore it down.
                if state.loot.claimed_ids.contains(&re.net_id) {
                    continue;
                }
                rift_client::game::state::on_loot_dropped(
                    &mut state.loot,
                    renderer,
                    &mut state.loot_model_cache,
                    re.net_id,
                    re.position,
                    item.clone(),
                    net.local_gender()
                        .map(rift_client::net::wire_gender_to_game),
                );
            }
        }

        // Reconcile revive-shrine visuals against the snapshot.
        // Server sends one row per active shrine with rolled
        // progress + channel counts; we mirror the set so a
        // fresh joiner sees an existing shrine and a completed
        // shrine vanishes the tick its row drops out.
        let shrine_rows: std::collections::HashMap<rift_net::NetId, (glam::Vec3, f32, u8, u8)> =
            net.remote
                .values()
                .filter_map(|re| match re.kind {
                    rift_net::messages::EntityKind::ReviveShrine {
                        progress,
                        channelers,
                        required,
                    } => Some((
                        re.net_id,
                        (re.position, progress as f32 / 255.0, channelers, required),
                    )),
                    _ => None,
                })
                .collect();
        rift_client::game::shrine_system::sync_visuals(&mut state.shrines, renderer, &shrine_rows);
        // Drop the local channel intent if the shrine we were
        // channeling no longer exists (completion / floor change).
        if let Some(target) = state.shrines.local_intent {
            if !shrine_rows.contains_key(&target) {
                state.shrines.local_intent = None;
            }
        }
    }

    /// Drain reliable world events from the server and dispatch
    /// each one to a kind-specific handler. Most are visual side
    /// effects (combat text, blood decals, AoE emitters); damage
    /// is the only one with an authoritative gameplay effect on
    /// the client (the local Health component, mirrored from
    /// `health_pct` in the snapshot).
    fn handle_world_events(&mut self, renderer: &mut Renderer) {
        // Drain into a local Vec so we don't hold a mutable
        // borrow on `self.net` while the per-event handlers also
        // need to touch it (read `our_net_id`, `avatar_entities`,
        // `last_positions`).
        let events = match self.net.as_mut() {
            Some(net) => net.drain_events(),
            None => return,
        };
        for ev in events {
            self.handle_world_event(ev, renderer);
        }
    }

    fn handle_world_event(&mut self, ev: rift_net::messages::WorldEvent, renderer: &mut Renderer) {
        use rift_net::messages::WorldEvent;
        match ev {
            WorldEvent::Damage {
                target,
                amount,
                crit,
                position,
            } => {
                self.handle_damage_event(target, amount, crit, position, renderer);
            }
            WorldEvent::Death {
                entity, hit_dir, ..
            } => {
                self.handle_death_event(entity, hit_dir, renderer);
            }
            WorldEvent::AbilityCast {
                caster,
                ability,
                dir,
                target,
                origin,
                ..
            } => {
                self.handle_ability_cast_event(caster, ability, dir, target, origin, renderer);
            }
            WorldEvent::Hit { target, .. } => {
                log::debug!("net: Hit target={target:?}");
            }
            WorldEvent::LootDropped {
                loot,
                item,
                position,
            } => {
                self.handle_loot_dropped_event(loot, item, position, renderer);
            }
            WorldEvent::ChannelTick {
                caster,
                ability,
                position,
                dir,
                ..
            } => {
                self.handle_channel_tick_event(caster, ability, position, dir);
            }
            WorldEvent::ChannelEnd { caster, ability } => {
                self.handle_channel_end_event(caster, ability);
            }
            WorldEvent::PlayerGhosted { entity, position } => {
                self.handle_player_ghosted_event(entity, position, renderer);
            }
            WorldEvent::PlayersRevived { entities } => {
                self.handle_players_revived_event(entities, renderer);
            }
            WorldEvent::Heal {
                caster,
                target,
                amount,
                over_time,
                position,
            } => {
                self.handle_heal_event(caster, target, amount, over_time, position, renderer);
            }
            WorldEvent::EnemyTelegraph {
                source,
                kind,
                position,
            } => {
                // Stub: a future audio system will play a
                // role-specific wind-up cue keyed on `kind`.
                // Logging at trace keeps the console quiet
                // unless you actually want to debug telegraphs.
                log::trace!("net: EnemyTelegraph source={source:?} kind={kind} pos={position:?}");
                let _ = (source, kind, position, renderer);
            }
        }
    }

    /// Spawn floating combat text for damage we just took or
    /// dealt. Direct hits go through `spawn_player_damage` so
    /// they're styled distinctly from damage we dealt out.
    fn handle_damage_event(
        &mut self,
        target: rift_net::NetId,
        amount: f32,
        crit: bool,
        position: [f32; 3],
        renderer: &mut Renderer,
    ) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        let world_pos = Vec3::from_array(position);
        if Some(target) == net.our_net_id() {
            state.combat_text.spawn_player_damage(world_pos, amount);
        } else {
            state.combat_text.spawn_damage(world_pos, amount, crit);
            // Visceral blood spurt on enemy hits. Anchored a
            // little above the snapshot position so droplets fly
            // from chest height rather than the feet. Skipped
            // for self-hits — the local player already has the
            // red vignette to communicate "you got hit".
            renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::blood_hit_spurt(Vec3::Y),
                world_pos + Vec3::new(0.0, 1.0, 0.0),
            );
        }
    }

    /// Friendly mirror of [`Self::handle_damage_event`]: spawn
    /// a green floating-heal number on the target plus a one-shot
    /// heal-burst VFX at chest height. Heal-over-time tick rows
    /// suppress the burst (the floating number alone communicates
    /// the regen tick) and rely on the sustained
    /// [`VfxKind::HealOverTimeAura`] spawned at cast time.
    fn handle_heal_event(
        &mut self,
        _caster: rift_net::NetId,
        _target: rift_net::NetId,
        amount: f32,
        over_time: bool,
        position: [f32; 3],
        renderer: &mut Renderer,
    ) {
        let Self { state, .. } = self;
        let world_pos = Vec3::from_array(position);
        state.combat_text.spawn_heal(world_pos, amount);
        if !over_time {
            renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::heal_burst(),
                world_pos + Vec3::new(0.0, 1.0, 0.0),
            );
        }
    }

    /// Drop a blood decal at the dying entity's last known
    /// position. The snapshot may have already culled the row by
    /// the time the reliable Death event arrives, so we rely on
    /// `NetClient.last_positions` which persists across
    /// snapshots.
    fn handle_death_event(
        &mut self,
        entity: rift_net::NetId,
        hit_dir: [f32; 3],
        renderer: &mut Renderer,
    ) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        let pos_opt = net.last_positions.get(&entity).copied();
        log::info!(
            "net: Death entity={entity:?} have_pos={} ({:?})",
            pos_opt.is_some(),
            pos_opt
        );
        // Record the death authoritatively. The snapshot is
        // view-culled, so the row may already have vanished from
        // `self.remote` for cull reasons rather than death — the
        // world-sync despawn pass needs this set to know which
        // case it's in (and whether to spawn `enemy_soul_return`).
        net.dead_net_ids.insert(entity);
        if let Some(pos) = pos_opt {
            // Reconstruct the impact direction. The server
            // attaches the killing-blow impulse to the Death
            // event (projectile velocity, AoE radial-outward,
            // etc.) so a fireball kill throws the splatter
            // along the bolt's flight path even when the
            // victim was standing still. Falls back to the
            // entity's last-frame velocity for paths that
            // don't carry direction (DoT ticks); falls back
            // again to a position-hashed angle inside the
            // blood system if both are zero.
            let event_dir = Vec3::from_array(hit_dir);
            let kill_dir = if event_dir.length_squared() > 1e-4 {
                event_dir
            } else {
                net.last_velocities
                    .get(&entity)
                    .copied()
                    .unwrap_or(Vec3::ZERO)
            };
            // Approximate "how violent was this kill?" from the
            // square-magnitude of the impulse. Trash mobs cap
            // around 3–4 m/s; bosses / boss melees push 8+ m/s.
            // The decal system clamps to 0..=1 internally.
            let speed_sq = kill_dir.length_squared();
            let power = (speed_sq / 64.0).clamp(0.0, 1.0);
            let ctx = rift_engine::renderer::blood::KillContext {
                pos,
                dir: kill_dir,
                power,
            };
            // Persistent floor blood: written into the per-floor
            // accumulation texture by the splat pass and sampled
            // by the forward shader as a real wet/dry material.
            let now = renderer.elapsed_secs();
            renderer
                .blood_field
                .splat_for_kill(ctx, now, &state.floor.wall_aabbs);
            // Big visceral burst on top of it. Anchored at
            // chest height so the upward cone reads as the kill
            // shot rather than ground splatter. A tiny
            // deterministic jitter (per-NetId) shifts overlapping
            // bursts apart by ~10cm so two enemies dying on top
            // of each other don't perfectly stack into a single
            // visible blob — purely cosmetic, doesn't move the
            // ground stain.
            let nid = entity.0 as u32;
            let jx = ((nid.wrapping_mul(0x9E37_79B9) >> 16) as f32 / 65535.0 - 0.5) * 0.2;
            let jz = ((nid.wrapping_mul(0x85EB_CA6B) >> 16) as f32 / 65535.0 - 0.5) * 0.2;
            let eid = renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::blood_splatter(Vec3::Y),
                pos + Vec3::new(jx, 1.0, jz),
            );
            log::info!(
                "vfx: spawned blood_splatter for entity={entity:?} \
                 at {:?} eid={eid:?}",
                pos
            );
            // The `enemy_soul_return` puff is intentionally NOT
            // spawned here — it's spawned in the world-sync
            // despawn pass (see `crates/rift-client/src/net/
            // world_sync.rs`) at the moment the corpse actually
            // drops out of snapshots (`DEATH_FADE_DUR` ≈ 1.6s
            // after the kill). Firing it on the Death event
            // would float the puff above a still-visible body
            // and lose the "sucked back to hell as they vanish"
            // read; deferring to despawn lines up the smoke
            // with the actual disappearance.
        } else {
            // Diagnostic path: we missed a death-VFX because we
            // never saw a snapshot row for `entity`. Most likely
            // the kill happened inside the same server tick that
            // first spawned the enemy, so the snapshot delivered
            // alongside the Death event no longer included the
            // row to insert into `last_positions`. Surface the
            // miss instead of swallowing it silently — without
            // this the bug presents as "enemies vanish without
            // a splatter".
            log::warn!(
                "net: Death for unknown entity={entity:?} — \
                 last_positions has {} entries; skipping VFX",
                net.last_positions.len()
            );
        }
        // Remote player death: play the death clip on their
        // avatar so observers see them topple instead of just
        // freezing in their last pose. The local player's death
        // clip is driven from `trigger_player_death` (catch-all
        // health detect) since the snapshot's hp=0 row arrives
        // before the reliable Death event.
        if Some(entity) != net.our_net_id() {
            if let Some(&avatar) = net.avatar_entities.get(&entity) {
                rift_client::game::state::on_remote_death(&mut state.world, avatar);
            }
        }
    }

    /// A teammate has finished their death animation and risen
    /// into ghost mode. The server has stopped including their
    /// row in our snapshot, so `world_sync` is about to despawn
    /// their avatar — without a visual cue they'd just pop out
    /// of existence. We spawn a soft cyan-white wisp burst at
    /// their last server position so the disappearance reads as
    /// "their soul left the body" rather than a glitch. The
    /// owning client suppresses this for themselves so a player
    /// rising into spectator mode isn't slapped with a VFX in
    /// front of their own camera.
    fn handle_player_ghosted_event(
        &mut self,
        entity: rift_net::NetId,
        position: [f32; 3],
        renderer: &mut Renderer,
    ) {
        log::info!("net: PlayerGhosted entity={entity:?}");
        let Self { state: _, net } = self;
        let Some(net) = net.as_mut() else { return };
        if Some(entity) == net.our_net_id() {
            return;
        }
        let pos = Vec3::from_array(position) + Vec3::new(0.0, 1.0, 0.0);
        renderer
            .vfx_system
            .spawn(rift_engine::renderer::vfx::presets::ghost_rise(), pos);
    }

    /// Players have been revived by a completed shrine channel.
    /// The server has already cleared their `is_ghost` flag so
    /// the next snapshot will untint our local view; here we
    /// just spawn a celebration VFX at each revived player's
    /// last known position. The local-ghost teardown (engine
    /// `Ghost` marker, animation reset) lives in the snapshot
    /// path that already watches `local_ghost_cached`.
    fn handle_players_revived_event(
        &mut self,
        entities: Vec<rift_net::NetId>,
        renderer: &mut Renderer,
    ) {
        log::info!("net: PlayersRevived count={}", entities.len());
        let Self { state: _, net } = self;
        let Some(net) = net.as_mut() else { return };
        for revived in entities {
            // Try the avatar position (remote players) first,
            // falling back to last_positions (works for the
            // local player too).
            let pos = net
                .last_positions
                .get(&revived)
                .copied()
                .unwrap_or(Vec3::ZERO);
            renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::ghost_rise(),
                pos + Vec3::new(0.0, 1.0, 0.0),
            );
        }
    }

    /// Spawn the AoE-zone visual for a server-confirmed cast and
    /// trigger the upper-body cast pose on remote casters. The
    /// local caster's pose is already running from `tick_combat`
    /// the moment the input fired, so we pass `caster_avatar =
    /// None` for our own casts to skip the pose hop.
    fn handle_ability_cast_event(
        &mut self,
        caster: rift_net::NetId,
        ability: u16,
        dir: [f32; 2],
        target: Option<[f32; 3]>,
        origin: [f32; 3],
        renderer: &mut Renderer,
    ) {
        log::debug!("net: AbilityCast caster={caster:?} ability={ability}");
        let aim = Vec3::new(dir[0], 0.0, dir[1]);
        let cast_origin = Vec3::from_array(origin);
        let target_pos = target.map(Vec3::from_array);
        // Ground-slam telegraph / impact: visual-only ability
        // casts where `dir[0]` carries the slam radius and
        // `dir[1]` carries the wind-up duration (wind-up only).
        // Centred on `target` (= caster position at cast). No
        // pose / cast-spark — these are emitted from enemy AI,
        // not the player cast pipeline.
        match ability as u8 {
            rift_game::abilities::id::GROUND_SLAM_WINDUP => {
                let centre = target_pos.unwrap_or(cast_origin);
                let radius = dir[0].max(0.5);
                let duration = dir[1].max(0.05);
                renderer.vfx_system.spawn(
                    rift_engine::renderer::vfx::presets::ground_slam_telegraph(radius, duration),
                    centre + Vec3::new(0.0, 0.05, 0.0),
                );
                return;
            }
            rift_game::abilities::id::GROUND_SLAM_IMPACT => {
                let centre = target_pos.unwrap_or(cast_origin);
                let radius = dir[0].max(0.5);
                renderer.vfx_system.spawn(
                    rift_engine::renderer::vfx::presets::ground_slam_impact(radius),
                    centre + Vec3::new(0.0, 0.05, 0.0),
                );
                return;
            }
            _ => {}
        }
        let Some(def) = rift_game::abilities::from_wire_id(ability as u8) else {
            return;
        };

        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };
        let caster_avatar = if Some(caster) == net.our_net_id() {
            None
        } else {
            net.avatar_entities.get(&caster).copied()
        };
        rift_client::game::state::on_remote_ability_cast(
            state,
            renderer,
            &def,
            aim,
            cast_origin,
            target_pos,
            caster_avatar,
        );
    }

    fn handle_loot_dropped_event(
        &mut self,
        loot: rift_net::NetId,
        item: rift_net::messages::ItemBlob,
        position: [f32; 3],
        renderer: &mut Renderer,
    ) {
        log::debug!(
            "net: LootDropped loot={loot:?} base_id={} rarity={} at {:?}",
            item.base_id,
            item.rarity,
            position
        );
        let pos = Vec3::from_array(position);
        let local_gender = self
            .net
            .as_ref()
            .and_then(|n| n.local_gender())
            .map(rift_client::net::wire_gender_to_game);
        rift_client::game::state::on_loot_dropped(
            &mut self.state.loot,
            renderer,
            &mut self.state.loot_model_cache,
            loot,
            pos,
            item,
            local_gender,
        );
    }

    /// Per-tick channel visuals (beam impact, whirlwind sweep).
    /// The cast pose is already running off the initial
    /// `AbilityCast`; this just keeps the per-tick effect
    /// position+aim fresh on every observer.
    fn handle_channel_tick_event(
        &mut self,
        caster: rift_net::NetId,
        ability: u16,
        position: [f32; 3],
        dir: [f32; 2],
    ) {
        log::trace!("net: ChannelTick caster={caster:?} ability={ability}");
        let pos = Vec3::from_array(position);
        let aim = Vec3::new(dir[0], 0.0, dir[1]);
        rift_client::game::state::on_channel_tick(&mut self.state, caster, ability as u8, pos, aim);
    }

    /// Tear down the cast pose on remote avatars (and on us if
    /// the server timed us out before the local timeout did).
    fn handle_channel_end_event(&mut self, caster: rift_net::NetId, ability: u16) {
        log::debug!("net: ChannelEnd caster={caster:?} ability={ability}");
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        let is_local = Some(caster) == net.our_net_id();
        let entity = if is_local {
            state
                .world
                .query::<(
                    &rift_engine::ecs::components::Player,
                    &rift_engine::ecs::components::LocalPlayer,
                )>()
                .iter()
                .map(|(e, _)| e)
                .next()
        } else {
            net.avatar_entities.get(&caster).copied()
        };
        rift_client::game::state::on_channel_end(state, caster, ability as u8, entity, is_local);
    }

    /// Forward this frame's local intentions to the server:
    /// profile + aim, hub↔rift transitions, ability casts,
    /// channel ends, loot pickups, and equip / unequip
    /// requests. None of these read server-pushed state, so
    /// they all live in one phase.
    fn forward_client_commands(&mut self) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Push the chosen profile to the wire as soon as
        // character-select completes. Until this fires the
        // `NetClient` holds back its `Hello`, so the server
        // never sees a placeholder profile.
        if let Some(profile) = state.net.profile.take() {
            use rift_net::messages::Gender as NetGender;
            let gender = match profile.gender {
                rift_game::character::Gender::Male => NetGender::Male,
                rift_game::character::Gender::Female => NetGender::Female,
            };
            // Account name is filled in alongside the profile by
            // the character-select screen. Empty string is a
            // safety net — the entry view doesn't let an empty
            // name confirm, so this should never fire.
            let account_name = state.net.account_name.take().unwrap_or_default();
            net.set_profile(ClientProfile {
                account_name,
                character_name: profile.name,
                class_id: "hero".to_string(),
                gender,
            });
        }

        // After SP code wrote the freshly-computed cursor aim
        // onto the local Player component, copy it to the net
        // client so the next outbound `InputCmd` ships it to
        // the server. The server then replicates it via the
        // `EntityKind::Player.aim_yaw` field, keeping remote
        // spine-twists in sync with where this player is
        // pointing.
        let aim = state
            .world
            .query::<(
                &rift_engine::ecs::components::Player,
                &rift_engine::ecs::components::LocalPlayer,
            )>()
            .iter()
            .map(|(_, (p, _))| p.aim_dir)
            .next()
            .unwrap_or(Vec3::ZERO);
        net.set_aim(aim);

        // Floor transition requests. The actual world
        // regeneration happens later when the server's
        // `LoadFloor` arrives.
        if let Some(req) = state.net.transition.take() {
            match req {
                NetTransitionRequest::EnterRift => net.request_enter_rift(),
                NetTransitionRequest::ReturnToHub => net.request_return_to_hub(),
            }
        }

        // Locally-issued ability casts — drained every frame so
        // a held key doesn't build up a backlog.
        for cast in state.net.casts.drain(..) {
            net.request_cast(
                cast.ability_id,
                cast.origin,
                cast.aim_dir,
                cast.placed_target,
                cast.target_net_id,
            );
        }

        // Channel-end requests (button release / movement-cancel
        // during a hold-to-channel ability).
        for ability_id in state.channel.pending_ends.drain(..) {
            net.request_end_channel(ability_id);
        }

        // Loot-pickup requests. The server validates range and
        // broadcasts `LootClaimed` on success; we tear down the
        // visual + add the item to inventory when that
        // confirmation arrives.
        for loot_id in state.loot.pending_pickups.drain(..) {
            net.request_pickup_loot(loot_id);
        }

        // Equip / unequip requests. UI pushes these in response
        // to clicks; the server is authoritative and replies
        // with fresh Inventory/Equipment syncs.
        for req in state
            .loot
            .pending_equip_requests
            .drain(..)
            .collect::<Vec<_>>()
        {
            match req {
                EquipRequest::Equip { inventory_index } => {
                    net.request_equip_item(inventory_index);
                }
                EquipRequest::Unequip { slot } => {
                    net.request_unequip_item(slot);
                }
                EquipRequest::SwapBag { a, b } => {
                    net.request_swap_inventory_slots(a, b);
                }
                EquipRequest::DropToWorld { inventory_index } => {
                    net.request_drop_inventory_item(inventory_index);
                }
                EquipRequest::Salvage { inventory_index } => {
                    net.request_salvage_inventory_item(inventory_index);
                }
                EquipRequest::SalvageBulk { rarity_max } => {
                    net.request_salvage_inventory_bulk(rarity_max);
                }
                EquipRequest::UnequipToSlot {
                    slot,
                    inventory_index,
                } => {
                    net.request_unequip_to_bag_slot(slot, inventory_index);
                }
            }
        }

        // Stash session toggles. Pushed by the F-prompt at the
        // hub chest. `true` opens, `false` closes.
        for open in state
            .net
            .stash_session_requests
            .drain(..)
            .collect::<Vec<_>>()
        {
            if open {
                net.request_open_stash();
            } else {
                net.request_close_stash();
            }
        }

        // Loadout-slot changes. Pushed by the spellbook UI; the
        // server is authoritative and replies with a fresh
        // `ServerMsg::Loadout`.
        for (slot_index, ability_id) in state
            .net
            .pending_loadout_changes
            .drain(..)
            .collect::<Vec<_>>()
        {
            net.request_set_loadout_slot(slot_index, ability_id);
        }

        // Rift exit-vote requests: F-press on the rift-spawn
        // portal flips this flag for one frame. The server
        // either short-circuits (solo → instant transition) or
        // broadcasts a fresh `RiftExitVote` snapshot which the
        // HUD vote panel then renders.
        if std::mem::take(&mut state.net.pending_exit_vote_start) {
            net.request_exit_vote_start();
        }
        // Y/N casts during an active vote. The server filters
        // out duplicates / non-Pending voters, so it's safe to
        // forward whatever the input layer queued.
        for yes in state
            .net
            .pending_exit_vote_casts
            .drain(..)
            .collect::<Vec<_>>()
        {
            net.request_exit_vote_cast(yes);
        }

        // Revive-shrine intent edge → server. Sent only when
        // the gameplay tick has flagged a transition (start /
        // stop / target swap). The server validates range +
        // alive status; we already mirror the intent locally
        // for the HUD prompt + beam VFX.
        if let Some(intent) = state.net.pending_shrine_intent.take() {
            net.request_set_shrine_channel(intent);
        }

        // Chat: drain inbound lines from the net session into
        // the HUD scrollback, then ship any outbound lines the
        // chat UI queued this frame.
        for inbound in net.take_pending_chats() {
            let our_name = net.character_name().map(|s| s.to_string());
            state.chat.push(
                rift_client::game::chat::ChatLine {
                    channel: inbound.channel,
                    sender: inbound.sender,
                    target: inbound.target,
                    text: inbound.text,
                },
                our_name.as_deref(),
            );
        }
        for (channel, target, text) in state.net.pending_chats_out.drain(..).collect::<Vec<_>>() {
            net.send_chat(channel, target, text);
        }

        // Party / portal messages: drain server pushes into
        // the party UI mirror, and ship any outbound intents
        // queued by the chat slash parser, the right-click
        // context menu, the portal modal, and the per-member
        // confirm modal.
        if let Some(msg) = net.take_pending_party_state() {
            if let rift_net::messages::ServerMsg::PartyState { leader, members } = msg {
                if let Some(name) = net.character_name() {
                    state.party.set_our_name(name.to_string());
                }
                state.party.ingest_state(leader, members);
            }
        }
        for invite in net.take_pending_party_invites() {
            state.party.ingest_invite(invite);
        }
        for err in net.take_pending_party_errors() {
            state.party.ingest_error(err);
        }
        if let Some(prompt) = net.take_pending_portal_prompt() {
            state.party.ingest_portal_prompt(prompt);
        }
        if let Some(value) = net.take_pending_deepest_floor() {
            state.party.ingest_deepest_floor(value);
        }
        if std::mem::take(&mut state.net.pending_open_portal_modal) {
            state.party.open_portal_modal();
        }
        for msg in state.net.pending_party_msgs.drain(..).collect::<Vec<_>>() {
            net.send_party_msg(msg);
        }
        if let Some((floor, mode)) = state.net.pending_propose_rift_entry.take() {
            net.request_propose_rift_entry(floor, mode);
        }
        if let Some(accept) = state.net.pending_portal_confirm.take() {
            net.request_portal_confirm(accept);
        }

        // Stash transfer requests (deposit / withdraw). Drained
        // alongside equip requests; the server replies with
        // fresh InventorySync + StashSync.
        for req in state
            .loot
            .pending_stash_requests
            .drain(..)
            .collect::<Vec<_>>()
        {
            match req {
                StashRequest::Deposit {
                    inventory_index,
                    tab_index,
                } => {
                    net.request_deposit_to_stash(inventory_index, tab_index);
                }
                StashRequest::Withdraw {
                    tab_index,
                    stash_index,
                } => {
                    net.request_withdraw_from_stash(tab_index, stash_index);
                }
                StashRequest::Swap { tab_index, a, b } => {
                    net.request_swap_stash_slots(tab_index, a, b);
                }
                StashRequest::DepositToSlot {
                    inventory_index,
                    tab_index,
                    stash_index,
                } => {
                    net.request_deposit_to_stash_slot(inventory_index, tab_index, stash_index);
                }
                StashRequest::WithdrawToSlot {
                    tab_index,
                    stash_index,
                    inventory_index,
                } => {
                    net.request_withdraw_from_stash_slot(tab_index, stash_index, inventory_index);
                }
                StashRequest::BuyTab => {
                    net.request_buy_stash_tab();
                }
                StashRequest::RenameTab { tab_index, name } => {
                    net.request_rename_stash_tab(tab_index, name);
                }
                StashRequest::RecolorTab { tab_index, color } => {
                    net.request_recolor_stash_tab(tab_index, color);
                }
            }
        }
    }

    /// Apply server-pushed UI state that's safe to integrate
    /// after the SP frame ran: loot-claim confirmations,
    /// inventory + equipment mirrors, the XP bar, and the rift
    /// progress meter. None of these affect input handling for
    /// the next frame, so they live here instead of in
    /// `net_pre_phase`.
    fn apply_server_pushed_state(&mut self, renderer: &mut Renderer) {
        let Self { state, net } = self;
        let Some(net) = net.as_mut() else { return };

        // Loot-claim confirmations. `claimed_by == our_client_id`
        // means *we* picked it up; everyone else just removes
        // the visual.
        let our_client_id = net.our_client_id();
        for (loot, claimed_by) in net.drain_loot_claims() {
            let mine = our_client_id == Some(claimed_by);
            state.resolve_loot_claim(renderer, loot, mine);
        }

        // Loot-pickup rejections (server enforced cap, etc.).
        // The drop is left on the ground; we drop our pending
        // request and warn the player so they know to make room.
        for (loot, reason) in net.drain_pickup_rejections() {
            state.loot.pending_pickups.retain(|id| *id != loot);
            match reason {
                rift_net::messages::PickupRejectReason::InventoryFull => {
                    state.warn_inventory_full();
                }
                rift_net::messages::PickupRejectReason::NotEligible => {
                    state.warn_not_eligible();
                }
            }
        }

        // Authoritative inventory mirror. Items arrive as
        // `ItemBlob` so we round-trip through `Item::from_wire`;
        // rows that fail to decode (mismatched build) are
        // dropped — the next sync will correct it.
        if let Some(blobs) = net.drain_inventory_sync() {
            let items: Vec<Option<rift_game::loot::Item>> = blobs
                .into_iter()
                .map(|opt| {
                    opt.and_then(|b| {
                        let prov = b
                            .provenance
                            .clone()
                            .map(|v| rift_game::loot::LootProvenance::from_ids(v));
                        // `from_wire` always reconstructs a
                        // stable item; layer the blob's
                        // unstable flag on top so server
                        // state survives the round-trip.
                        rift_game::loot::Item::from_wire(
                            b.base_id, b.rarity, b.ilvl, &b.affixes, b.anchored, prov,
                        )
                        .map(|mut it| {
                            it.unstable = b.unstable;
                            it
                        })
                    })
                })
                .collect();
            let filled = items.iter().filter(|s| s.is_some()).count();
            log::info!("client: hydrated mp_inventory with {} item(s)", filled);
            state.loot.items = items;
        }

        // Authoritative equipment mirror. Same shape as
        // inventory; failed-to-decode rows are dropped.
        if let Some(slots) = net.drain_equipment_sync() {
            let mut equip = rift_game::loot::Equipment::new();
            for (slot_byte, blob) in slots {
                let Some(slot) = rift_game::loot::EquipSlot::from_u8(slot_byte) else {
                    continue;
                };
                let Some(item) = rift_game::loot::Item::from_wire(
                    blob.base_id,
                    blob.rarity,
                    blob.ilvl,
                    &blob.affixes,
                    blob.anchored,
                    blob.provenance
                        .clone()
                        .map(|v| rift_game::loot::LootProvenance::from_ids(v)),
                )
                .map(|mut it| {
                    it.unstable = blob.unstable;
                    it
                }) else {
                    continue;
                };
                equip.set(slot, Some(item));
            }
            log::info!("client: hydrated equipment with {} slot(s)", equip.count());
            state.loot.equipment = equip;
            state.player_state.recompute_stats(&state.loot.equipment);

            // Refresh the local player's modular outfit attachments
            // so anything with a `BaseItem::model_path` shows up
            // (or disappears) on the avatar in lock-step with the
            // server-authoritative equipment. The apply itself
            // no-ops when the avatar entity hasn't been spawned
            // yet (true on the very first sync, which lands
            // during `EnteringWorld`), so we also flag the state
            // dirty and let the frame loop retry once the avatar
            // exists.
            state.loot.equipment_visuals_dirty = true;
            rift_client::game::equipment_visuals::apply_local_equipment_visuals(state, renderer);
            if rift_client::game::equipment_visuals::has_local_player(&state.world) {
                state.loot.equipment_visuals_dirty = false;
            }
        }

        // Authoritative stash mirror. Decoded the same way as
        // the bag — failed-to-decode rows are dropped and the
        // next sync corrects. One client tab per server tab.
        if let Some(tab_blobs) = net.drain_stash_sync() {
            use rift_client::game::states::sub_state::StashTabClient;
            let mut total_items = 0usize;
            let tabs: Vec<StashTabClient> = tab_blobs
                .into_iter()
                .map(|tab| {
                    let items: Vec<Option<rift_game::loot::Item>> = tab
                        .items
                        .into_iter()
                        .map(|opt| {
                            opt.and_then(|b| {
                                let prov = b
                                    .provenance
                                    .clone()
                                    .map(|v| rift_game::loot::LootProvenance::from_ids(v));
                                rift_game::loot::Item::from_wire(
                                    b.base_id, b.rarity, b.ilvl, &b.affixes, b.anchored, prov,
                                )
                                .map(|mut it| {
                                    it.unstable = b.unstable;
                                    it
                                })
                            })
                        })
                        .collect();
                    total_items += items.iter().filter(|s| s.is_some()).count();
                    StashTabClient {
                        name: tab.name,
                        color: tab.color,
                        items,
                    }
                })
                .collect();
            log::info!(
                "client: hydrated stash with {} item(s) across {} tab(s)",
                total_items,
                tabs.len(),
            );
            state.loot.stash_tabs = tabs;
        }

        // Authoritative XP / level snapshots.
        if let Some((level, xp, xp_to_next)) = net.drain_character_stats() {
            let prev_level = state.player_state.experience.level;
            state.player_state.experience.level = level.max(1);
            state.player_state.experience.current_xp = xp;
            state.player_state.experience.total_xp = state.player_state.experience.total_xp.max(xp);
            state.player_state.experience.set_xp_to_next(xp_to_next);
            if prev_level != level {
                state.player_state.recompute_stats(&state.loot.equipment);
                state.frame.level_up_flash = 1.0;
                log::info!("client: leveled up to {level}");
            }
        }

        // Authoritative ability-loadout snapshots. Server is the
        // source of truth; mutate `PlayerState::loadout` and
        // re-materialize the runtime `AbilitySlot`.
        if let Some(slots) = net.drain_loadout() {
            state.player_state.loadout = rift_game::loadout::Loadout::from_slots(slots);
            state.player_state.abilities = state.player_state.loadout.materialize();
        }

        // Authoritative shard balance. Pushed by the server on
        // hello + after every salvage; mirror onto the local
        // player state so the HUD readout updates the same
        // frame the server confirms the change.
        if let Some(amount) = net.drain_shards() {
            state.player_state.shards = amount;
        }

        // Per-peer visible equipment. Each entry maps a peer
        // `ClientId` to the base-item indices currently
        // equipped by that player. We resolve to the avatar
        // entity (skipping entries whose avatar hasn't been
        // spawned yet — `world_sync` will re-queue when it
        // does) and reconcile the attachment set in place.
        for (client_id, base_ids) in net.drain_peer_equipment_visuals() {
            let Some(entity) = net.avatar_for_client(client_id) else {
                continue;
            };
            // The peer's avatar was spawned with their gender's
            // base mesh; pick the matching gendered model from
            // each item so attachments share the host skeleton.
            let Some(profile) = net.profile_for_client(client_id) else {
                continue;
            };
            let gender = rift_client::net::wire_gender_to_game(profile.gender);
            let desired = rift_client::game::equipment_visuals::desired_visuals_for_base_ids(
                &base_ids, gender,
            );
            rift_client::game::equipment_visuals::apply_equipment_visuals(
                &mut state.world,
                renderer,
                &mut state.equipment_visual_cache,
                entity,
                &desired,
            );
        }

        // Authoritative rift-progress snapshots.
        if let Some((progress, required, boss_spawned, boss_killed, complete)) =
            net.drain_rift_progress()
        {
            state.rift.progress = progress as f32;
            state.rift.progress_required = required.max(1) as f32;
            state.rift.boss_spawned = boss_spawned;
            state.rift.boss_killed = boss_killed;
            state.rift.floor_complete = complete;
        }

        // Authoritative combat-meter snapshot (~1 Hz).
        if let Some((elapsed, entries)) = net.take_pending_meters() {
            state.meters.apply_snapshot(elapsed, entries, net);
        }

        // Authoritative rift exit-vote snapshot. Mirrored onto
        // `GameState::exit_vote` for the HUD vote panel and the
        // gameplay-thread Y/N input gate.
        if let Some(vote) = net.drain_exit_vote() {
            state.exit_vote = Some(vote);
        }

        // Mirror our authoritative `NetId` so gameplay-thread
        // code can identify the local voter without holding a
        // reference to `NetClient`.
        state.net.our_net_id_cached = net.our_net_id();
        state.net.local_ghost_cached = net.is_local_ghost();

        // Mirror authoritative essence pool fraction onto
        // `PlayerState`. The HUD reads this every frame to drive
        // the essence bar; the canonical scalar lives on the
        // server and is round-tripped via the snapshot's
        // `resource_pct` field.
        state.player_state.resource_pct = net.local_resource_pct();

        // Deferred local-equipment visual apply: the first
        // `EquipmentSync` arrives before the local avatar has
        // been spawned (during `EnteringWorld`), so the
        // immediate-on-receive apply silently no-ops. Retry
        // here every frame the flag is set until the avatar
        // exists, then clear.
        if state.loot.equipment_visuals_dirty
            && rift_client::game::equipment_visuals::has_local_player(&state.world)
        {
            rift_client::game::equipment_visuals::apply_local_equipment_visuals(state, renderer);
            state.loot.equipment_visuals_dirty = false;
        }
    }
}

/// Parsed command-line arguments. Tiny, ad-hoc — clap is overkill for
/// two flags. Once we grow more options we'll graduate.
struct Args {
    connect: Option<SocketAddr>,
}

/// Compile-time default server address baked into the client. Set
/// at build time via the `RIFT_DEFAULT_SERVER` env var (read by
/// `build.rs`); falls back to the local dev server. Players who
/// just double-click `rift.exe` connect here without needing a
/// flag. Override at runtime with `--connect`, `RIFT_SERVER`, or
/// `--offline`.
const DEFAULT_SERVER: Option<&str> = option_env!("RIFT_DEFAULT_SERVER");

fn parse_args() -> Args {
    let mut connect: Option<SocketAddr> = None;
    let mut explicit_offline = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--connect" => {
                let v = iter.next().expect("--connect requires an address");
                connect = Some(v.parse().expect("invalid --connect address"));
            }
            "--offline" => {
                explicit_offline = true;
            }
            "--help" | "-h" => {
                eprintln!(
                    "rift [--connect host:port] [--offline]\n\
                     \n\
                     Defaults to the server baked in at build time\n\
                     (RIFT_DEFAULT_SERVER), or to the RIFT_SERVER env\n\
                     var if set. Pass --offline to skip multiplayer\n\
                     entirely."
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    // Resolution order: explicit --connect > $RIFT_SERVER >
    // compile-time default. --offline trumps everything.
    if connect.is_none() && !explicit_offline {
        if let Ok(env_addr) = std::env::var("RIFT_SERVER") {
            if !env_addr.is_empty() {
                connect = Some(env_addr.parse().expect("invalid RIFT_SERVER address"));
            }
        }
    }
    if connect.is_none() && !explicit_offline {
        if let Some(default) = DEFAULT_SERVER {
            connect = Some(
                default
                    .parse()
                    .expect("invalid RIFT_DEFAULT_SERVER baked at build time"),
            );
        }
    }
    Args { connect }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = parse_args();

    let net = if let Some(addr) = args.connect {
        // Open the connection now so the handshake happens in
        // parallel with character-select. The cosmetic profile is
        // pushed via `set_profile` once the player picks one, which
        // also unblocks Hello.
        Some(NetClient::connect(addr)?)
    } else {
        None
    };

    let window = Window::new("Rift Crawler", 1280, 720);
    window.run(RiftApp {
        state: GameState::new(),
        net,
    })
}
