//! Server-side ability kernel.
//!
//! The static ability table itself lives in `rift-game` so the
//! client can share it (cooldown UI / button → ability mapping).
//! This module turns a [`CombatIntent`] (player input or AI
//! decision) into authoritative simulation effects through a
//! single pipeline:
//!
//! ```text
//! CombatIntent ──► submit() ──► AcceptedCast ──► dispatch() ──► effects
//! ```
//!
//! [`submit`] runs the per-source validation (loadout / cooldown
//! / origin sanity for players; trivial for AI) and produces a
//! normalised [`AcceptedCast`]. [`dispatch`] then matches on the
//! ability's [`AbilityKind`] and runs the effects (spawn
//! projectiles, place AoE zones, attach channels, queue summons,
//! …) — one match arm per kind, shared by every caster type.
//!
//! Both call sites (`Sim::cast_ability` for players, `Sim::step`
//! for AI cast resolves) compose `submit` + `dispatch` inline.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use hecs::Entity;
use rift_dungeon::Floor;
use rift_net::{messages::WorldEvent, ClientId, NetId, NetTick};

pub use rift_game::abilities::id;
pub use rift_game::abilities::{lookup, AbilityKind, AbilityWireId, TargetingMode};

use super::player::ServerPlayer;
use super::projectile::{ServerAoeZone, ServerProjectile, Team};

/// Number of cooldown slots tracked per player. Plenty of headroom
/// over the 6 ability ids in use today; bumping this is free.
pub const COOLDOWN_SLOTS: usize = 16;

/// Per-player cooldown state. Indexed by ability id.
pub type CooldownTable = HashMap<ClientId, [f32; COOLDOWN_SLOTS]>;

/// Decay every active cooldown by `dt`.
pub fn tick_cooldowns(cooldowns: &mut CooldownTable, dt: f32) {
    for cds in cooldowns.values_mut() {
        for cd in cds.iter_mut() {
            if *cd > 0.0 {
                *cd = (*cd - dt).max(0.0);
            }
        }
    }
}

/// Reset every cooldown for every player. Used on floor change.
pub fn clear_cooldowns(cooldowns: &mut CooldownTable) {
    for cds in cooldowns.values_mut() {
        *cds = [0.0; COOLDOWN_SLOTS];
    }
}

// ─── Kernel types ────────────────────────────────────────────────────────
//
// `CombatIntent` is the un-validated input — what a player asked
// to do, or what an AI decided to do. `AcceptedCast` is the
// post-validation, normalised payload that flows into
// [`dispatch`]. Two doors, one room: every cast resolves
// through the same effect pipeline regardless of source, but
// each source runs through its own validation gate so the
// player path's anti-cheat (loadout / cooldown / sanity-origin)
// stays out of the AI path's hot loop.

/// One cast request, before validation. Built by the caller
/// (network handler for players, AI tick for enemies) and fed
/// through [`submit`].
#[derive(Clone, Copy, Debug)]
pub enum CombatIntent {
    /// Player input. `client_origin` is the client-authored
    /// hand position (sanity-checked in [`submit`] against the
    /// authoritative body position so a malicious client can't
    /// teleport the projectile spawn).
    Player {
        client_id: ClientId,
        ability_id: AbilityWireId,
        client_origin: Vec3,
        aim: Vec3,
        placed_target: Option<Vec3>,
        /// Friendly entity target for heal-style casts. `None`
        /// for any ability that doesn't use
        /// [`TargetingMode::TargetEntity`].
        target_net_id: Option<NetId>,
    },
    /// AI-driven cast (e.g. enemy bolt at end of wind-up). The
    /// AI is trusted — there's no validation beyond the kind
    /// dispatch in [`dispatch`].
    Ai {
        caster: NetId,
        /// Casting enemy's `MonsterRole::to_wire_byte()`,
        /// snapshot here so dispatch can stamp it on any
        /// projectile / zone / channel it spawns. Drives the
        /// receiving player's TAKEN-tab attribution.
        attacker_kind: u8,
        ability_id: AbilityWireId,
        origin: Vec3,
        aim: Vec3,
        damage_mult: f32,
        /// Caster's crit chance (0..1). `0.0` means "no crit".
        crit_chance: f32,
        crit_damage: f32,
        /// Ability-specific scalar override (e.g. boss slam
        /// enrage radius). `0.0` falls back to the registry
        /// value at dispatch time.
        param_a: f32,
    },
}

