//! Data-driven ability runtime — client-side cast-anim FSM.
//!
//! Declarative ability data (`Ability`, `AbilityEffect`, `ParticlePreset`,
//! ...) lives in `rift_game::abilities`. This file only contains the
//! ECS-touching execution helpers that interpret those types against
//! the engine's renderer + world.

use glam::Vec3;
use hecs::World;

use rift_game::abilities::{Ability, AbilityEffect, ActionMovement, ParticlePreset};
use rift_game::components::PlayerAction;
use rift_game::talents::TalentTree;

use crate::animation::Animator;
use crate::ecs::components::{
    AnimationSet, LocalPlayer, Player, SpellCast, SpellPhase, Transform, Velocity,
};
use crate::renderer::vfx::{presets as vfx_presets, spec::Effect};
use crate::renderer::Renderer;

/// Map a declarative `ParticlePreset` to a concrete VFX [`Effect`].
/// Routed through the declarative `vfx::presets` module rather
/// than the legacy `EmitterConfig` builders so all ability
/// visuals share one rendering path.
pub fn effect_for_preset(preset: ParticlePreset) -> Effect {
    match preset {
        ParticlePreset::DodgePuff => vfx_presets::dodge_puff(),
        ParticlePreset::RainOfFire => vfx_presets::rain_of_fire(),
        ParticlePreset::Cast(rgb) => vfx_presets::cast_spark(rgb),
    }
}

/// Resources shared by every effect arm during one `execute_ability` call.
pub struct AbilityCtx<'a> {
    pub origin: Vec3,
    pub aim_dir: Vec3,
    pub target: Option<Vec3>,
    pub damage: f32,
    pub talents: Option<&'a TalentTree>,
    pub world: &'a mut World,
    pub renderer: &'a mut Renderer,
}

impl<'a> AbilityCtx<'a> {
    pub fn placed_position(&self) -> Vec3 {
        self.target.unwrap_or(self.origin + self.aim_dir * 5.0)
    }
}

/// Walk every effect in `ability.effects` and execute it. Damage-bearing
/// variants are no-ops on the client.
pub fn execute_ability(ability: &Ability, ctx: &mut AbilityCtx<'_>) {
    for effect in ability.effects {
        match effect {
            AbilityEffect::SpawnProjectiles { .. } => {
                // Server-authoritative.
            }
            AbilityEffect::SpawnAoeZone { visual, visual_y, .. } => {
                if let Some(p) = visual {
                    let pos = ctx.placed_position();
                    ctx.renderer
                        .vfx_system
                        .spawn(effect_for_preset(*p), pos + Vec3::new(0.0, *visual_y, 0.0));
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
        }
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
    let player_id = ctx
        .world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(e, _)| e)
        .next();
    let Some(pid) = player_id else { return };

    let player_t = ctx.world.get::<&Transform>(pid).ok().map(|t| (t.position, t.rotation));
    let Some((position, rotation)) = player_t else { return };

    if let Some(p) = emitter {
        ctx.renderer
            .vfx_system
            .spawn(effect_for_preset(p), position + Vec3::new(0.0, 0.5, 0.0));
    }

    let body_dir = {
        let fwd = rotation * Vec3::Z;
        let f = Vec3::new(fwd.x, 0.0, fwd.z);
        if f.length_squared() > 0.0001 { f.normalize() } else { Vec3::Z }
    };
    if let Ok(mut p) = ctx.world.get::<&mut Player>(pid) {
        p.action = action;
        p.action_timer = duration;
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
    };
    execute_ability(ability, &mut ctx);
}

/// Convenience: dispatch through `execute_ability` for a placed cast.
pub fn execute_ability_placed(
    ability: &Ability,
    target: Vec3,
    damage: f32,
    talents: Option<&TalentTree>,
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
    };
    execute_ability(ability, &mut ctx);
}
