//! Local-player death / rise / ghost-state subsystem.
//!
//! Owns the small state machine that drives the local
//! avatar's lifecycle around death:
//!
//! ```text
//!   alive ──HP→0──▶ down-pose ──server flips ghost──▶ rising
//!     ▲                                                  │
//!     └──────────── respawn / shrine revive ◀────────────┘
//! ```
//!
//! All of these helpers operate on a `&mut hecs::World` plus
//! a few caller-owned scalars (the local-ghost cache, the
//! HP-edge tracker, the damage-flash strength, the floor
//! number for parity-driven anim variants). They are called
//! from `GameState`'s `update_gameplay` / `update_render`
//! phases — keeping the actual logic out of `state.rs` so
//! `state.rs` reads as orchestration.

use glam::Vec3;
use hecs::World;

use rift_engine::animation::Animator;
use rift_engine::animation_profile::{AnimBindings, AnimClipKey, PLAYER_PROFILE};
use rift_engine::ecs::components::{
    AnimationSet, Ghost, GhostRising, Health, LocalPlayer, Player, PlayerAction, Renderable,
    SkinnedAttachments, SpellCast, SpellPhase, Velocity,
};
use rift_engine::renderer::Renderer;

/// Find the local player's avatar entity. There is at most
/// one (`LocalPlayer` is a singleton tag on the predicted
/// avatar) so we just take the first match.
pub fn player_id(world: &World) -> Option<hecs::Entity> {
    world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(e, _)| e)
        .next()
}

/// `true` while the local player is in the post-death
/// down-pose: HP is at zero AND the server hasn't yet
/// flipped us to ghost mode. Once the local-ghost cache
/// goes true we leave the down-pose — input + camera +
/// movement systems all re-engage so the player can scout.
/// Cast / loot pickup remain server-rejected for ghosts.
pub fn is_dead(world: &World, local_ghost_cached: bool) -> bool {
    if local_ghost_cached {
        return false;
    }
    let Some(pid) = player_id(world) else {
        return false;
    };
    world
        .get::<&Health>(pid)
        .map(|h| h.is_dead())
        .unwrap_or(false)
}

/// Detect HP drops on the local player since last frame and play
/// a hit-react one-shot on the upper body. Uses the built-in
/// `SpellCast::play_hit` cooldown so the flinch doesn't repeat
/// every frame while the player is being chewed on.
///
/// `prev_hp` is the caller-owned previous-frame HP latch
/// (typically `GameState::prev_player_hp`); this fn refreshes
/// it in place. `damage_flash` is bumped for non-lethal HP
/// drops so the screen-edge vignette accompanies the hit even
/// if the avatar has no matching hit-react clip. `floor` is
/// mixed into the candidate-clip pick so adjacent floors
/// alternate Hit_Chest / Hit_Head for variety.
pub fn tick_hit_react(
    world: &mut World,
    prev_hp: &mut Option<f32>,
    damage_flash: &mut f32,
    floor: u32,
) {
    let Some(pid) = player_id(world) else {
        *prev_hp = None;
        return;
    };
    let (cur_hp, max_hp) = match world.get::<&Health>(pid) {
        Ok(h) => (h.current, h.max.max(0.001)),
        Err(_) => return,
    };
    let prev = *prev_hp;
    *prev_hp = Some(cur_hp);
    let Some(prev) = prev else { return };
    if cur_hp + 0.001 >= prev {
        return;
    }
    // Don't replay if death just triggered.
    if cur_hp <= 0.001 {
        return;
    }

    let damage_frac = ((prev - cur_hp) / max_hp).clamp(0.0, 1.0);
    let flash = (0.18 + damage_frac * 0.70).min(0.48);
    *damage_flash = (*damage_flash).max(flash);

    // Pick a chest/head hit at random for variety. The asset
    // pack ships `Hit_Chest` and `Hit_Head`; either is fine.
    let candidates: &[&str] = if floor % 2 == 0 {
        &["Hit_Chest", "Hit_Head", "HitRecieve", "HitReceive", "Hit"]
    } else {
        &["Hit_Head", "Hit_Chest", "HitRecieve", "HitReceive", "Hit"]
    };
    let clip = world
        .get::<&AnimBindings>(pid)
        .ok()
        .and_then(|bindings| bindings.get(AnimClipKey::HitReact))
        .or_else(|| {
            world
                .get::<&AnimationSet>(pid)
                .ok()
                .and_then(|set| set.find_any(candidates))
        });
    let Some(clip) = clip else { return };

    if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
        cast.play_hit(clip);
    }
}

