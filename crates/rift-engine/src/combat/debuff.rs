//! Generic timed debuff component + system.
//!
//! `Debuffs` is an ECS component holding any number of active
//! [`Debuff`]s on an entity (typically an enemy).  Each debuff carries
//! its own runtime numbers (DoT, taken-damage multiplier, slow), an
//! attached visual emitter slot, and a remaining timer.
//!
//! `debuff_tick_system` runs once per frame to:
//!   1. tick timers,
//!   2. fire DoT damage (returns `(pos, damage)` events for floating-text/HUD),
//!   3. keep the visual emitter glued to the entity's current position,
//!   4. retire expired debuffs (and their emitters).
//!
//! Damage application elsewhere should pass through
//! [`Debuffs::damage_taken_mult`] so things like Mark-for-Death stack
//! cleanly with raw incoming damage from arrows, AoE zones, etc.

use glam::Vec3;
use hecs::World;

use crate::ecs::components::{Enemy, Health, Transform};
use crate::renderer::particles::{Emitter, EmitterConfig};
use crate::renderer::Renderer;

/// Identity of a debuff — used for stacking / refresh rules.  When an
/// entity already has a debuff with the same `kind`, applying a new
/// one of the same kind refreshes its timer rather than stacking
/// multiplicatively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebuffKind {
    MarkForDeath,
    Poison,
    Burn,
    Bleed,
    Slow,
    /// Free-form id for game-specific debuffs.
    Custom(u16),
}

/// Visual representation of a debuff while it is active.
#[derive(Clone, Copy, Debug)]
pub enum DebuffVisual {
    /// No visual (gameplay-only debuff).
    None,
    /// Continuous particle emitter glued to the entity's position.
    /// `rgb` tints the emitter; `rate` is the spawn rate per second.
    Aura {
        rgb: [f32; 3],
        rate: f32,
        size: f32,
    },
}

/// One active debuff on an entity.
#[derive(Clone, Debug)]
pub struct Debuff {
    pub kind: DebuffKind,
    /// Seconds until the debuff expires.
    pub remaining: f32,
    /// Original duration, for HUD progress bars.
    pub duration: f32,

    /// DoT damage per tick (0 = no DoT).
    pub damage_per_tick: f32,
    /// Seconds between DoT ticks.
    pub tick_interval: f32,
    /// Internal accumulator.
    pub tick_timer: f32,

    /// Multiplier applied to *incoming* damage on this entity (1.0 = no change).
    pub damage_taken_mult: f32,
    /// Multiplier applied to the entity's movement speed (1.0 = no change).
    pub move_speed_mult: f32,

    pub visual: DebuffVisual,
    /// Renderer emitter slot, populated lazily on first tick.
    pub(crate) emitter_slot: Option<usize>,
}

impl Debuff {
    /// Mark for Death: enemies in the AoE take +25% damage for `duration` seconds.
    pub fn mark_for_death(duration: f32) -> Self {
        Self {
            kind: DebuffKind::MarkForDeath,
            remaining: duration,
            duration,
            damage_per_tick: 0.0,
            tick_interval: 0.0,
            tick_timer: 0.0,
            damage_taken_mult: 1.25,
            move_speed_mult: 1.0,
            visual: DebuffVisual::Aura {
                rgb: [1.0, 0.15, 0.15],
                rate: 28.0,
                size: 0.45,
            },
            emitter_slot: None,
        }
    }

    /// Poison: ticks `dps * tick_interval` damage every `tick_interval` seconds.
    pub fn poison(dps: f32, duration: f32) -> Self {
        let tick_interval = 0.5;
        Self {
            kind: DebuffKind::Poison,
            remaining: duration,
            duration,
            damage_per_tick: dps * tick_interval,
            tick_interval,
            tick_timer: 0.0,
            damage_taken_mult: 1.0,
            move_speed_mult: 1.0,
            visual: DebuffVisual::Aura {
                rgb: [0.30, 1.00, 0.25],
                rate: 22.0,
                size: 0.40,
            },
            emitter_slot: None,
        }
    }

