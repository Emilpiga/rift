//! Server-side status-effect stack and tick.
//!
//! An [`EffectStack`] component lives on every entity that can
//! carry buffs / debuffs (today: players + enemies). Each active
//! effect carries its own remaining-duration and DoT/HoT tick
//! clock; identical re-applications refresh duration rather than
//! stack count. The stack is queried by:
//!  - `enemy::tick_ai` — to apply [`EffectKind::MoveSpeedMult`]
//!    when picking a step distance.
//!  - `projectile` / `channel` — to multiply outgoing damage by
//!    [`EffectKind::IncomingDamageMult`].
//!  - `snapshot::build` — to emit the per-entity `effects` list.

use hecs::Entity;
use rift_game::abilities::AbilityWireId;
use rift_game::effects::{lookup, EffectDef, EffectKind};
use rift_net::{
    messages::{ActiveEffect, WorldEvent},
    NetId,
};

use super::enemies::ServerEnemy;
use super::player::ServerPlayer;

/// One running instance of a debuff on a target.
#[derive(Clone, Copy, Debug)]
pub struct EffectInstance {
    pub id: u8,
    pub remaining: f32,
    /// Total duration this entry was applied with. Used as
    /// the denominator for the HUD's radial duration ring,
    /// and for the snapshot's [`ActiveEffect::duration`]
    /// field. Stays fixed across refreshes — a refresh that
    /// tops up `remaining` to the same `applied_duration`
    /// keeps the ring's full-circle reading consistent.
    pub applied_duration: f32,
    /// Time since last DoT tick fired (seconds). Only meaningful for
    /// debuffs whose def carries a `DamageOverTime` effect.
    pub dot_acc: f32,
    /// Entity that applied this effect. `None` for system /
    /// environmental sources. Used to credit DoT damage and
    /// HoT healing back to the caster in the combat meters.
    /// Refreshes stomp this with the latest applier so the
    /// most recent caster owns the running attribution
    /// (mirrors how most damage meters resolve overlap).
    pub caster: Option<Entity>,
    /// Wire ability id of the source that produced the effect.
    /// Used as the meter's per-ability bucket when DoT / HoT
    /// ticks credit the caster. `255` (`ABILITY_ID_OTHER`) when
    /// the source can't be attributed to a specific ability.
    pub ability_id: AbilityWireId,
    /// Attacker kind for the TAKEN-tab breakdown when this
    /// instance is sitting on a player and ticks damage.
    /// `MonsterRole::to_wire_byte()` for known enemy casters,
    /// or [`super::meters::ATTACKER_KIND_OTHER`] when the
    /// applier wasn't an enemy (e.g. self-applied debuff,
    /// environmental). Refreshes overwrite this alongside
    /// `caster` / `ability_id`.
    pub attacker_kind: u8,
}

/// Per-entity stack of active debuffs. Tiny by construction —
/// distinct ids only, refreshed in place on re-application.
#[derive(Clone, Debug, Default)]
pub struct EffectStack {
    pub active: Vec<EffectInstance>,
}

impl EffectStack {
    pub fn apply(
        &mut self,
        debuff_id: u8,
        duration_override: Option<f32>,
        caster: Option<Entity>,
        ability_id: AbilityWireId,
        attacker_kind: u8,
    ) {
        let Some(def) = lookup(debuff_id) else { return };
        let dur = duration_override.unwrap_or(def.default_duration);
        if let Some(existing) = self.active.iter_mut().find(|d| d.id == debuff_id) {
            existing.remaining = existing.remaining.max(dur);
            existing.applied_duration = existing.applied_duration.max(dur);
            // Latest applier owns the attribution. Refreshing
            // a DoT mid-flight transfers credit for the rest
            // of the duration, which is the convention the
            // common WoW-style meters use.
            existing.caster = caster;
            existing.ability_id = ability_id;
            existing.attacker_kind = attacker_kind;
        } else {
            self.active.push(EffectInstance {
                id: debuff_id,
                remaining: dur,
                applied_duration: dur,
                dot_acc: 0.0,
                caster,
                ability_id,
                attacker_kind,
            });
        }
    }

    /// Snapshot view used by `sim::snapshot::build` to populate
    /// [`EntitySnapshot::effects`]. One [`ActiveEffect`] row per
    /// active entry.
    pub fn to_snapshot(&self) -> Vec<ActiveEffect> {
        self.active
            .iter()
            .map(|d| ActiveEffect {
                id: d.id,
                remaining: d.remaining,
                duration: d.applied_duration.max(0.001),
            })
            .collect()
    }

    /// Look up the strongest [`EffectKind::IncomingDamageMult`]
    /// across all active debuffs. Multiple muls multiply.
    pub fn incoming_damage_mult(&self) -> f32 {
        let mut m = 1.0;
        for d in &self.active {
            if let Some(def) = lookup(d.id) {
                for e in def.effects {
                    if let EffectKind::IncomingDamageMult(x) = e {
                        m *= x;
                    }
                }
            }
        }
        m
    }