/// One validated cast, ready for [`dispatch`]. Carries
/// everything dispatch needs and nothing it doesn't — by the
/// time we hit dispatch, all source-specific concerns
/// (sessions, cooldown tables, client trust) are out of scope.
#[derive(Clone, Copy, Debug)]
pub struct AcceptedCast {
    pub caster: NetId,
    /// Casting enemy's kind for the TAKEN-tab attribution.
    /// `MonsterRole::to_wire_byte()` for AI casts; left at
    /// [`super::meters::ATTACKER_KIND_OTHER`] for player
    /// casts (the field is unused there).
    pub attacker_kind: u8,
    /// Caster's ECS entity, when known. Player casts always
    /// have one (used for Channel attachment + Evasive Roll).
    /// AI casts pass `None` — enemies don't currently target
    /// effects at their own entity.
    pub caster_entity: Option<Entity>,
    pub ability_id: AbilityWireId,
    /// Authoritative body position of the caster.
    pub origin: Vec3,
    /// Spawn position for projectile-shaped abilities. For
    /// player casts this is the (validated) client-authored
    /// hand position so visuals emerge from the casting hand
    /// on every observer's screen; for AI casts it's a fixed
    /// chest-height offset of `origin`. Pre-baked here so
    /// [`dispatch`] doesn't carry per-source spawn logic.
    pub spawn_origin: Vec3,
    /// Unit XZ aim direction.
    pub aim: Vec3,
    /// Ground-targeted spot for placed-AoE abilities.
    pub placed_target: Option<Vec3>,
    /// Total scalar applied to `ability.base_damage` at effect
    /// time. Player path bakes in the gear / attribute /
    /// element / archetype multipliers here so dispatch is
    /// source-agnostic; AI path passes the floor difficulty
    /// scalar.
    pub damage_scalar: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
    /// Which side this cast belongs to. Drives team-aware
    /// downstream effects (projectile target list, AoE-zone
    /// target list).
    pub team: Team,
    /// Ability-specific scalar override (e.g. effective slam
    /// radius for `DelayedAoe`). `0.0` means "use the
    /// registry value".
    pub param_a: f32,
    /// Resolved entity target for heal-style casts. Set by
    /// [`submit`] from the client-supplied `target_net_id`
    /// after validation; dispatch arms that need it (Heal,
    /// HealOverTimeTarget) panic-on-`None` would be a bug, so
    /// they silently no-op instead.
    pub target_entity: Option<Entity>,
    /// Wire id of `target_entity`, mirrored here so dispatch
    /// can populate `WorldEvent::Heal` without re-querying
    /// the player row.
    pub target_net_id: Option<NetId>,
    /// World-space position of the heal target at submit
    /// time. Used as the `Heal` event position so the
    /// floating green number anchors on the right body even
    /// if the target moves between submit and dispatch.
    pub target_position: Option<Vec3>,
    /// Extra projectile count contributed by gear affixes
    /// (e.g. legendary `+N projectiles to Fireball`). Stacks
    /// additively with the registry-authored `count` inside
    /// the `AbilityKind::Projectiles` dispatch arm. `0` for
    /// AI casts and players without a matching mod.
    pub extra_projectiles: u32,
    /// Global range multiplier from the caster's `Stat::Range`
    /// rolls (`1.0` = no change). Dispatch arms multiply the
    /// per-kind range parameters by this so projectile travel
    /// distance, AoE radius, and beam length all scale with the
    /// same affix. `1.0` for AI casts — enemies don't roll
    /// player gear stats.
    pub range_mult: f32,
    /// Active ability transform (e.g. `FrostRayShatter`)
    /// contributed by a legendary affix. Dispatch arms that
    /// recognise the variant alter their behaviour
    /// accordingly; everyone else ignores it. `None` when no
    /// transform is equipped or for AI casts.
    pub transform: Option<rift_game::loot::AbilityVariant>,
    /// `true` when this cast was synthesised by the proc
    /// dispatcher (Mirrorglass Amulet pool, OnDodge /
    /// OnLowHealth / OnHit `CastAbility` procs). Distinguishes
    /// a free, momentary trigger from a manual cast the
    /// player can control. Read by the `AbilityKind::Channel`
    /// arm to:
    ///   * skip if the caster is already mid-channel (the
    ///     focused cast they actually started has priority
    ///     and must not be silently replaced — replacing a
    ///     `ServerChannel` doesn't emit `ChannelEnd`, which
    ///     orphans the previous beam's client VFX);
    ///   * clamp infinite-duration channels (Frost Ray) to
    ///     a short burst with `cancel_on_move = false` so a
    ///     proc-cast doesn't lock the player into a held
    ///     channel they never opted into.
    /// Always `false` for player / AI casts that went through
    /// `submit`.
    pub is_proc: bool,
}

/// Burst duration applied when a proc-cast targets an
/// [`AbilityKind::Channel`] ability. Tuned so the burst lands
/// a couple of damage ticks (the channel's `tick_interval`
/// dictates the count) without freezing the player's pose for
/// long enough to interrupt their actual input. Short enough
/// that the channel ends well before the proc owner's cast
/// rhythm could trigger another proc.
pub const PROC_CHANNEL_BURST_SECS: f32 = 0.6;