    /// Burn: bigger DoT, shorter duration, hot palette.
    pub fn burn(dps: f32, duration: f32) -> Self {
        let tick_interval = 0.4;
        Self {
            kind: DebuffKind::Burn,
            remaining: duration,
            duration,
            damage_per_tick: dps * tick_interval,
            tick_interval,
            tick_timer: 0.0,
            damage_taken_mult: 1.0,
            move_speed_mult: 1.0,
            visual: DebuffVisual::Aura {
                rgb: [1.00, 0.55, 0.10],
                rate: 32.0,
                size: 0.55,
            },
            emitter_slot: None,
        }
    }

    /// Slow: scales movement speed by `mult` (e.g. 0.5 = halved).
    pub fn slow(mult: f32, duration: f32) -> Self {
        Self {
            kind: DebuffKind::Slow,
            remaining: duration,
            duration,
            damage_per_tick: 0.0,
            tick_interval: 0.0,
            tick_timer: 0.0,
            damage_taken_mult: 1.0,
            move_speed_mult: mult,
            visual: DebuffVisual::Aura {
                rgb: [0.55, 0.75, 1.00],
                rate: 16.0,
                size: 0.40,
            },
            emitter_slot: None,
        }
    }
}

/// Component: all active debuffs on an entity.
#[derive(Default, Clone, Debug)]
pub struct Debuffs {
    pub list: Vec<Debuff>,
}

impl Debuffs {
    /// Apply (or refresh) a debuff.  Same-kind debuffs replace the
    /// existing one when the new one has a longer remaining time —
    /// otherwise the existing entry's timer is bumped to the new
    /// duration (classic "refresh") to avoid letting weak applications
    /// reset a stronger active effect.
    pub fn apply(&mut self, mut new: Debuff) {
        if let Some(existing) = self.list.iter_mut().find(|d| d.kind == new.kind) {
            // Preserve emitter so the visual doesn't flicker on refresh.
            new.emitter_slot = existing.emitter_slot;
            if new.remaining > existing.remaining {
                *existing = new;
            } else {
                existing.remaining = new.remaining.max(existing.remaining);
                existing.duration = new.duration.max(existing.duration);
                existing.damage_taken_mult = existing.damage_taken_mult.max(new.damage_taken_mult);
                existing.move_speed_mult = existing.move_speed_mult.min(new.move_speed_mult);
            }
            return;
        }
        self.list.push(new);
    }

    /// Combined damage-taken multiplier across all active debuffs.
    pub fn damage_taken_mult(&self) -> f32 {
        let mut m = 1.0;
        for d in &self.list {
            // Take the *highest* damage-taken-mult rather than multiplying so
            // dual-marking doesn't compound to silly numbers.
            if d.damage_taken_mult > m {
                m = d.damage_taken_mult;
            }
        }
        m
    }

    /// Combined movement-speed multiplier (lowest wins).
    pub fn move_speed_mult(&self) -> f32 {
        let mut m = 1.0_f32;
        for d in &self.list {
            if d.move_speed_mult < m {
                m = d.move_speed_mult;
            }
        }
        m
    }

    pub fn has(&self, kind: DebuffKind) -> bool {
        self.list.iter().any(|d| d.kind == kind)
    }
}

/// Apply raw damage to an entity, scaled by its `Debuffs`'
/// damage-taken multiplier (if any).  Returns the damage actually
/// applied.  Centralising the lookup here keeps every damage source
/// (projectiles, AoE zones, contact, instant area) honoring debuffs.
pub fn apply_damage(world: &World, entity: hecs::Entity, raw: f32) -> f32 {
    let mult = world
        .get::<&Debuffs>(entity)
        .map(|d| d.damage_taken_mult())
        .unwrap_or(1.0);
    let dmg = raw * mult;
    if let Ok(mut h) = world.get::<&mut Health>(entity) {
        h.current -= dmg;
    }
    dmg
}