    /// Aggregate movement-speed multiplier (multiplicative).
    pub fn move_speed_mult(&self) -> f32 {
        let mut m = 1.0;
        for d in &self.active {
            if let Some(def) = lookup(d.id) {
                for e in def.effects {
                    if let EffectKind::MoveSpeedMult(x) = e {
                        m *= x;
                    }
                }
            }
        }
        m
    }

    /// Aggregate healing-received multiplier across all active
    /// effects. `<1.0` reduces incoming heals (Necrotic);
    /// `>1.0` would amplify them. Heal sites multiply the
    /// queued amount by this before applying to `hp`.
    pub fn healing_received_mult(&self) -> f32 {
        let mut m = 1.0;
        for d in &self.active {
            if let Some(def) = lookup(d.id) {
                for e in def.effects {
                    if let EffectKind::HealingReceivedMult(x) = e {
                        m *= x;
                    }
                }
            }
        }
        m
    }
}

/// One queued DoT damage application from `tick`. Used for both
/// enemy and player targets — the consumer routes by entity kind.
struct DotHit {
    target: Entity,
    target_net_id: NetId,
    target_pos: glam::Vec3,
    damage: f32,
    /// Original applier of the DoT (carried by [`EffectInstance`]).
    /// Used to credit the per-tick damage to the caster's combat
    /// meter when the target is an enemy.
    caster: Option<Entity>,
    /// Wire ability id of the source. Bucketed in the meter's
    /// per-ability breakdown.
    ability_id: AbilityWireId,
    /// Attacker kind for the TAKEN-tab breakdown (player
    /// targets only). Carried over from
    /// [`EffectInstance::attacker_kind`].
    attacker_kind: u8,
}

/// One queued heal-over-time tick from `tick`. Player-only (no
/// enemy heal source today). Applied in-place inside the player
/// walk, then mirrored as a [`WorldEvent::Heal`] so clients can
/// spawn floating green numbers + sustained aura visuals.
struct HotHit {
    target: Entity,
    target_net_id: NetId,
    target_pos: glam::Vec3,
    amount: f32,
    /// Original applier of the HoT. Used to credit the per-
    /// tick healing to the caster's meter row.
    caster: Option<Entity>,
    ability_id: AbilityWireId,
}