/// Validate a [`CombatIntent`] and produce an
/// [`AcceptedCast`], or return `None` if the intent should be
/// silently dropped (cooldown, missing session, unknown
/// ability id, off-loadout cast …).
///
/// Side effect: on a successful Player intent, the cooldown
/// table is updated. AI intents have no per-caster cooldown
/// table at this layer — the AI ticks its own per-attack
/// cooldowns inside `super::enemy`.
pub fn submit(
    world: &hecs::World,
    sessions: &HashMap<ClientId, Entity>,
    cooldowns: &mut CooldownTable,
    floor: &Floor,
    intent: CombatIntent,
) -> Option<AcceptedCast> {
    match intent {
        CombatIntent::Player {
            client_id,
            ability_id,
            client_origin,
            aim,
            placed_target,
            target_net_id,
        } => {
            let ability = lookup(ability_id)?;
            let &entity = sessions.get(&client_id)?;
            // Reject casts that don't match the player's
            // persisted loadout. A misbehaving client that
            // asks to fire an ability they haven't slotted
            // gets silently dropped here rather than burning
            // the cooldown — checked before we touch the
            // cooldown table so a rejected cast leaves no
            // residue. Evasive Roll is exempt: it's a passive
            // bound to Space on the client and never sits in
            // a loadout slot.
            let p_ref = world.get::<&ServerPlayer>(entity).ok()?;
            if !rift_game::loadout::can_player_cast(&p_ref.loadout, ability_id) {
                return None;
            }
            // Talent-tree gate. Every ability except the
            // always-on neutrals (`PUNCH` per `TALENT_TREE.md`
            // §2.1, and `EVASIVE_ROLL` which is bound to
            // Space and unlocks via the Hub tier-1 dodge
            // node §11.1 — the client treats it as a free
            // passive that ignores the unlock check, so the
            // server must mirror that or rolls silently
            // drop) must have its `UnlockAbility` talent node
            // invested before the player can fire it. Mirrors
            // the client-side gate in `trigger_local_cast` so
            // a misbehaving / desynced client can't bypass the
            // tree by hand-crafting a `Cast` message.
            if ability_id != rift_game::abilities::id::PUNCH
                && ability_id != rift_game::abilities::id::EVASIVE_ROLL
                && !p_ref.talents.is_ability_unlocked(ability.id)
            {
                return None;
            }
            // Snapshot the caster's authoritative state. The
            // borrow drops at end of statement so the
            // cooldown-table mutation below is safe.
            let body = p_ref.k.position;
            let net_id = p_ref.net_id;
            let dmg_scalar = p_ref.damage_scalar();
            let crit_chance = p_ref.stats.crit_chance;
            let crit_damage = p_ref.stats.crit_damage;
            let ability_mult = p_ref.stats.ability_damage_mult(ability);
            // Per-ability affix mods. Fold the
            // `AmplifyAbilityDamage` factor into the damage
            // scalar so dispatch stays mod-agnostic, and pull
            // out the count / transform overrides for
            // dispatch-time consumption.
            let affix_dmg = p_ref.ability_mods.damage_for(ability.id);
            let affix_cd = p_ref.ability_mods.cooldown_for(ability.id);
            let stat_cdr = p_ref.stats.cooldown_reduction;
            let range_mult = p_ref.stats.range.max(0.1);
            let extra_projectiles = p_ref.ability_mods.extra_projectiles_for(ability.id);
            let transform = p_ref.ability_mods.transform_for(ability.id);
            drop(p_ref);

            // Friendly entity-target validation. For abilities
            // that use [`TargetingMode::TargetEntity`] we
            // require a live, in-range, line-of-sight target;
            // any failure silently drops the cast (no cooldown
            // burn, same shape as a loadout reject). A `None`
            // wire target falls back to the caster — Landing 1
            // ships before the client gains a hover-pick UI, so
            // self-cast is the implicit default.
            let (target_entity, target_position) =
                if matches!(ability.targeting, TargetingMode::TargetEntity) {
                    let want = target_net_id.unwrap_or(net_id);
                    // Find the player whose `net_id` matches.
                    // Self-cast is allowed (Shift+key) — the
                    // alive / range / LOS gates apply
                    // uniformly.
                    let mut found: Option<(Entity, Vec3)> = None;
                    for (e, p) in world.query::<&ServerPlayer>().iter() {
                        if p.net_id == want && !p.is_dead_or_ghosting() {
                            found = Some((e, p.k.position));
                            break;
                        }
                    }
                    let (te, tpos) = found?;
                    let d = tpos - body;
                    let dist2 = d.x * d.x + d.z * d.z;
                    let max_range = ability.range * range_mult;
                    if dist2 > max_range * max_range {
                        return None;
                    }
                    if !floor.line_of_sight(body, tpos) {
                        return None;
                    }
                    (Some(te), Some(tpos))
                } else {
                    (None, None)
                };

            // Placed-AoE LoS gate. Matches the client-side
            // visualizer (red ring): we refuse to drop the
            // zone behind a wall so a misbehaving client
            // can't bypass the check.
            if let (TargetingMode::Placed { .. }, Some(target_pos)) =
                (ability.targeting, placed_target)
            {
                if !floor.line_of_sight(body, target_pos) {
                    return None;
                }
            }

            let cds = cooldowns.entry(client_id).or_insert([0.0; COOLDOWN_SLOTS]);
            let slot = (ability_id.raw() as usize).min(COOLDOWN_SLOTS - 1);
            if cds[slot] > 0.0 {
                return None;
            }
            // Essence gate. Has to run after the cooldown
            // check so a free-cast retry of an on-cooldown
            // ability doesn't burn a different ability's
            // resource. We re-borrow the player mutably for
            // the deduct \u2014 the earlier `p_ref` snapshot
            // borrow is already dropped by this point.
            {
                let mut p_mut = world.get::<&mut ServerPlayer>(entity).ok()?;
                if !p_mut.try_spend_resource(ability.resource_cost) {
                    return None;
                }
            }
            // Effective cooldown:
            //   base × affix_cd (per-ability `ReduceAbilityCooldown`)
            //         × (1 - stat_cdr) (gear-wide `CooldownReduction` stat)
            // Floor at 0.05 s so a stack of cdr can't burn the
            // server in a tight cast loop.
            let effective_cd = (ability.cooldown * affix_cd * (1.0 - stat_cdr).max(0.0)).max(0.05);
            cds[slot] = effective_cd;

            // Trust the client's hand-position origin within a
            // sanity radius of the simulated body (~2 m). This
            // lets projectiles visibly emerge from the casting
            // hand on every observer's screen without enabling
            // a teleport-the-spawn exploit. Out-of-range or
            // zero origins fall back to a chest-height offset
            // of the body. Aim-forward nudge matches the old
            // behaviour so the projectile starts just past the
            // hand instead of inside it.
            let trusted_origin = if client_origin.distance_squared(body) <= 2.0 * 2.0 {
                client_origin
            } else {
                body + Vec3::Y * 1.25
            };
            let spawn_origin = trusted_origin + aim * 0.25;

            Some(AcceptedCast {
                caster: net_id,
                attacker_kind: super::meters::ATTACKER_KIND_OTHER,
                caster_entity: Some(entity),
                ability_id,
                origin: body,
                spawn_origin,
                aim,
                placed_target,
                // Pre-bake gear / attribute / element /
                // archetype scaling. Dispatch only knows
                // `ability.base_damage * damage_scalar`.
                damage_scalar: dmg_scalar * ability_mult * affix_dmg,
                crit_chance,
                crit_damage,
                team: Team::Player,
                param_a: 0.0,
                target_entity,
                // Resolved net id may differ from the request:
                // a `None` wire id was rewritten to the caster
                // for self-cast.
                target_net_id: target_entity.map(|_| target_net_id.unwrap_or(net_id)),
                target_position,
                extra_projectiles,
                range_mult,
                transform,
                is_proc: false,
            })
        }
        CombatIntent::Ai {
            caster,
            attacker_kind,
            ability_id,
            origin,
            aim,
            damage_mult,
            crit_chance,
            crit_damage,
            param_a,
        } => {
            // AI is trusted — no validation beyond the
            // existence of a registry entry. Still gate on it
            // so a misauthored ability id is silently dropped.
            lookup(ability_id)?;
            // Caster bolts emerge ~1.1 m up + slight forward
            // offset so the visual reads as coming from the
            // chest, not the feet.
            let spawn_origin = origin + Vec3::Y * 1.1 + aim * 0.4;
            Some(AcceptedCast {
                caster,
                attacker_kind,
                caster_entity: None,
                ability_id,
                origin,
                spawn_origin,
                aim,
                placed_target: None,
                damage_scalar: damage_mult,
                crit_chance,
                crit_damage,
                team: Team::Enemy,
                param_a,
                target_entity: None,
                target_net_id: None,
                target_position: None,
                extra_projectiles: 0,
                range_mult: 1.0,
                transform: None,
                is_proc: false,
            })
        }
    }
}

