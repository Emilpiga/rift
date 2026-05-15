//! Client-side mirror of the server's revive shrines.
//!
//! Tracks one [`ShrineVisual`] per replicated
//! [`EntityKind::ReviveShrine`] row: spawns the holy beam VFX
//! when the row first appears, despawns it when the row drops
//! out of the snapshot (channel completion or floor change),
//! and surfaces the F-prompt + progress bar on the HUD.
//!
//! The actual channel mechanic is server-authoritative — this
//! module just toggles intent via
//! [`NetClient::request_toggle_shrine_channel`] on F-press and
//! mirrors the server's progress / channelers / required
//! readout for the HUD.

use glam::Vec3;
use rift_engine::animation_profile::{AnimBindings, AnimClipKey, JointKey, SkeletonBindings};
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::input::Input;
use rift_engine::renderer::vfx::{presets, EffectId};
use rift_engine::renderer::Renderer;
use rift_net::messages::SHRINE_INTERACT_RADIUS;
use rift_net::NetId;
use std::collections::HashMap;

use crate::game::sub_state::NetState;
use crate::game::sub_state::ShrineClientState;

/// Per-shrine on-screen state mirrored from the snapshot. The
/// emitter id lets us tear the VFX down cleanly when the server
/// despawns the shrine row.
#[derive(Clone)]
pub struct ShrineVisual {
    pub net_id: NetId,
    pub position: Vec3,
    pub progress: f32, // 0.0..=1.0
    pub channelers: u8,
    pub required: u8,
    pub pillar_emitter: EffectId,
}

/// Reconcile the local shrine list with the latest snapshot of
/// `EntityKind::ReviveShrine` rows. Spawns visuals for new ids,
/// updates progress/counts on existing ones, despawns visuals
/// whose ids dropped out.
pub fn sync_visuals(
    shrines: &mut ShrineClientState,
    renderer: &mut Renderer,
    rows: &HashMap<NetId, (Vec3, f32, u8, u8)>,
) {
    // Despawn stale visuals first so the renderer slot count
    // doesn't balloon during a flurry of channel completions.
    shrines.visuals.retain(|v| {
        if rows.contains_key(&v.net_id) {
            true
        } else {
            renderer.vfx_system.despawn(v.pillar_emitter);
            false
        }
    });

    // Spawn-or-update each row.
    for (net_id, (pos, progress, channelers, required)) in rows {
        if let Some(v) = shrines.visuals.iter_mut().find(|v| v.net_id == *net_id) {
            v.position = *pos;
            v.progress = *progress;
            v.channelers = *channelers;
            v.required = *required;
            continue;
        }
        let pillar = renderer
            .vfx_system
            .spawn(presets::revive_shrine_pillar(), *pos);
        log::info!("shrine: visual spawned for {net_id:?} at {pos:?}");
        shrines.visuals.push(ShrineVisual {
            net_id: *net_id,
            position: *pos,
            progress: *progress,
            channelers: *channelers,
            required: *required,
            pillar_emitter: pillar,
        });
    }
}

/// Find the closest shrine the local player is standing inside
/// the interact radius of. Used by the HUD prompt + F-press
/// dispatch. Returns `None` if no shrine is in range.
pub fn nearest_in_range(world: &hecs::World, shrines: &ShrineClientState) -> Option<NetId> {
    if shrines.visuals.is_empty() {
        return None;
    }
    let player_pos = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()?;
    let radius_sq = SHRINE_INTERACT_RADIUS * SHRINE_INTERACT_RADIUS;
    let mut best: Option<(NetId, f32)> = None;
    for v in &shrines.visuals {
        let d2 = (v.position - player_pos).length_squared();
        if d2 > radius_sq {
            continue;
        }
        if best.map_or(true, |(_, b)| d2 < b) {
            best = Some((v.net_id, d2));
        }
    }
    best.map(|(id, _)| id)
}

/// Per-frame interact tick. Computes the desired channel
/// intent ("am I alive, in range, and holding F?") from the
/// current input + shrine list, mirrors it onto
/// `local_intent` for VFX / HUD use, and queues an
/// edge-triggered server send when it changes.
pub fn tick(
    shrines: &mut ShrineClientState,
    world: &hecs::World,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    is_ghost: bool,
) {
    use winit::keyboard::KeyCode;

    let in_range = if is_ghost {
        None
    } else {
        nearest_in_range(world, shrines)
    };
    let f_held = input.is_key_held(KeyCode::KeyF);

    // HUD prompt: only when in range + alive. The label hints
    // at hold semantics so the player doesn't single-tap and
    // wonder why nothing happened.
    if let Some(_) = in_range {
        *hud_prompt = Some(if f_held {
            "CHANNELING... HOLD [F]"
        } else {
            "HOLD [F] TO CHANNEL REVIVE SHRINE"
        });
    }

    // Desired intent: shrine in range AND F held AND alive.
    let desired = match (in_range, f_held) {
        (Some(id), true) => Some(id),
        _ => None,
    };

    if desired != shrines.local_intent {
        // Mirror locally so the beam + pose can react this
        // frame; queue the wire send for the binary's drain.
        shrines.local_intent = desired;
        net.pending_shrine_intent = Some(desired);
    }
}