/// Per-frame system: tick DoTs, drive visual emitters, retire expired
/// debuffs.  Returns `(world_position, damage)` events for the
/// floating-damage-text system, mirroring the projectile tick API.
pub fn debuff_tick_system(
    world: &mut World,
    renderer: &mut Renderer,
    dt: f32,
) -> Vec<(Vec3, f32)> {
    let mut damage_events: Vec<(Vec3, f32)> = Vec::new();

    // Snapshot positions because we need a separate &mut to renderer
    // and to spawn DoT damage through `apply_damage` later.
    let snapshots: Vec<(hecs::Entity, Vec3)> = world
        .query::<(&Transform, &Enemy)>()
        .iter()
        .map(|(e, (t, _))| (e, t.position))
        .collect();

    // First pass: tick timers + DoT damage.  Collect dotted entities
    // and the per-debuff dot to apply (so we can do the apply_damage
    // call separately, since it needs a `&mut Health` and we can't
    // have both `&mut Debuffs` and `&mut Health` simultaneously on the
    // same query.)
    let mut dot_apply: Vec<(hecs::Entity, f32, Vec3)> = Vec::new();
    let mut dirty: Vec<hecs::Entity> = Vec::new();

    for (entity, position) in &snapshots {
        let Ok(mut debuffs) = world.get::<&mut Debuffs>(*entity) else { continue };
        let mut any_changes = false;
        for d in debuffs.list.iter_mut() {
            d.remaining -= dt;
            if d.tick_interval > 0.0 && d.damage_per_tick > 0.0 {
                d.tick_timer += dt;
                while d.tick_timer >= d.tick_interval && d.remaining > -dt {
                    d.tick_timer -= d.tick_interval;
                    dot_apply.push((*entity, d.damage_per_tick, *position));
                }
            }
            // Update emitter position to follow the enemy.
            if let DebuffVisual::Aura { .. } = d.visual {
                if let Some(slot) = d.emitter_slot {
                    if let Some(em) = renderer.particle_system.emitters.get_mut(slot) {
                        em.position = *position + Vec3::new(0.0, 1.0, 0.0);
                    }
                }
            }
            if d.remaining <= 0.0 {
                any_changes = true;
            }
        }
        if any_changes {
            dirty.push(*entity);
        }
    }

    // Apply DoTs through the centralised path so any *other* debuffs
    // (e.g. Mark for Death) amplify them.
    for (entity, raw, pos) in dot_apply {
        let dmg = apply_damage(world, entity, raw);
        damage_events.push((pos, dmg));
    }

    // Lazily spawn aura emitters for newly-applied debuffs.
    let to_spawn: Vec<(hecs::Entity, usize, [f32; 3], f32, f32, Vec3)> = {
        let mut v = Vec::new();
        for (entity, position) in &snapshots {
            let Ok(debuffs) = world.get::<&Debuffs>(*entity) else { continue };
            for (i, d) in debuffs.list.iter().enumerate() {
                if d.emitter_slot.is_some() { continue }
                if let DebuffVisual::Aura { rgb, rate, size } = d.visual {
                    v.push((*entity, i, rgb, rate, size, *position));
                }
            }
        }
        v
    };
    for (entity, idx, rgb, rate, size, pos) in to_spawn {
        let cfg = EmitterConfig::aura(rgb, rate, size);
        let emitter = Emitter::new(pos + Vec3::new(0.0, 1.0, 0.0), cfg);
        let slot = renderer.particle_system.add_emitter(emitter);
        if let Ok(mut debuffs) = world.get::<&mut Debuffs>(entity) {
            if let Some(d) = debuffs.list.get_mut(idx) {
                d.emitter_slot = Some(slot);
            }
        }
    }

    // Retire expired debuffs and deactivate their emitters.
    for entity in dirty {
        let Ok(mut debuffs) = world.get::<&mut Debuffs>(entity) else { continue };
        let mut i = 0;
        while i < debuffs.list.len() {
            if debuffs.list[i].remaining <= 0.0 {
                let d = debuffs.list.swap_remove(i);
                if let Some(slot) = d.emitter_slot {
                    renderer.particle_system.deactivate_emitter(slot);
                }
            } else {
                i += 1;
            }
        }
    }

    damage_events
}

/// On enemy death, deactivate any remaining debuff emitters so they
/// don't leak particles after the body despawns.  Call from the
/// despawn pipeline before removing the entity.
pub fn cleanup_debuff_visuals(world: &World, renderer: &mut Renderer, entity: hecs::Entity) {
    if let Ok(debuffs) = world.get::<&Debuffs>(entity) {
        for d in &debuffs.list {
            if let Some(slot) = d.emitter_slot {
                renderer.particle_system.deactivate_emitter(slot);
            }
        }
    }
}