/// Optional sinks for [`dispatch`]. Each effect produced by
/// the kind match writes into one of these; the caller
/// decides which to pass in. Decoupling from `Sim` directly
/// means dispatch is pure-ish and easy to reason about — every
/// mutation it performs is visible in this struct.
pub struct DispatchSinks<'a> {
    pub aoe_zones: &'a mut Vec<ServerAoeZone>,
    pub events: &'a mut Vec<WorldEvent>,
    pub next_projectile_net_id: &'a mut u32,
    /// Damage rows targeted at players (used by `DelayedAoe`).
    pub player_damage: &'a mut Vec<super::combat_ctx::PlayerHit>,
    /// Healing rows targeted at players. Drained by the caller
    /// after dispatch the same way `player_damage` is — keeps
    /// dispatch from poking `ServerPlayer.hp` directly while
    /// the projectile / channel borrow is in scope (matters for
    /// the AI-tick path even though current heal sources are
    /// player-only, because future buffs may queue heals from
    /// inside the same world borrow).
    pub player_heals: &'a mut Vec<(Entity, f32)>,
    /// Summon spawn requests `(pos, role, hp_mult)` queued for
    /// `Sim::step` to drain into entities. Net-id allocation
    /// stays in `Sim`.
    pub summons: &'a mut Vec<(Vec3, rift_game::monsters::MonsterRole, f32)>,
    /// Live `(entity, position)` rows for every player. Read
    /// by `DelayedAoe` to find who's inside the slam disc.
    pub player_targets: &'a [(Entity, Vec3)],
    /// Queue for player `MeleeArc` swings. Each arm in `dispatch`
    /// pushes one row when a swing fires; the caller (top of the
    /// damage pass in `Sim::step`) drains the queue against the
    /// live enemy snapshot. Deferring resolution lets melee reuse
    /// the same `apply_hits_to_enemies` pipeline that projectile
    /// / channel hits use (aggro, procs, kills, loot) without
    /// `dispatch` having to own a `CombatCtx` itself.
    pub melee_swings: &'a mut Vec<PendingMeleeSwing>,
}

/// One queued melee swing emitted by an `AbilityKind::MeleeArc`
/// dispatch. Resolved at the top of the damage pass — every
/// enemy within `radius` of `origin` whose bearing from `origin`
/// is inside the half-`arc_radians` cone around `aim` takes
/// `damage` exactly once, with the crit roll seeded from the
/// usual `(tick, target, attacker, ability)` tuple.
#[derive(Clone, Copy, Debug)]
pub struct PendingMeleeSwing {
    pub caster_net_id: NetId,
    pub ability_id: AbilityWireId,
    pub origin: Vec3,
    pub aim: Vec3,
    pub radius: f32,
    pub arc_radians: f32,
    pub damage: f32,
    pub crit_chance: f32,
    pub crit_damage: f32,
}