/// Triggered when the snapshot brings local Health to zero. Plays
/// the death animation and freezes input. Server-authoritative
/// respawn happens via a follow-up `LoadFloor`.
///
/// Bumps `damage_flash` (caller-owned post-FX scalar) so the
/// kill-frame is also a screen flash.
pub fn trigger_death(world: &mut World, damage_flash: &mut f32, floor: u32) {
    *damage_flash = (*damage_flash + 0.45).min(0.85);
    log::info!("Player death triggered (rift floor {}).", floor);

    let Some(pid) = player_id(world) else { return };

    let clip = world
        .get::<&AnimBindings>(pid)
        .ok()
        .and_then(|bindings| bindings.get(AnimClipKey::Death))
        .or_else(|| {
            world
                .get::<&AnimationSet>(pid)
                .ok()
                .and_then(|set| set.find_any(PLAYER_PROFILE.names_for(AnimClipKey::Death)))
        });
    let Some(clip) = clip else {
        log::warn!("Death animation not found in player's clip set");
        return;
    };

    if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
        cast.phase = SpellPhase::Idle;
        cast.layer_animator = None;
        cast.weight = 0.0;
        cast.pending_oneshot = None;
        cast.oneshot_is_hit = false;
    }
    if let Ok(mut anim) = world.get::<&mut Animator>(pid) {
        anim.cross_fade(clip, false, 0.18);
        anim.speed = 1.0;
    }
    if let Ok(mut vel) = world.get::<&mut Velocity>(pid) {
        vel.linear = Vec3::ZERO;
    }
    if let Ok(mut p) = world.get::<&mut Player>(pid) {
        p.action = PlayerAction::None;
        p.action_timer = 0.0;
        p.vy = 0.0;
        p.airborne = false;
    }
}

/// Crossfade the local avatar out of the death pose into
/// idle once the server flips us to ghost mode. Mirror image
/// of [`trigger_death`] — same component touch-points,
/// opposite clip + intent. Runs once per ghost transition
/// (edge-detected by the caller).
pub fn trigger_rise(world: &mut World, floor: u32) {
    log::info!("Player rose as ghost (rift floor {}).", floor);

    let Some(pid) = player_id(world) else { return };

    // Asset pack ships `LayToIdle` (UAL2) which is the perfect
    // get-up-from-corpse-pose anim. Fall back to plain Idle
    // crossfade if the rig somehow lacks it.
    let lay_to_idle = world
        .get::<&AnimBindings>(pid)
        .ok()
        .and_then(|bindings| bindings.get(AnimClipKey::GhostRise))
        .or_else(|| {
            world
                .get::<&AnimationSet>(pid)
                .ok()
                .and_then(|set| set.find_any(PLAYER_PROFILE.names_for(AnimClipKey::GhostRise)))
        });

    if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
        cast.phase = SpellPhase::Idle;
        cast.layer_animator = None;
        cast.weight = 0.0;
        cast.pending_oneshot = None;
        cast.oneshot_is_hit = false;
    }
    if let Some(clip) = lay_to_idle {
        // `LayToIdle` is a one-shot get-up animation. Tag the
        // avatar with `GhostRising` (with the clip's duration
        // as countdown) so engine systems keep the dead-gate
        // engaged - movement / input stay locked while the
        // rise plays. `tick_rise` swaps the marker for `Ghost`
        // once the timer hits 0.
        let duration = clip.duration.max(0.1);
        if let Ok(mut anim) = world.get::<&mut Animator>(pid) {
            anim.cross_fade(clip, false, 0.25);
            anim.speed = 1.0;
        }
        // 0.25s crossfade + clip duration. Leave a small buffer
        // (0.1s) so the standing pose is fully visible before
        // input unlocks.
        let _ = world.insert_one(
            pid,
            GhostRising {
                remaining: duration + 0.1,
            },
        );
        return;
    }

    // Fallback: no `LayToIdle` clip in the rig. Skip straight
    // to ghost mode + idle loop - better than freezing the
    // player on the down-pose forever.
    let _ = world.insert_one(pid, Ghost);
    let clip = world
        .get::<&AnimBindings>(pid)
        .ok()
        .and_then(|bindings| bindings.get(AnimClipKey::Idle))
        .or_else(|| {
            world
                .get::<&AnimationSet>(pid)
                .ok()
                .and_then(|set| set.find_any(PLAYER_PROFILE.names_for(AnimClipKey::Idle)))
        });
    let Some(clip) = clip else {
        log::warn!("Idle animation not found for ghost rise");
        return;
    };
    if let Ok(mut anim) = world.get::<&mut Animator>(pid) {
        anim.cross_fade(clip, true, 0.35);
        anim.speed = 1.0;
    }
}