/// Decay every active debuff, fire any due DoT ticks, drop expired
/// entries. Enemy DoT damage is applied in-place and emits
/// `WorldEvent::Damage` into `ctx.events`; player DoT damage is
/// returned as `(player_entity, damage)` rows for the caller to
/// route through `apply_player_damage` (same path direct hits
/// take).
pub fn tick(
    world: &mut hecs::World,
    ctx: &mut super::combat_ctx::CombatCtx<'_>,
    dt: f32,
) -> Vec<super::combat_ctx::PlayerHit> {
    let mut hits: Vec<DotHit> = Vec::new();
    let mut dead: Vec<(Entity, NetId, glam::Vec3)> = Vec::new();
    for (entity, (en, stack)) in world.query_mut::<(&mut ServerEnemy, &mut EffectStack)>() {
        if en.is_dying() {
            continue;
        }
        let pos = en.k.position;
        let net_id = en.net_id;
        // Walk in reverse so swap_remove is safe.
        let mut i = stack.active.len();
        while i > 0 {
            i -= 1;
            stack.active[i].remaining -= dt;
            stack.active[i].dot_acc += dt;
            let id = stack.active[i].id;
            if let Some(def) = lookup(id) {
                accumulate_dot(&mut stack.active[i], def, &mut hits, entity, net_id, pos);
            }
            if stack.active[i].remaining <= 0.0 {
                stack.active.swap_remove(i);
            }
        }
        // Apply queued DoT to the enemy in-place — we still hold
        // its `&mut`. We can't share the projectile.rs damage
        // helper here (the borrow extends through the loop body),
        // so just inline the simple case.
        for hit in hits.drain(..).filter(|h| h.target == entity) {
            en.hp = (en.hp - hit.damage).max(0.0);
            ctx.events.push(WorldEvent::Damage {
                target: hit.target_net_id,
                amount: hit.damage,
                crit: false,
                position: hit.target_pos.to_array(),
            });
            // Credit the per-tick damage to the original
            // caster's combat-meter row + per-ability bucket.
            // Anonymous DoTs (no caster recorded at apply
            // time) are skipped — they'd have nowhere to
            // land, and the wire event still fires.
            if let Some(caster) = hit.caster {
                ctx.meter_events
                    .push(super::combat_ctx::MeterEvent::DamageDealt {
                        attacker: caster,
                        ability_id: hit.ability_id,
                        amount: hit.damage,
                    });
            }
            if en.hp <= 0.0 {
                dead.push((hit.target, hit.target_net_id, glam::Vec3::ZERO));
                break;
            }
        }
    }
    super::loot::finalise_kills(world, ctx, dead);

    // Mirror pass for players. Returns DoT damage rows so the
    // caller pipes them through `apply_player_damage` — same
    // path enemy-projectile rows go through, so death /
    // respawn / party-wipe stays single-source.
    let mut player_damage: Vec<super::combat_ctx::PlayerHit> = Vec::new();
    let mut heals: Vec<HotHit> = Vec::new();
    for (entity, (p, stack)) in world.query_mut::<(&mut ServerPlayer, &mut EffectStack)>() {
        if p.hp <= 0.0 {
            continue;
        }
        let pos = p.k.position;
        let net_id = p.net_id;
        let mut i = stack.active.len();
        while i > 0 {
            i -= 1;
            stack.active[i].remaining -= dt;
            stack.active[i].dot_acc += dt;
            let id = stack.active[i].id;
            if let Some(def) = lookup(id) {
                accumulate_dot(&mut stack.active[i], def, &mut hits, entity, net_id, pos);
                accumulate_hot(&mut stack.active[i], def, &mut heals, entity, net_id, pos);
            }
            if stack.active[i].remaining <= 0.0 {
                stack.active.swap_remove(i);
            }
        }
        for hit in hits.drain(..).filter(|h| h.target == entity) {
            player_damage.push(super::combat_ctx::PlayerHit {
                target: entity,
                attacker_kind: hit.attacker_kind,
                ability_id: hit.ability_id,
                amount: hit.damage,
            });
        }
        // Apply HoT in-place — we already hold `&mut p`.
        // Emitting the wire event from inside the query borrow
        // is fine because `ctx.events` doesn't alias the world.
        // Heals are scaled by the target's healing-received
        // multiplier (Necrotic ⇒ 0.5×) and by the
        // `Stat::HealingReceived` gear bonus so HoT ticks
        // honour both the same debuff direct heals do and any
        // healing-amp affixes the target has rolled. Captured
        // before the drain so the pending self-tick this frame
        // can't shrink itself by its own application.
        let heal_mult = stack.healing_received_mult() * (1.0 + p.stats.healing_received).max(0.0);
        for heal in heals.drain(..).filter(|h| h.target == entity) {
            let amount = heal.amount * heal_mult;
            // Capture pre-heal HP so the meter can credit only
            // the *effective* (non-overheal) portion to the
            // caster's HPS row, matching how direct heals are
            // booked in `Sim::handle_cast_request`.
            let before = p.hp;
            p.hp = (p.hp + amount).min(p.hp_max);
            let effective = p.hp - before;
            ctx.events.push(WorldEvent::Heal {
                // No caster tracking on debuff instances yet —
                // self-attribute the tick to the target. Visuals
                // and combat text don't depend on caster identity.
                caster: heal.target_net_id,
                target: heal.target_net_id,
                amount,
                over_time: true,
                position: heal.target_pos.to_array(),
            });
            // Credit non-overheal portion to the caster.
            // Anonymous HoTs are skipped (no row to write).
            if effective > 0.0 {
                if let Some(caster) = heal.caster {
                    ctx.meter_events
                        .push(super::combat_ctx::MeterEvent::HealingDone {
                            caster,
                            ability_id: heal.ability_id,
                            amount: effective,
                        });
                }
            }
        }
    }
    player_damage
}

/// Pop any due DoT ticks off `instance.dot_acc` and queue damage
/// against the target.
fn accumulate_dot(
    instance: &mut EffectInstance,
    def: &EffectDef,
    hits: &mut Vec<DotHit>,
    target: Entity,
    target_net_id: NetId,
    target_pos: glam::Vec3,
) {
    for effect in def.effects {
        if let EffectKind::DamageOverTime { dps, interval } = effect {
            while instance.dot_acc >= *interval {
                instance.dot_acc -= interval;
                hits.push(DotHit {
                    target,
                    target_net_id,
                    target_pos,
                    damage: dps * interval,
                    caster: instance.caster,
                    ability_id: instance.ability_id,
                    attacker_kind: instance.attacker_kind,
                });
            }
        }
    }
}

/// Twin of [`accumulate_dot`] for [`EffectKind::HealOverTime`].
/// Shares `instance.dot_acc` because no def carries both effect
/// kinds today (DoT and HoT are mutually exclusive on a single
/// def by convention). If that ever changes, split the clocks.
fn accumulate_hot(
    instance: &mut EffectInstance,
    def: &EffectDef,
    heals: &mut Vec<HotHit>,
    target: Entity,
    target_net_id: NetId,
    target_pos: glam::Vec3,
) {
    for effect in def.effects {
        if let EffectKind::HealOverTime { hps, interval } = effect {
            while instance.dot_acc >= *interval {
                instance.dot_acc -= interval;
                heals.push(HotHit {
                    target,
                    target_net_id,
                    target_pos,
                    amount: hps * interval,
                    caster: instance.caster,
                    ability_id: instance.ability_id,
                });
            }
        }
    }
}