/// Run the effect pipeline for an [`AcceptedCast`]. One match
/// arm per [`AbilityKind`]; both player and AI casts flow
/// through here, so adding a new ability shape is a single
/// arm and (optionally) a registry entry.
pub fn dispatch(
    world: &mut hecs::World,
    accepted: AcceptedCast,
    sinks: &mut DispatchSinks<'_>,
    tick: NetTick,
) {
    let Some(ability) = lookup(accepted.ability_id) else {
        return;
    };
    let scaled_damage = ability.base_damage * accepted.damage_scalar;

    match ability.kind {
        // ── Player-shaped kinds ─────────────────────────────────
        AbilityKind::Projectiles {
            count,
            spread,
            speed,
            ttl,
            pierce,
            apply_debuff,
        } => {
            // FireballToBeam transform (Embercrown Helm) —
            // turns the discrete projectile fan into a short
            // forward beam channel. Hooked at the top of the
            // Projectiles arm so it short-circuits the
            // projectile spawn entirely; the rest of the arm
            // is the unaffected baseline.
            if accepted.transform == Some(rift_game::loot::AbilityVariant::FireballToBeam) {
                if let Some(entity) = accepted.caster_entity {
                    // Proc-cast guard: same reasoning as the
                    // generic Channel arm below. If the
                    // caster is already mid-channel, skip
                    // rather than stomp `ServerChannel` (a
                    // silent replace doesn't emit
                    // `ChannelEnd`, orphaning the previous
                    // beam's client VFX).
                    if accepted.is_proc
                        && world.get::<&super::channel::ServerChannel>(entity).is_ok()
                    {
                        return;
                    }
                    // Beam shape derived from the projectile's
                    // ttl × speed (reach the original bolt would
                    // have travelled), with a short duration so
                    // the spell still feels like a one-shot cast
                    // rather than a hold-to-channel ability. The
                    // damage budget is folded into a per-tick
                    // value so the total beam DPS matches the
                    // single-projectile hit damage at the
                    // canonical pierce of 1 enemy.
                    const BEAM_DURATION: f32 = 0.55;
                    const BEAM_TICK_INTERVAL: f32 = 0.11;
                    let ticks = (BEAM_DURATION / BEAM_TICK_INTERVAL).round().max(1.0);
                    // Beam reach matches the registry-authored
                    // ability range (12 m) — *not* the
                    // projectile's `speed * ttl`, which is the
                    // distance a bolt would coast before
                    // despawning (~40 m for Fireball) and felt
                    // unbounded compared to the channel
                    // animation. `range_mult` still applies so
                    // +Range gear scales the beam.
                    let range = 12.0_f32 * accepted.range_mult;
                    let damage_per_tick = scaled_damage / ticks;
                    // Re-stamp the AbilityCast wire event so
                    // clients render the beam — Fireball's
                    // own visual shape is `Projectile` and
                    // carries no beam recipe. `FIREBALL_BEAM`
                    // is a synthetic registry row that
                    // authors `ShapeVisuals::Beam`; the
                    // client looks up the visual shape by
                    // wire id when handling ChannelTick, so
                    // we need the inserted ServerChannel to
                    // use that id (and we re-emit the
                    // AbilityCast under the same id so the
                    // cast pose / SFX match).
                    sinks.events.push(WorldEvent::AbilityCast {
                        caster: accepted.caster,
                        ability: id::FIREBALL_BEAM.raw() as u16,
                        origin: accepted.spawn_origin.to_array(),
                        dir: [accepted.aim.x, accepted.aim.z],
                        target: None,
                        start_tick: tick,
                    });
                    let _ = world.insert_one(
                        entity,
                        super::channel::ServerChannel {
                            ability_id: id::FIREBALL_BEAM,
                            team: accepted.team,
                            attacker_kind: accepted.attacker_kind,
                            remaining: BEAM_DURATION,
                            elapsed: 0.0,
                            tick_interval: BEAM_TICK_INTERVAL,
                            tick_acc: 0.0,
                            effect: rift_game::abilities::ChannelEffect::Beam {
                                range,
                                width: 1.0,
                                damage_per_tick,
                                pierce_targets: 32,
                            },
                            crit_chance: accepted.crit_chance,
                            crit_damage: accepted.crit_damage,
                            apply_debuff,
                            aim: accepted.aim,
                            // Fireball is an instant cast — the
                            // transformed beam mirrors that
                            // ergonomically by ignoring move
                            // cancel.
                            cancel_on_move: false,
                            transform: accepted.transform,
                            pulse_period: accepted
                                .transform
                                .map(super::transforms::transform_pulse_period)
                                .unwrap_or(0.0),
                            pulse_acc: 0.0,
                        },
                    );
                    // Fire the initial `ChannelPulse` so the
                    // client starts the bead animation in
                    // step with the server's accumulator.
                    // (Subsequent pulses are emitted from the
                    // channel-tick driver.)
                    if let Some(period) = accepted
                        .transform
                        .map(super::transforms::transform_pulse_period)
                        .filter(|p| *p > 0.0)
                    {
                        sinks.events.push(WorldEvent::ChannelPulse {
                            caster: accepted.caster,
                            ability: id::FIREBALL_BEAM.raw() as u16,
                            travel_time: period,
                        });
                    }
                }
                // Silence the unused-binding warning for
                // pierce / spread on this short-circuit path.
                let _ = (pierce, spread, speed, ttl);
                return;
            }
            // Global `Stat::Range` scales projectile travel
            // distance. Multiplying `ttl` (rather than `speed`)
            // keeps projectile feel the same — same launch
            // velocity, just flies longer before despawning.
            let ttl = ttl * accepted.range_mult;
            // Affix-driven extra projectiles (legendary
            // `+N projectiles to <ability>`) stack on top of
            // the registry-authored count. The original
            // `spread` is reused as the *total* fan width so a
            // single-shot ability that picks up `+2 projectiles`
            // becomes a tight 3-shot fan rather than firing
            // straight overlapping bolts; pre-fanned abilities
            // (Fireball Volley etc.) widen proportionally.
            let total_count = count.saturating_add(accepted.extra_projectiles);
            // Default fan width when the registry left the base
            // ability single-shot but an affix added projectiles.
            // ~22° matches the existing Fireball Volley feel without
            // making `extra_projectiles == 1` overlap visually.
            let effective_spread = if count <= 1 && total_count > 1 {
                0.4 // ~23°
            } else {
                spread
            };
            for i in 0..total_count {
                let angle_offset = if total_count > 1 {
                    let t = i as f32 / (total_count - 1) as f32 - 0.5;
                    t * effective_spread
                } else {
                    0.0
                };
                let dir = Quat::from_rotation_y(angle_offset) * accepted.aim;
                let net_id = NetId(*sinks.next_projectile_net_id);
                *sinks.next_projectile_net_id = sinks
                    .next_projectile_net_id
                    .wrapping_add(1)
                    .max(0x4000_0000);
                world.spawn((ServerProjectile {
                    net_id,
                    ability_id: accepted.ability_id,
                    owner: accepted.caster,
                    team: accepted.team,
                    attacker_kind: accepted.attacker_kind,
                    position: accepted.spawn_origin,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance: accepted.crit_chance,
                    crit_damage: accepted.crit_damage,
                    pierce_remaining: pierce,
                    size: 0.6,
                    apply_debuff,
                },));
            }
        }
        AbilityKind::AoeZone {
            radius,
            duration,
            tick_interval,
            apply_debuff,
        } => {
            // Global `Stat::Range` scales AoE radius.
            let radius = radius * accepted.range_mult;
            let pos = accepted
                .placed_target
                .unwrap_or(accepted.origin + accepted.aim * 5.0);
            sinks.aoe_zones.push(ServerAoeZone {
                owner: accepted.caster,
                ability_id: accepted.ability_id,
                attacker_kind: accepted.attacker_kind,
                team: accepted.team,
                position: Vec3::new(pos.x, 0.0, pos.z),
                radius,
                damage_per_tick: scaled_damage,
                crit_chance: accepted.crit_chance,
                crit_damage: accepted.crit_damage,
                tick_interval,
                duration,
                elapsed: 0.0,
                tick_timer: 0.0,
                apply_debuff,
            });
        }
        AbilityKind::Channel {
            duration,
            tick_interval,
            effect,
            apply_debuff,
            cancel_on_move,
        } => {
            // Apply the caster's global range multiplier to the
            // per-tick spatial parameter of the channel effect
            // (beam length / aura radius) so `Stat::Range`
            // grows channels the same way it grows projectiles.
            let effect = match effect {
                rift_game::abilities::ChannelEffect::AuraAroundCaster {
                    radius,
                    damage_per_tick,
                } => rift_game::abilities::ChannelEffect::AuraAroundCaster {
                    radius: radius * accepted.range_mult,
                    damage_per_tick,
                },
                rift_game::abilities::ChannelEffect::Beam {
                    range,
                    width,
                    damage_per_tick,
                    pierce_targets,
                } => rift_game::abilities::ChannelEffect::Beam {
                    range: range * accepted.range_mult,
                    width,
                    damage_per_tick,
                    pierce_targets,
                },
            };
            // Channels need a caster entity to attach to. AI
            // casts pass `None`; if a future enemy ever wants
            // to channel we'll pipe its entity through
            // `AcceptedCast.caster_entity` (insert directly
            // with `team: Team::Enemy`).
            if let Some(entity) = accepted.caster_entity {
                // Proc-cast safety net. A free cast of a
                // Channel ability fired by an on-hit / on-
                // dodge / on-low-health proc (Mirrorglass
                // Amulet pool) has two failure modes the
                // manual path doesn't:
                //
                //   1. If the caster is already mid-channel
                //      (typically Fireball-as-beam from
                //      Embercrown's transform), naively
                //      inserting a fresh `ServerChannel`
                //      stomps the existing one. `insert_one`
                //      replaces the component silently \u2014 no
                //      `ChannelEnd` is emitted, so the
                //      previous beam's client VFX is
                //      orphaned and renders forever.
                //   2. Channels like Frost Ray are authored
                //      with `duration = f32::INFINITY` and
                //      end on key release. A proc-cast has
                //      no key to release, so the channel
                //      latches indefinitely (only essence
                //      drain or movement-cancel can end it).
                //
                // The two guards below address both: skip
                // when a focused channel is already active
                // (their cast takes priority), and otherwise
                // clamp the proc-cast to a short burst with
                // movement-cancel disabled so it always
                // self-terminates.
                let (duration, cancel_on_move) = if accepted.is_proc {
                    if world.get::<&super::channel::ServerChannel>(entity).is_ok() {
                        // Caster is already channeling \u2014
                        // their focused cast wins. Skip the
                        // proc to keep the active VFX /
                        // damage stream intact.
                        return;
                    }
                    (duration.min(PROC_CHANNEL_BURST_SECS), false)
                } else {
                    (duration, cancel_on_move)
                };
                let pulse_period = accepted
                    .transform
                    .map(super::transforms::transform_pulse_period)
                    .unwrap_or(0.0);
                let _ = world.insert_one(
                    entity,
                    super::channel::ServerChannel {
                        ability_id: accepted.ability_id,
                        team: accepted.team,
                        attacker_kind: accepted.attacker_kind,
                        remaining: duration,
                        elapsed: 0.0,
                        tick_interval,
                        tick_acc: 0.0,
                        effect,
                        crit_chance: accepted.crit_chance,
                        crit_damage: accepted.crit_damage,
                        apply_debuff,
                        aim: accepted.aim,
                        cancel_on_move,
                        transform: accepted.transform,
                        pulse_period,
                        pulse_acc: 0.0,
                    },
                );
                if pulse_period > 0.0 {
                    sinks.events.push(WorldEvent::ChannelPulse {
                        caster: accepted.caster,
                        ability: accepted.ability_id.raw() as u16,
                        travel_time: pulse_period,
                    });
                }
            }
        }
        AbilityKind::MeleeArc {
            radius,
            arc_radians,
        } => {
            // Pure damage primitive: queue the cone hit for
            // the resolver. The pose lock + locked lunge
            // direction are stamped on the caster's
            // kinematic by the generic `SetPlayerAction`
            // pass below, which runs after this match for
            // any ability that declares a server-driven
            // pose. Mirrors how every other damage kind
            // (`Projectiles`, `AoeZone`, `Channel`) restricts
            // its arm to the damage shape and lets shared
            // passes handle motion / animation side-effects.
            sinks.melee_swings.push(PendingMeleeSwing {
                caster_net_id: accepted.caster,
                ability_id: accepted.ability_id,
                origin: accepted.origin,
                aim: accepted.aim,
                radius,
                arc_radians,
                damage: scaled_damage,
                crit_chance: accepted.crit_chance,
                crit_damage: accepted.crit_damage,
            });
        }
        AbilityKind::ClientOnly => {
            // No server side-effect on its own. Abilities of
            // this kind that *do* need a caster pose lock
            // (e.g. Evasive Roll) declare it via
            // `AbilityEffect::SetPlayerAction`; the generic
            // pass below picks it up and stamps the
            // caster's kinematic. Keeping the arm empty
            // means a new ClientOnly ability with a pose
            // requirement doesn't need any server code
            // changes — it just authors the effect entry.
        }
        // ── AI-shaped kinds ─────────────────────────────────────
        AbilityKind::EnemyProjectiles {
            count,
            spread,
            speed,
            ttl,
            windup: _,
            size,
            apply_debuff,
        } => {
            for i in 0..count {
                let angle_offset = if count > 1 {
                    let t = i as f32 / (count - 1) as f32 - 0.5;
                    t * spread
                } else {
                    0.0
                };
                let dir = Quat::from_rotation_y(angle_offset) * accepted.aim;
                let net_id = NetId(*sinks.next_projectile_net_id);
                *sinks.next_projectile_net_id = sinks
                    .next_projectile_net_id
                    .wrapping_add(1)
                    .max(0x4000_0000);
                world.spawn((ServerProjectile {
                    net_id,
                    ability_id: accepted.ability_id,
                    owner: accepted.caster,
                    team: accepted.team,
                    attacker_kind: accepted.attacker_kind,
                    position: accepted.spawn_origin,
                    velocity: dir * speed,
                    ttl,
                    damage: scaled_damage,
                    crit_chance: accepted.crit_chance,
                    crit_damage: accepted.crit_damage,
                    pierce_remaining: 0,
                    size,
                    apply_debuff,
                },));
            }
        }
        AbilityKind::DelayedAoe { radius, windup: _ } => {
            // `param_a > 0.0` overrides the registry radius
            // for this cast (e.g. boss slam enrage scaling
            // captured at wind-up start).
            let effective_radius = if accepted.param_a > 0.0 {
                accepted.param_a
            } else {
                radius
            };
            let r2 = effective_radius * effective_radius;
            for (pe, pp) in sinks.player_targets {
                let dx = pp.x - accepted.origin.x;
                let dz = pp.z - accepted.origin.z;
                if dx * dx + dz * dz <= r2 {
                    sinks.player_damage.push(super::combat_ctx::PlayerHit {
                        target: *pe,
                        attacker_kind: accepted.attacker_kind,
                        ability_id: accepted.ability_id,
                        amount: scaled_damage,
                    });
                }
            }
            // Paired impact visual — sustained ground ring
            // is already up from the wind-up event; this
            // fires the shockwave.
            sinks.events.push(WorldEvent::AbilityCast {
                caster: accepted.caster,
                ability: id::GROUND_SLAM_IMPACT.raw() as u16,
                origin: accepted.origin.to_array(),
                dir: [effective_radius, 0.0],
                target: Some(accepted.origin.to_array()),
                start_tick: tick,
            });
        }
        AbilityKind::Summon {
            count,
            role,
            hp_mult,
            ring_radius,
            windup: _,
        } => {
            // Spawn enemies in a ring at evenly-spaced
            // angles. Net-id allocation stays in `Sim::step`
            // so we route through `summons` instead of
            // inserting entities directly.
            let n = count.max(1) as i32;
            for i in 0..n {
                let theta = std::f32::consts::TAU * (i as f32) / (n as f32);
                let pos = accepted.origin + Vec3::new(theta.cos(), 0.0, theta.sin()) * ring_radius;
                sinks.summons.push((pos, role, hp_mult));
            }
        }
        // ── Friendly support kinds ─────────────────────────────
        AbilityKind::HealTarget { amount } => {
            // Submit already validated alive / range / LOS, so
            // a missing target_entity here means the cast
            // shouldn't have made it past submit — silently
            // no-op rather than panic.
            let (Some(target), Some(target_net), Some(tpos)) = (
                accepted.target_entity,
                accepted.target_net_id,
                accepted.target_position,
            ) else {
                return;
            };
            sinks.player_heals.push((target, amount));
            sinks.events.push(WorldEvent::Heal {
                caster: accepted.caster,
                target: target_net,
                amount,
                over_time: false,
                position: tpos.to_array(),
            });
        }
        AbilityKind::HealOverTimeTarget { apply_buff } => {
            let Some(target) = accepted.target_entity else {
                return;
            };
            // The buff system keeps its own tick clock — we
            // just refresh / apply at the registry's default
            // duration (tooltip says 10 s, registry agrees).
            if let Ok(mut stack) = world.get::<&mut super::effect::EffectStack>(target) {
                stack.apply(
                    apply_buff,
                    None,
                    accepted.caster_entity,
                    accepted.ability_id,
                    super::meters::ATTACKER_KIND_OTHER,
                );
            }
        }
    }

    // ── Generic kinematic side-effect pass ────────────────────
    //
    // Any ability that authors an `AbilityEffect::SetPlayerAction`
    // entry whose `action` is server-driven (Roll, Attack)
    // stamps the matching kinematic state on the caster here.
    // This is the server's counterpart to the client-side
    // `ability_runtime::execute_ability` walk: same data
    // (`ability.effects`), same selectors (`SetPlayerAction`),
    // each side runs the parts of the effect it owns. Adding a
    // new pose-locking ability (heavy attack, parry, leap…)
    // means authoring a `SetPlayerAction` entry in the registry
    // — no per-ability dispatch arm or id-equality check.
    //
    // Locomotion / cast-flavoured `PlayerAction` variants
    // (None, Walk, Run, JumpAir, JumpLand, Cast) are not
    // server-kinematic actions: the kinematic enum
    // (`kinematic::action::*`) only encodes Roll and Attack,
    // because those are the actions where the server *drives*
    // motion. Everything else is animation state owned by the
    // client's locomotion picker. We deliberately ignore them
    // here.
    apply_kinematic_side_effects(world, ability, &accepted, tick);
}

