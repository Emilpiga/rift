//! Data-driven ability runtime.
//!
//! An `Ability` carries a slice of `AbilityEffect`s that are walked in
//! order by [`execute_ability`].  Each variant maps to one of a small
//! set of engine primitives (projectile spawn, AoE zone, player FSM
//! transition, instant area damage).  Ability authors edit only the
//! `effects` list — no engine code changes needed for new abilities.
//!
//! The escape hatch is [`AbilityEffect::Custom`], which takes an
//! `fn(&mut AbilityCtx)` for the rare ability that needs imperative
//! logic.

use glam::{Quat, Vec3};
use hecs::World;

use crate::animation::Animator;
use crate::combat::ability::Ability;
use crate::combat::debuff::{Debuff, Debuffs};
use crate::combat::projectile::Projectile;
use crate::combat::projectile_pool::{AoeZone, ProjectilePool};
use crate::combat::talent::{AbilityModifier, TalentEffect, TalentTree};
use crate::ecs::components::{
    AnimationSet, Enemy, Player, PlayerAction, SpellCast, SpellPhase, Transform, Velocity,
};
use crate::renderer::particles::{Emitter, EmitterConfig};
use crate::renderer::Renderer;

/// Where on the caster a projectile spawns relative to the body.
///
/// The shoulder offset matches the player's right-hand position when
/// raised for a bow draw / cast — projectiles look like they leave the
/// hand instead of the chest.
#[derive(Clone, Copy, Debug)]
pub struct SpawnOffset {
    pub forward: f32,
    pub up: f32,
    pub right: f32,
}

impl SpawnOffset {
    pub const HAND: Self = Self {
        forward: 0.55,
        up: 1.25,
        right: 0.30,
    };
    pub const ROOT: Self = Self {
        forward: 0.0,
        up: 0.0,
        right: 0.0,
    };
}

/// A small, named subset of the engine particle presets that abilities
/// are allowed to spawn.  Keeps `AbilityEffect` data-only (no
/// function pointers into the renderer crate's particle module).
#[derive(Clone, Copy, Debug)]
pub enum ParticlePreset {
    DodgePuff,
    /// Rain of arrows visual: orange streaks pouring down.
    RainOfArrows,
    /// Generic cast plume in a tinted color.
    Cast([f32; 3]),
}

impl ParticlePreset {
    fn into_emitter(self, position: Vec3) -> Emitter {
        match self {
            ParticlePreset::DodgePuff => Emitter::new(position, EmitterConfig::dodge_puff()),
            ParticlePreset::RainOfArrows => {
                Emitter::new(position, EmitterConfig::rain_of_arrows([1.0, 0.8, 0.3]))
            }
            ParticlePreset::Cast(rgb) => Emitter::new(position, EmitterConfig::hit_spark(rgb)),
        }
    }
}

/// How the player moves while a `SetPlayerAction` is held.
#[derive(Clone, Copy, Debug)]
pub enum ActionMovement {
    /// Constant velocity along the body's facing (transform forward).
    /// Used for Roll / dashes.
    Forward(f32),
    /// Velocity is locked to zero for the action's duration.
    Frozen,
    /// Movement isn't touched (use for upper-body-only effects).
    None,
}

/// One declarative effect of an ability.  Abilities are `&[AbilityEffect]`.
#[derive(Clone, Copy, Debug)]
pub enum AbilityEffect {
    /// Spawn `count` projectiles in a `spread` fan along the aim
    /// direction.  `damage_mult` multiplies the base damage passed
    /// into [`execute_ability`].
    SpawnProjectiles {
        count: u32,
        spread: f32,
        damage_mult: f32,
        /// Base pierce on top of any talent-derived bonus.
        pierce: u32,
        spawn_offset: SpawnOffset,
    },
    /// Place an AoE damage zone at the cast target (or, for instant
    /// abilities, at `origin + aim_dir * 5.0`).
    SpawnAoeZone {
        radius: f32,
        damage_mult: f32,
        duration: f32,
        tick_interval: f32,
        visual: Option<ParticlePreset>,
        /// Vertical offset applied to the visual emitter (e.g. rain
        /// of arrows spawns 5m above the ground).
        visual_y: f32,
    },
    /// Instant area damage at the cast point — single damage event,
    /// no zone, no ticks.  Used for Mark-for-Death style spells.
    InstantAreaDamage { radius: f32, damage_mult: f32 },
    /// Drive the player full-body action FSM.
    SetPlayerAction {
        action: PlayerAction,
        duration: f32,
        clip: &'static [&'static str],
        movement: ActionMovement,
        /// Cancel any active upper-body cast so the new clip reads cleanly.
        cancel_cast: bool,
        emitter: Option<ParticlePreset>,
    },
    /// Imperative escape hatch for one-off abilities that don't fit
    /// the data-driven model.  Receives a fully built context.
    Custom(fn(&mut AbilityCtx<'_>)),
    /// Apply a [`Debuff`] (Mark for Death, Poison, Burn, Slow, ...) to
    /// every enemy within `radius` of the cast target.  Use
    /// `radius = 0.0` for a single-target apply at the placed point.
    ///
    /// `debuff` is a factory because `Debuff` isn't `Copy` (it owns a
    /// `Vec` slot through `Debuffs`).  Authors pass a `fn` that
    /// constructs a fresh debuff — typically one of the named
    /// presets like `Debuff::poison`.
    ApplyDebuff {
        radius: f32,
        debuff: fn() -> Debuff,
    },
}

/// Resources shared by every effect arm during one `execute_ability` call.
///
/// `target` is `Some` for placed (reticle-confirmed) casts and `None`
/// for instant casts; effects that consume a target fall back to
/// `origin + aim_dir * 5.0` when it's `None`.
pub struct AbilityCtx<'a> {
    pub origin: Vec3,
    pub aim_dir: Vec3,
    pub target: Option<Vec3>,
    pub damage: f32,
    pub talents: Option<&'a TalentTree>,
    pub world: &'a mut World,
    pub renderer: &'a mut Renderer,
    pub projectiles: &'a mut ProjectilePool,
}