/// Drive the local player's channel pose + the beam VFX
/// endpoints. Edge-detects `local_intent` flips:
///   - `None -> Some`: enters the `Spell_Simple_Idle_Loop` upper-body
///     cast pose via `SpellCast` and spawns the beam emitter.
///   - `Some -> None` (or shrine despawned): cancels the pose
///     and tears the beam down.
/// While active, refreshes the beam's endpoints + the hand swirl's
/// anchor every frame so they track the moving player and shrine.
///
/// `is_ghost` is passed in because ghosts can't channel (they're
/// the ones being revived) so we never want to drive their pose.
pub fn tick_channel_pose(
    shrines: &mut ShrineClientState,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    player_id: Option<hecs::Entity>,
    is_ghost: bool,
) {
    use rift_engine::ecs::components::{AnimationSet, Skinned, SpellCast, SpellPhase};

    let prev = shrines.prev_local_intent;
    // Suppress channeling visuals entirely for ghosts.
    let curr = if is_ghost { None } else { shrines.local_intent };

    // Resolve current shrine position (if still alive). If the
    // shrine row dropped out of the snapshot the channel is over
    // even if `local_intent` still has a stale id; treat that
    // same as a `Some -> None` transition.
    let shrine_pos = curr.and_then(|id| {
        shrines
            .visuals
            .iter()
            .find(|v| v.net_id == id)
            .map(|v| v.position)
    });
    let active = shrine_pos.is_some();
    let was_active = prev.is_some();

    // Player-position lookup. Bail entirely if we have no local
    // avatar yet (loading screen, character select, etc.).
    let player_pos =
        player_id.and_then(|pid| world.get::<&Transform>(pid).ok().map(|t| t.position));

    // Hand position from the rigged skeleton. Mirrors the same
    // join lookup the cast pipeline uses for projectile origins
    // (see `tick_combat`'s `hand` resolution): pull the joint's
    // local-space translation from `Skinned::joint_worlds`,
    // transform by the avatar's world matrix. Falls back to a
    // chest-height offset if anything is missing (e.g. unrigged
    // mesh, hand_joint not yet bound).
    let hand_pos = player_id
        .and_then(|pid| {
            let mut q = world
                .query_one::<(
                    &Transform,
                    &Player,
                    Option<&SkeletonBindings>,
                    Option<&Skinned>,
                )>(pid)
                .ok()?;
            let (t, p, bindings, s) = q.get()?;
            let hand_joint = bindings
                .and_then(|b| b.get(JointKey::CastHand))
                .unwrap_or(p.hand_joint);
            if hand_joint == u32::MAX {
                return None;
            }
            let s = s?;
            let m = s.joint_worlds.get(hand_joint as usize)?;
            let local = m.col(3).truncate();
            Some(t.matrix().transform_point3(local))
        })
        .or_else(|| player_pos.map(|p| p + Vec3::new(0.0, 1.15, 0.0)));

    // ── Edge: enter channel ──────────────────────────────────
    if active && !was_active {
        if let (Some(pid), Some(_)) = (player_id, player_pos) {
            // Only set up the cast pose if a `Spell_Simple_Idle_Loop`
            // (or fallback) clip exists in the rig. Without the
            // clip the engine system would just reset state, so
            // skip the SpellCast setup but still spawn the beam.
            let has_clip = world
                .get::<&AnimBindings>(pid)
                .ok()
                .and_then(|bindings| bindings.get(AnimClipKey::ChannelLoop))
                .or_else(|| {
                    world.get::<&AnimationSet>(pid).ok().and_then(|set| {
                        set.find_any(&[
                            "Spell_Simple_Idle_Loop",
                            "Spell_Idle_Loop",
                            "Cast_Loop",
                            "Channel_Loop",
                            "Spell_Simple_Enter",
                        ])
                    })
                })
                .is_some();
            if has_clip {
                if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
                    cast.phase = SpellPhase::Entering;
                    cast.channeling = true;
                    cast.fired = true; // suppress projectile fire-out
                    cast.pending_ability = None;
                    cast.pending_oneshot = None;
                    cast.oneshot_is_hit = false;
                }
            }
            // Spawn the beam ribbon. Endpoints are refreshed
            // below in the always-on update branch.
            let id = renderer.vfx_system.spawn(
                presets::shrine_channel_beam(),
                hand_pos.unwrap_or(Vec3::ZERO),
            );
            shrines.channel_beam = Some(id);
        }
    }

    // ── Edge: exit channel ───────────────────────────────────
    if !active && was_active {
        if let Some(pid) = player_id {
            if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
                if cast.channeling {
                    cast.phase = SpellPhase::Exiting;
                    cast.channeling = false;
                }
            }
        }
        if let Some(id) = shrines.channel_beam.take() {
            renderer.vfx_system.despawn(id);
        }
    }

    // ── Active: refresh endpoints + anchor ───────────────────
    if active {
        if let (Some(beam), Some(hp), Some(sp)) = (shrines.channel_beam, hand_pos, shrine_pos) {
            // Aim at the shrine's mid-height so the beam doesn't
            // dive into the floor on tall avatars.
            let tip = sp + Vec3::new(0.0, 0.8, 0.0);
            renderer.vfx_system.set_endpoints(beam, hp, tip);
            renderer.vfx_system.set_anchor(beam, hp);
        }
    }

    shrines.prev_local_intent = curr;
}
