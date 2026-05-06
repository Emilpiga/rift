//! Server-side debuff stack and tick.
//!
//! A [`DebuffStack`] component lives on every entity that can be
//! debuffed (today: enemies). Each active debuff carries its own
//! remaining-duration and DoT tick clock; identical re-applications
//! refresh duration rather than stack count. The stack is queried
//! by:
//!  - `enemy::tick_ai` — to apply [`DebuffEffect::MoveSpeedMult`]
//!    when picking a step distance.
//!  - `projectile` / `channel` — to multiply outgoing damage by
//!    [`DebuffEffect::IncomingDamageMult`].
//!  - `snapshot::build` — to emit the per-enemy `debuffs` bitmask.

use hecs::Entity;
use rift_game::debuffs::{self, DebuffDef, DebuffEffect};
use rift_net::{messages::WorldEvent, NetId};

use super::enemy::ServerEnemy;

/// One running instance of a debuff on a target.
#[derive(Clone, Copy, Debug)]
pub struct ActiveDebuff {
    pub id: u8,
    pub remaining: f32,
    /// Time since last DoT tick fired (seconds). Only meaningful for
    /// debuffs whose def carries a `DamageOverTime` effect.
    pub dot_acc: f32,
}

/// Per-entity stack of active debuffs. Tiny by construction —
/// distinct ids only, refreshed in place on re-application.
#[derive(Clone, Debug, Default)]
pub struct DebuffStack {
    pub active: Vec<ActiveDebuff>,
}

impl DebuffStack {
    pub fn apply(&mut self, debuff_id: u8, duration_override: Option<f32>) {
        let Some(def) = debuffs::lookup(debuff_id) else { return };
        let dur = duration_override.unwrap_or(def.default_duration);
        if let Some(existing) = self.active.iter_mut().find(|d| d.id == debuff_id) {
            existing.remaining = existing.remaining.max(dur);
        } else {
            self.active.push(ActiveDebuff {
                id: debuff_id,
                remaining: dur,
                dot_acc: 0.0,
            });
        }
    }

    /// Encode the active set into a `u32` bitmask for the snapshot.
    pub fn bitmask(&self) -> u32 {
        let mut m = 0u32;
        for d in &self.active {
            m |= debuffs::bit_for(d.id);
        }
        m
    }

    /// Look up the strongest [`DebuffEffect::IncomingDamageMult`]
    /// across all active debuffs. Multiple muls multiply.
    pub fn incoming_damage_mult(&self) -> f32 {
        let mut m = 1.0;
        for d in &self.active {
            if let Some(def) = debuffs::lookup(d.id) {
                for e in def.effects {
                    if let DebuffEffect::IncomingDamageMult(x) = e {
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
            if let Some(def) = debuffs::lookup(d.id) {
                for e in def.effects {
                    if let DebuffEffect::MoveSpeedMult(x) = e {
                        m *= x;
                    }
                }
            }
        }
        m
    }
}

/// One queued DoT damage application from `tick`.
struct DotHit {
    enemy: Entity,
    enemy_net_id: NetId,
    enemy_pos: glam::Vec3,
    damage: f32,
}

/// Decay every active debuff, fire any due DoT ticks, drop expired
/// entries. Damage events are pushed into `ctx.events` as
/// `WorldEvent::Damage`. Kills are finalised through
/// [`super::loot::finalise_kills`] so DoT-driven deaths drop loot
/// the same way direct hits do.
pub fn tick(world: &mut hecs::World, ctx: &mut super::loot::DeathCtx<'_>, dt: f32) {
    let mut hits: Vec<DotHit> = Vec::new();
    let mut dead: Vec<(Entity, NetId)> = Vec::new();
    for (entity, (en, stack)) in world.query_mut::<(&mut ServerEnemy, &mut DebuffStack)>() {
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
            if let Some(def) = debuffs::lookup(id) {
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
        for hit in hits.drain(..).filter(|h| h.enemy == entity) {
            en.hp = (en.hp - hit.damage).max(0.0);
            ctx.events.push(WorldEvent::Damage {
                target: hit.enemy_net_id,
                amount: hit.damage,
                crit: false,
                position: hit.enemy_pos.to_array(),
            });
            if en.hp <= 0.0 {
                dead.push((hit.enemy, hit.enemy_net_id));
                break;
            }
        }
    }
    super::loot::finalise_kills(world, ctx, dead);
}

/// Pop any due DoT ticks off `instance.dot_acc` and queue damage
/// against the target.
fn accumulate_dot(
    instance: &mut ActiveDebuff,
    def: &DebuffDef,
    hits: &mut Vec<DotHit>,
    enemy: Entity,
    enemy_net_id: NetId,
    enemy_pos: glam::Vec3,
) {
    for effect in def.effects {
        if let DebuffEffect::DamageOverTime { dps, interval } = effect {
            while instance.dot_acc >= *interval {
                instance.dot_acc -= interval;
                hits.push(DotHit {
                    enemy,
                    enemy_net_id,
                    enemy_pos,
                    damage: dps * interval,
                });
            }
        }
    }
}