/// Walk `ability.effects` and apply any kinematic side-effect
/// declared by a `SetPlayerAction` entry to the caster's
/// `ServerPlayer.k`. Keeps the dispatch arms focused on damage
/// primitives by lifting "lock the caster's pose for N seconds"
/// out into a single shared pass.
fn apply_kinematic_side_effects(
    world: &mut hecs::World,
    ability: &rift_game::abilities::Ability,
    accepted: &AcceptedCast,
    tick: NetTick,
) {
    use rift_game::abilities::AbilityEffect;
    use rift_game::components::PlayerAction;

    let Some(entity) = accepted.caster_entity else {
        return;
    };
    for effect in ability.effects {
        let AbilityEffect::SetPlayerAction { action, .. } = effect else {
            continue;
        };
        let Ok(mut p) = world.get::<&mut ServerPlayer>(entity) else {
            return;
        };
        match action {
            PlayerAction::Roll => {
                rift_game::kinematic::start_roll(&mut p.k, accepted.aim);
                // Stamp the action-start tick so snapshot
                // pipeline can carry it to the local client.
                // Without this the client's local timer
                // drifts ~RTT/2 ahead of the server's and
                // every subsequent snapshot snaps the
                // predicted position back into the still-
                // rolling server pose.
                p.action_start = tick;
            }
            PlayerAction::Attack => {
                rift_game::kinematic::start_attack(&mut p.k, accepted.aim);
                p.action_start = tick;
            }
            // Locomotion / cast poses: animation-only state
            // owned by the client. The server doesn't drive
            // motion for these, so there's nothing to stamp.
            _ => {}
        }
    }
}