impl<'a> AbilityCtx<'a> {
    /// World position to use for placed effects (zone, instant area).
    pub fn placed_position(&self) -> Vec3 {
        self.target.unwrap_or(self.origin + self.aim_dir * 5.0)
    }
}

/// Walk every effect in `ability.effects` and execute it.
pub fn execute_ability(ability: &Ability, ctx: &mut AbilityCtx<'_>) {
    for effect in ability.effects {
        match effect {
            AbilityEffect::SpawnProjectiles {
                count,
                spread,
                damage_mult,
                pierce,
                spawn_offset,
            } => {
                spawn_projectile_volley(
                    ability,
                    *count,
                    *spread,
                    *damage_mult,
                    *pierce,
                    *spawn_offset,
                    ctx,
                );
            }
            AbilityEffect::SpawnAoeZone {
                radius,
                damage_mult,
                duration,
                tick_interval,
                visual,
                visual_y,
            } => {
                let pos = ctx.placed_position();
                if let Some(p) = visual {
                    let emitter = p.into_emitter(pos + Vec3::new(0.0, *visual_y, 0.0));
                    ctx.renderer.particle_system.add_emitter(emitter);
                }
                ctx.projectiles.queue_aoe(AoeZone {
                    position: pos,
                    radius: *radius,
                    damage_per_tick: ctx.damage * *damage_mult,
                    tick_interval: *tick_interval,
                    duration: *duration,
                    elapsed: 0.0,
                    tick_timer: 0.0,
                });
            }
            AbilityEffect::InstantAreaDamage {
                radius,
                damage_mult,
            } => {
                let pos = ctx.placed_position();
                let dmg = ctx.damage * *damage_mult;
                // Snapshot enemies in range first, then apply damage
                // through the centralised path so debuff multipliers
                // (Mark for Death, etc.) get respected.
                let targets: Vec<hecs::Entity> = ctx
                    .world
                    .query::<(&Transform, &Enemy)>()
                    .iter()
                    .filter(|(_, (t, _))| (t.position - pos).length() < *radius)
                    .map(|(e, _)| e)
                    .collect();
                for entity in targets {
                    crate::combat::debuff::apply_damage(ctx.world, entity, dmg);
                }
            }
            AbilityEffect::SetPlayerAction {
                action,
                duration,
                clip,
                movement,
                cancel_cast,
                emitter,
            } => {
                set_player_action(*action, *duration, clip, *movement, *cancel_cast, *emitter, ctx);
            }
            AbilityEffect::Custom(f) => f(ctx),
            AbilityEffect::ApplyDebuff { radius, debuff } => {
                let pos = ctx.placed_position();
                let targets: Vec<hecs::Entity> = ctx
                    .world
                    .query::<(&Transform, &Enemy)>()
                    .iter()
                    .filter(|(_, (t, _))| {
                        *radius <= 0.0 || (t.position - pos).length() < *radius
                    })
                    .map(|(e, _)| e)
                    .collect();
                for entity in targets {
                    // Ensure a Debuffs component exists, then apply.
                    if ctx.world.get::<&Debuffs>(entity).is_err() {
                        let _ = ctx.world.insert_one(entity, Debuffs::default());
                    }
                    if let Ok(mut bag) = ctx.world.get::<&mut Debuffs>(entity) {
                        bag.apply(debuff());
                    }
                }
            }
        }
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn spawn_projectile_volley(
    ability: &Ability,
    count: u32,
    spread: f32,
    damage_mult: f32,
    base_pierce: u32,
    offset: SpawnOffset,
    ctx: &mut AbilityCtx<'_>,
) {
    let yaw = ctx.aim_dir.x.atan2(ctx.aim_dir.z);
    let yaw_q = Quat::from_rotation_y(yaw);
    let spawn_pos = ctx.origin
        + Vec3::Y * offset.up
        + yaw_q * Vec3::new(offset.right, 0.0, 0.0)
        + ctx.aim_dir * offset.forward;

    // Resolve talent-driven pierce bonus for *this* ability id.
    let mut pierce_bonus: u32 = 0;
    if let Some(tree) = ctx.talents {
        for node in &tree.nodes {
            if node.current_rank == 0 {
                continue;
            }
            if let TalentEffect::AbilityMod {
                ability: mod_ab,
                modifier: AbilityModifier::Pierce(n),
            } = &node.effect
            {
                if *mod_ab == ability.id {
                    pierce_bonus += *n * node.current_rank as u32;
                }
            }
        }
    }

    let dmg = ctx.damage * damage_mult;
    for i in 0..count {
        let angle_offset = if count > 1 {
            let t = i as f32 / (count - 1) as f32 - 0.5;
            t * spread
        } else {
            0.0
        };
        let dir = Quat::from_rotation_y(angle_offset) * ctx.aim_dir;
        let mut proj = Projectile::arrow(spawn_pos, dir, dmg);
        proj.pierce_remaining = base_pierce + pierce_bonus;
        ctx.projectiles.queue_projectile(proj);
    }
}

fn set_player_action(
    action: PlayerAction,
    duration: f32,
    clip_names: &[&str],
    movement: ActionMovement,
    cancel_cast: bool,
    emitter: Option<ParticlePreset>,
    ctx: &mut AbilityCtx<'_>,
) {
    // Locate the player.
    let player_id = ctx.world.query::<&Player>().iter().map(|(e, _)| e).next();
    let Some(pid) = player_id else { return };

    // Snapshot transform for emitter placement and forward direction.
    let player_t = ctx.world.get::<&Transform>(pid).ok().map(|t| (t.position, t.rotation));
    let Some((position, rotation)) = player_t else { return };

    if let Some(p) = emitter {
        let e = p.into_emitter(position + Vec3::new(0.0, 0.5, 0.0));
        ctx.renderer.particle_system.add_emitter(e);
    }

    // Drive movement choice.
    let body_dir = {
        let fwd = rotation * Vec3::Z;
        let f = Vec3::new(fwd.x, 0.0, fwd.z);
        if f.length_squared() > 0.0001 { f.normalize() } else { Vec3::Z }
    };
    if let Ok(mut p) = ctx.world.get::<&mut Player>(pid) {
        p.action = action;
        p.action_timer = duration;
        // For Forward-style actions, cache the direction in `aim_dir`
        // so `player_action_pre_system` keeps the velocity glued there.
        if let ActionMovement::Forward(_) = movement {
            p.aim_dir = body_dir;
        }
    }
    if let Ok(mut v) = ctx.world.get::<&mut Velocity>(pid) {
        match movement {
            ActionMovement::Forward(speed) => v.linear = body_dir * speed,
            ActionMovement::Frozen => v.linear = Vec3::ZERO,
            ActionMovement::None => {}
        }
    }

    // Cross-fade into the action clip.
    let clip = ctx
        .world
        .get::<&AnimationSet>(pid)
        .ok()
        .and_then(|s| s.find_any(clip_names));
    if let Some(clip) = clip {
        if let Ok(mut anim) = ctx.world.get::<&mut Animator>(pid) {
            anim.cross_fade(clip, false, 0.08);
            anim.speed = 1.0;
        }
    }

    if cancel_cast {
        if let Ok(mut cast) = ctx.world.get::<&mut SpellCast>(pid) {
            cast.phase = SpellPhase::Idle;
            cast.layer_animator = None;
            cast.weight = 0.0;
            cast.pending_oneshot = None;
        }
    }
}

/// Convenience: dispatch through `execute_ability` from the common
/// "instant cast in aim dir" call site.
pub fn execute_ability_instant(
    ability: &Ability,
    origin: Vec3,
    aim_dir: Vec3,
    damage: f32,
    talents: Option<&TalentTree>,
    projectiles: &mut ProjectilePool,
    world: &mut World,
    renderer: &mut Renderer,
) {
    let mut ctx = AbilityCtx {
        origin,
        aim_dir,
        target: None,
        damage,
        talents,
        world,
        renderer,
        projectiles,
    };
    execute_ability(ability, &mut ctx);
}

/// Convenience: dispatch through `execute_ability` for a placed cast.
pub fn execute_ability_placed(
    ability: &Ability,
    target: Vec3,
    damage: f32,
    talents: Option<&TalentTree>,
    projectiles: &mut ProjectilePool,
    world: &mut World,
    renderer: &mut Renderer,
) {
    let mut ctx = AbilityCtx {
        origin: target,
        aim_dir: Vec3::Z,
        target: Some(target),
        damage,
        talents,
        world,
        renderer,
        projectiles,
    };
    execute_ability(ability, &mut ctx);
}