/// Tick the [`GhostRising`] countdown on the local player and
/// promote them to a full [`Ghost`] once the rise anim has
/// finished playing. While `GhostRising` is present, engine
/// systems still see the avatar as dead so movement / input
/// stay locked — the player is glued to the standing pose for
/// the brief get-up-from-floor window. Called once per frame
/// from `update_gameplay`.
pub fn tick_rise(world: &mut World, dt: f32) {
    let Some(pid) = player_id(world) else { return };
    let finished = if let Ok(mut rising) = world.get::<&mut GhostRising>(pid) {
        rising.remaining -= dt;
        rising.remaining <= 0.0
    } else {
        return;
    };
    if finished {
        let _ = world.remove_one::<GhostRising>(pid);
        let _ = world.insert_one(pid, Ghost);
        log::info!("Ghost rise animation complete - movement unlocked");
    }
}

/// Strip both `Ghost` and `GhostRising` markers — called on
/// the inverse edge (ghost → respawned) so the engine's
/// dead-gates re-engage if HP somehow lands at 0 again before
/// the world is rebuilt.
pub fn clear_markers(world: &mut World) {
    let Some(pid) = player_id(world) else { return };
    let _ = world.remove_one::<Ghost>(pid);
    let _ = world.remove_one::<GhostRising>(pid);
}

/// Push the ghost tint onto the local player's renderer
/// slots when `local_ghost_cached` is true; otherwise force
/// them back to opaque white. Touches the base `Renderable`
/// slot plus every visible `SkinnedAttachments` piece (so
/// outfit gear ghosts together with the body). Cheap O(N)
/// over the local avatar's attachments — runs every frame
/// from `update_render` so a respawn snaps back to opaque
/// without a one-frame flicker.
pub fn apply_tint(world: &World, renderer: &mut Renderer, local_ghost_cached: bool) {
    // Pale cyan-white at 40% alpha. RGB > 1.0 in the cyan
    // channels gives the lit colour a faint spectral lift even
    // after the multiply (lit * tint), since the forward
    // pipeline outputs HDR before tonemap.
    const GHOST_TINT: [f32; 4] = [0.75, 0.92, 1.05, 0.40];
    const OPAQUE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    let tint = if local_ghost_cached {
        GHOST_TINT
    } else {
        OPAQUE
    };

    // Drive the post-composite ghost-view effect (desat + cool
    // tint + radial vignette). Instant-on for now — could be
    // eased over ~0.3s on the rise edge if we want a softer
    // transition.
    renderer.ghost_mix = if local_ghost_cached { 1.0 } else { 0.0 };

    let mut local_entity = None;
    for (e, _) in world.query::<&LocalPlayer>().iter() {
        local_entity = Some(e);
        break;
    }
    let Some(entity) = local_entity else { return };

    // Base mesh.
    if let Ok(r) = world.get::<&Renderable>(entity) {
        if let Some(obj) = renderer.objects.get_mut(r.object_index) {
            obj.tint = tint;
        }
    }
    // Outfit attachments.
    if let Ok(attach) = world.get::<&SkinnedAttachments>(entity) {
        for piece in &attach.pieces {
            if let Some(obj) = renderer.objects.get_mut(piece.object_index) {
                obj.tint = tint;
            }
        }
    }
}