/// Free-cast helper for proc-driven ability casts (Mirrorglass
/// Amulet's OnHit `CastAbility` pool, future on-kill/on-dodge
/// triggers, …). Bypasses the player's cooldown table, loadout
/// gate, and essence cost — the proc itself already paid the
/// "cost" by rolling — but otherwise reuses the standard
/// player-cast pipeline so the spawned effect is fully
/// replicated, fully scaled by gear, and visible to every
/// client through the same `WorldEvent` traffic as a manual
/// cast.
///
/// `caster` is the player ECS entity whose proc fired; this is
/// the player who gets the damage / meter attribution for the
/// resulting effect. `position` is the trigger-time anchor
/// (enemy hit position, player dodge position, low-HP latch
/// position) — used both to direct the cast (aim from the
/// caster toward `position` when far enough away) and as the
/// fallback spawn origin for any placement-driven ability.
///
/// No-ops silently when the caster is missing, dead, or the
/// ability id is unknown — proc dispatch shouldn't error out
/// the per-tick step.
pub fn dispatch_proc_cast(
    world: &mut hecs::World,
    caster: Entity,
    request: super::procs::ProcCastRequest,
    sinks: &mut DispatchSinks<'_>,
    tick: NetTick,
) {
    let ability = match rift_game::abilities::lookup_by_id(request.ability) {
        Some(a) => a,
        None => return,
    };
    let wire_id = ability.wire_id;
    // Snapshot all caster state in one immutable borrow so the
    // dispatch call below can re-borrow `world` mutably.
    let (
        body,
        net_id,
        aim_yaw,
        dmg_scalar,
        crit_chance,
        crit_damage,
        ability_mult,
        affix_dmg,
        range_mult,
        extra_projectiles,
        transform,
    ) = {
        let Ok(p) = world.get::<&ServerPlayer>(caster) else {
            return;
        };
        if p.hp <= 0.0 {
            return;
        }
        (
            p.k.position,
            p.net_id,
            p.k.aim_yaw,
            p.damage_scalar(),
            p.stats.crit_chance,
            p.stats.crit_damage,
            p.stats.ability_damage_mult(ability),
            p.ability_mods.damage_for(ability.id),
            p.stats.range.max(0.1),
            p.ability_mods.extra_projectiles_for(ability.id),
            p.ability_mods.transform_for(ability.id),
        )
    };

    // Aim selection — prefer pointing the cast at the proc
    // trigger position when it's meaningfully separated from
    // the caster (OnHit: enemy_pos); fall back to the
    // caster's facing yaw for self-anchored procs (OnDodge,
    // OnLowHealth).
    let mut delta = request.position - body;
    delta.y = 0.0;
    let aim = if delta.length_squared() > 0.25 {
        delta.normalize_or_zero()
    } else {
        Vec3::new(aim_yaw.sin(), 0.0, aim_yaw.cos())
    };
    let aim = if aim.length_squared() < 1.0e-4 {
        Vec3::Z
    } else {
        aim
    };
    let spawn_origin = body + Vec3::Y * 1.25 + aim * 0.25;
    let placed_target = Some(request.position);

    // Wire a minimal `WorldEvent::AbilityCast` so clients
    // play the standard cast SFX / animation cue. Without
    // this the proc-cast would silently spawn effects with
    // no audio/visual "tell" for the player.
    sinks.events.push(WorldEvent::AbilityCast {
        caster: net_id,
        ability: wire_id.raw() as u16,
        origin: spawn_origin.to_array(),
        dir: [aim.x, aim.z],
        target: Some(request.position.to_array()),
        start_tick: tick,
    });

    let accepted = AcceptedCast {
        caster: net_id,
        attacker_kind: super::meters::ATTACKER_KIND_OTHER,
        caster_entity: Some(caster),
        ability_id: wire_id,
        origin: body,
        spawn_origin,
        aim,
        placed_target,
        damage_scalar: dmg_scalar * ability_mult * affix_dmg,
        crit_chance,
        crit_damage,
        team: request.team,
        param_a: 0.0,
        target_entity: None,
        target_net_id: None,
        target_position: None,
        extra_projectiles,
        range_mult,
        transform,
        is_proc: true,
    };
    dispatch(world, accepted, sinks, tick);
}
