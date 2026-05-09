//! Client-side ability plumbing — cast pose FSM, channel beam
//! visuals, and the matching server-event handlers.
//!
//! Three entry-point shapes live here, all touching the same
//! `state.channel.visuals` list and per-entity `SpellCast`
//! component:
//!
//! 1. **Local-input dispatch.** [`trigger_local_cast`] is fired
//!    from `tick_combat` when the local player presses an
//!    action-bar key. It runs the ability's declarative
//!    `effects` list and drives the cast-pose FSM.
//!
//! 2. **Server-event handlers.** One `on_<event>` function per
//!    `WorldEvent` variant that needs client-side visuals or
//!    component mutations:
//!
//!    - [`on_remote_ability_cast`] — `WorldEvent::AbilityCast`
//!    - [`on_channel_tick`] — `WorldEvent::ChannelTick`
//!    - [`on_channel_end`] — `WorldEvent::ChannelEnd`
//!    - [`on_remote_death`] — `WorldEvent::Death` for remote
//!      avatars
//!
//!    Each handler takes whatever resolved context the binary's
//!    event loop already has (the matching `Ability`, the
//!    caster's avatar entity if any, …) and does the full set
//!    of side-effects for that event in one call. The
//!    `NetId → entity` mapping stays in `main.rs` where
//!    `NetClient` lives.
//!
//! 3. **Per-frame tick.** [`tick_channel_visuals`] runs every
//!    frame from `update_render` to keep the beam meshes fresh
//!    against caster movement and wall raycasts.
//!
//! Loot-drop visuals live in `loot_system` instead — they don't
//! touch `SpellCast` or channel state.

use glam::Vec3;
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::Renderer;
use rift_game::abilities::Ability;
use rift_game::talents::TalentTree;

use super::state::GameState;
use super::sub_state::ChannelVisual;

/// Local cast feedback. The server still owns damage / projectile
/// spawn — this just plays the cast animation + any client-side
/// particles immediately so the input feels responsive.
///
/// Two responsibilities, in order:
///
/// 1. **Run the ability's declarative `effects` list.** This is
///    the authoritative source of truth for client-side cast
///    visuals: cast-time emitters, dodge puffs, AoE-zone
///    particle spawns, `SetPlayerAction` cross-fades, etc.
///    `SpawnProjectiles` is server-authoritative and is a
///    no-op here — running it is harmless. Always running this
///    list means new effect variants don't need a corresponding
///    branch in this dispatcher.
///
/// 2. **Drive the cast-pose FSM** for ability shapes that need
///    one. This isn't expressible as data because it touches
///    the per-skeleton `SpellCast` component — projectile
///    casts use `cast.begin` (one-shot), channels use
///    `cast.begin_channel` (held until end-of-channel).
pub fn trigger_local_cast(
    ability: &Ability,
    aim_dir: Vec3,
    origin: Vec3,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    talents: &TalentTree,
) {
    use rift_engine::ecs::components::SpellCast;

    // 1. Always run the declarative effects list. Authors put
    //    visual / movement / FSM-side-effects here; we don't
    //    second-guess what's in it based on the ability kind.
    rift_engine::combat::execute_ability_instant(
        ability,
        origin,
        aim_dir,
        0.0,
        Some(talents),
        world,
        renderer,
    );

    // 2. Cast-pose FSM. Projectile shapes get a one-shot pose;
    //    channels get a held pose released by `cast.end_channel`
    //    on channel-end. Any other shape (AoE placed, movement,
    //    utility) doesn't need a pose and is fully covered by
    //    the effects list above.
    let has_projectile = ability
        .effects
        .iter()
        .any(|e| matches!(e, rift_game::abilities::AbilityEffect::SpawnProjectiles { .. }));
    let is_channeled = matches!(
        rift_game::abilities::lookup(ability.wire_id).map(|d| d.kind),
        Some(rift_game::abilities::AbilityKind::Channel { .. })
    );
    if !(has_projectile || is_channeled) {
        return;
    }
    let pid = world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(e, _)| e)
        .next();
    let Some(pid) = pid else { return };
    let Ok(mut cast) = world.get::<&mut SpellCast>(pid) else { return };
    if is_channeled {
        cast.begin_channel(ability.clone(), aim_dir);
    } else {
        cast.begin(ability.clone(), aim_dir, 0.0);
    }
}

/// Handle a `WorldEvent::AbilityCast` from the server.
///
/// Two side-effects, both unconditional once we know the ability:
///
/// 1. **Ground-AoE emitter** for any `SpawnAoeZone` effects on
///    the ability. Driven off the server event so every observer
///    (including the local caster, who otherwise returns out of
///    `tick_combat` after sending the placement) sees the same
///    visual at the same authoritative position.
///
/// 2. **Upper-body cast pose** on the caster's avatar. Skipped
///    when `caster_avatar` is `None` (which the binary passes
///    for the local caster, whose pose is already running from
///    `trigger_local_cast` the moment the input fired).
///
/// `cast_origin` is the server-reported caster position, used as
/// the fallback origin for the AoE emitter when `target` is
/// `None`. `aim` is the cast direction (XZ-plane vector).
pub fn on_remote_ability_cast(
    state: &mut GameState,
    renderer: &mut Renderer,
    ability: &Ability,
    aim: Vec3,
    cast_origin: Vec3,
    target: Option<Vec3>,
    caster_avatar: Option<hecs::Entity>,
) {
    use rift_engine::combat::effect_for_vfx;
    use rift_engine::ecs::components::SpellCast;

    // 1. Ground-AoE emitter for any SpawnAoeZone effects.
    for effect in ability.effects {
        if let rift_game::abilities::AbilityEffect::SpawnAoeZone {
            visual,
            visual_y,
            ..
        } = effect
        {
            let Some(preset) = visual else { continue };
            // Match `AbilityCtx::placed_position`: use `target` if
            // the cast was placed (e.g. Rain of Fire), otherwise
            // fall back to a forward offset along aim from the
            // caster origin.
            let pos = target.unwrap_or(cast_origin + aim * 5.0)
                + Vec3::new(0.0, *visual_y, 0.0);
            renderer.vfx_system.spawn_bundle(effect_for_vfx(*preset), pos);
        }
    }

    // 1b. Caster-anchored one-shot emitters (Fire Wave etc.).
    // Mirrors the local-side `SpawnEmitterAtCaster` arm in
    // `execute_ability` so remote observers see the same burst
    // on the casting avatar. We use `cast_origin` (server-
    // authoritative caster position at cast time) as the
    // anchor; the live remote avatar may have moved since, but
    // the burst is short-lived enough that the snap to the
    // cast-time position reads as intentional.
    for effect in ability.effects {
        if let rift_game::abilities::AbilityEffect::SpawnEmitterAtCaster {
            visual,
            height,
        } = effect
        {
            renderer.vfx_system.spawn_bundle(
                effect_for_vfx(*visual),
                cast_origin + Vec3::new(0.0, *height, 0.0),
            );
        }
    }

    // 2. Remote cast pose. Only projectile / channel shapes drive
    //    a pose today; snapshots cover the rest.
    let Some(entity) = caster_avatar else { return };
    let has_projectile = ability
        .effects
        .iter()
        .any(|e| matches!(e, rift_game::abilities::AbilityEffect::SpawnProjectiles { .. }));
    let is_channeled = matches!(
        rift_game::abilities::lookup(ability.wire_id).map(|d| d.kind),
        Some(rift_game::abilities::AbilityKind::Channel { .. })
    );
    if !has_projectile && !is_channeled {
        return;
    }
    if let Ok(mut cast) = state.world.get::<&mut SpellCast>(entity) {
        if is_channeled {
            cast.begin_channel(ability.clone(), aim);
        } else {
            cast.begin(ability.clone(), aim, 0.0);
        }
    }
}

/// Handle a `WorldEvent::ChannelTick` from the server.
///
/// Updates an existing per-channel visual entry, or pushes a new
/// one if this is the first tick we've seen for this caster +
/// ability pair. The actual beam mesh is allocated lazily inside
/// [`tick_channel_visuals`], where we have access to the renderer.
pub fn on_channel_tick(
    state: &mut GameState,
    caster: rift_net::NetId,
    ability_id: u8,
    position: Vec3,
    aim: Vec3,
) {
    if let Some(entry) = state
        .channel
        .visuals
        .iter_mut()
        .find(|v| v.caster == caster && v.ability_id == ability_id)
    {
        entry.position = position;
        entry.aim = aim;
        entry.idle = 0.0;
        return;
    }
    state.channel.visuals.push(ChannelVisual {
        caster,
        ability_id,
        position,
        aim,
        idle: 0.0,
        obj_idx: None,
        vfx_id: None,
        ending: false,
        impact_acc: 0.0,
    });
}

/// Handle a `WorldEvent::ChannelEnd` from the server.
///
/// Three side-effects in one call:
///
/// 1. Clear `state.channel.active` if this was our channel.
/// 2. Cancel the cast-pose layer on the caster's avatar.
/// 3. Flag the per-channel visual entry for hide-and-drop on
///    the next [`tick_channel_visuals`] frame.
///
/// `caster_entity` is the caster's avatar entity (local or
/// remote); pass `None` when no entity is known and the cast
/// pose cancel is skipped. `is_local_caster` toggles step 1.
pub fn on_channel_end(
    state: &mut GameState,
    caster: rift_net::NetId,
    ability_id: u8,
    caster_entity: Option<hecs::Entity>,
    is_local_caster: bool,
) {
    use rift_engine::ecs::components::SpellCast;

    if is_local_caster {
        state.channel.active = None;
    }
    if let Some(entity) = caster_entity {
        if let Ok(mut cast) = state.world.get::<&mut SpellCast>(entity) {
            cast.cancel();
        }
    }
    if let Some(entry) = state
        .channel
        .visuals
        .iter_mut()
        .find(|v| v.caster == caster && v.ability_id == ability_id)
    {
        // Defer the actual hide-and-drop to `tick_channel_visuals`,
        // which has access to the renderer to despawn the VFX.
        entry.ending = true;
    }
}


/// Per-frame update for every active channel visual.
///
/// For each entry we lazily spawn a stretched beam mesh on the
/// renderer the first time we see it, then on subsequent frames
/// update its endpoints so the beam tracks the caster's hand and
/// aim direction. Walls clip the beam length via a raycast
/// against `state.floor.wall_aabbs`. Idle entries (no tick for ~0.4 s)
/// and entries flagged `ending` get their VFX despawned and are
/// dropped from the visuals list.
pub fn tick_channel_visuals(state: &mut GameState, renderer: &mut Renderer, dt: f32) {
    use rift_engine::physics::{self, Ray};
    use rift_game::abilities::{ShapeVisuals, VfxKind};

    // Common channel-render constants. Per-ability data
    // (beam VFX choice, hand offset) is pulled from each
    // ability's `ShapeVisuals` recipe — no `if ability_id == X`.
    const IDLE_TIMEOUT: f32 = 0.4;
    const IMPACT_INTERVAL: f32 = 0.10; // 10 Hz cold-burst cadence

    // Pull the local player's live transform + aim, and the
    // *world-space* position of its right-hand joint if the
    // skinning pass has produced one this frame. Beam visuals
    // for our own channel anchor at the hand for accuracy
    // (server tick rate of ~5 Hz would otherwise look choppy
    // *and* off-anatomy).
    use rift_engine::ecs::components::Skinned;
    let local_live: Option<(Vec3, Vec3, Option<Vec3>)> = state
        .world
        .query::<(&Transform, &Player, &LocalPlayer, Option<&Skinned>)>()
        .iter()
        .map(|(_, (t, p, _, s))| {
            let hand = s.and_then(|s| {
                if p.hand_joint == u32::MAX {
                    return None;
                }
                let idx = p.hand_joint as usize;
                s.joint_worlds.get(idx).map(|m| {
                    let local = m.col(3).truncate();
                    // joint_worlds are mesh-local; lift into
                    // world via the entity transform.
                    t.matrix().transform_point3(local)
                })
            });
            (t.position, p.aim_dir, hand)
        })
        .next();
    let local_active_ability = state.channel.active.map(|c| c.ability_id);
    let our_net_id = state.net.our_net_id_cached;

    // Snapshot enemy positions for client-side beam-corridor
    // hit detection (so we can spawn impact particles on every
    // pierced target). Mirrors the server-side logic in
    // `sim::channel::collect_hits` for `ChannelEffect::Beam`.
    use rift_engine::ecs::components::Enemy;
    let enemy_positions: Vec<Vec3> = state
        .world
        .query::<(&Transform, &Enemy)>()
        .iter()
        .map(|(_, (t, _))| t.position)
        .collect();

    // Drain a temporary list of indices to drop after the loop so
    // we can mutate `channel.visuals` while still holding `&mut
    // renderer`.
    let mut drop_indices: Vec<usize> = Vec::new();

    for (i, vis) in state.channel.visuals.iter_mut().enumerate() {
        // Pull both the authoritative beam math (`AbilityKind`)
        // and the visual recipe (`ShapeVisuals::Beam`) from the
        // single ability registry entry. Channels with a
        // non-Beam shape (Whirlwind aura, …) skip the beam
        // mesh path entirely.
        let ability_record = rift_game::abilities::lookup(vis.ability_id);
        let (beam_range, beam_corridor_width, pierce_targets) =
            match ability_record.map(|a| a.kind) {
                Some(rift_game::abilities::AbilityKind::Channel {
                    effect:
                        rift_game::abilities::ChannelEffect::Beam {
                            range,
                            width,
                            pierce_targets,
                            ..
                        },
                    ..
                }) => (range, width, pierce_targets),
                _ => (0.0, 0.0, 0),
            };
        // Beam visual recipe: VFX preset + hand-offset
        // fallback when no skinned hand-joint is available.
        let beam_visual = ability_record.and_then(|a| match a.visuals.shape {
            ShapeVisuals::Beam { effect, hand_offset } => Some((effect, hand_offset)),
            _ => None,
        });

        // Hide-and-drop path: ending flag set by `clear_channel_visual`
        // or idle timeout exceeded.
        vis.idle += dt;
        // Local-cast-just-released path: when our local channel
        // stops, the server's `ChannelEnd` (which would set
        // `vis.ending`) takes a network round-trip to arrive.
        // Without this short-circuit, the in-between frames
        // would lose `hand_override` (because `is_local` reads
        // `local_active_ability` which is already `None`),
        // re-anchor the beam to the chest fallback, and we'd
        // see the beam visibly teleport to the torso for a
        // frame before despawning. Detecting "this visual is
        // ours but our local channel has already stopped"
        // collapses that flicker into a clean immediate fade.
        let local_release_pending = our_net_id
            .map(|id| id == vis.caster)
            .unwrap_or(false)
            && local_active_ability != Some(vis.ability_id);
        if local_release_pending {
            vis.ending = true;
        }
        let expired = vis.ending || vis.idle > IDLE_TIMEOUT;

        // Resolve the caster: prefer matching to a known
        // remote-player avatar by net id; if no remote matches
        // (and we're channeling locally) treat the visual as
        // belonging to us. This keeps remote and local beams
        // visually consistent even if both happen at once.
        use rift_engine::ecs::components::RemotePlayer;
        let remote_data = state
            .world
            .query::<(&Transform, &Player, &RemotePlayer, Option<&Skinned>)>()
            .iter()
            .find(|(_, (_, _, rp, _))| rp.net_id == vis.caster.0)
            .map(|(_, (t, p, _, s))| {
                let hand = s.and_then(|s| {
                    if p.hand_joint == u32::MAX {
                        return None;
                    }
                    let idx = p.hand_joint as usize;
                    s.joint_worlds.get(idx).map(|m| {
                        let local = m.col(3).truncate();
                        t.matrix().transform_point3(local)
                    })
                });
                (t.position, p.aim_dir, hand)
            });
        let is_local =
            remote_data.is_none() && local_active_ability == Some(vis.ability_id);
        let mut hand_override: Option<Vec3> = None;
        if let Some((pos, aim, hand)) = remote_data {
            // Remote caster: anchor the beam to their hand
            // joint and pull pos/aim from the live (interpolated)
            // transform instead of the stale `ChannelTick`
            // payload, so the beam doesn't visibly trail the
            // body while they move.
            vis.position = pos;
            if aim.length_squared() > 1e-6 {
                vis.aim = Vec3::new(aim.x, 0.0, aim.z).normalize_or_zero();
            }
            hand_override = hand;
        } else if is_local {
            if let Some((pos, aim, hand)) = local_live {
                vis.position = pos;
                if aim.length_squared() > 1e-6 {
                    vis.aim = Vec3::new(aim.x, 0.0, aim.z).normalize_or_zero();
                }
                hand_override = hand;
                // Heartbeat the idle timer so we don't fade out
                // between server ticks.
                vis.idle = 0.0;
            }
        }
        let _ = is_local;

        // Skip non-beam channels (Whirlwind etc.); just let them
        // age out without spawning a mesh. Determined by the
        // ability's declared `ShapeVisuals` rather than by
        // wire id.
        let Some((beam_vfx, beam_hand_offset)) = beam_visual else {
            if expired {
                drop_indices.push(i);
            }
            continue;
        };
        if beam_range <= 0.0 {
            if expired {
                drop_indices.push(i);
            }
            continue;
        }

        // Compute beam endpoints first — we need them whether
        // we're spawning a fresh effect, updating an existing
        // one, or zeroing one out for despawn.
        let origin = hand_override.unwrap_or_else(|| vis.position + Vec3::Y * beam_hand_offset);
        let dir = if vis.aim.length_squared() > 1e-6 {
            vis.aim.normalize()
        } else {
            Vec3::Z
        };
        let ray = Ray {
            origin,
            direction: dir,
        };
        let length = match physics::raycast(&ray, beam_range, &state.floor.wall_aabbs) {
            Some(hit) => hit.distance.max(0.05),
            None => beam_range,
        };
        let tip = origin + dir * length;

        if expired {
            if let Some(id) = vis.vfx_id.take() {
                renderer.vfx_system.despawn(id);
            }
            drop_indices.push(i);
            continue;
        }

        // Lazy spawn the declarative VFX effect on first
        // frame; subsequent frames just push fresh endpoints.
        // Effect comes from the ability's `ShapeVisuals::Beam`
        // recipe — adding a new beam ability requires no
        // changes here, only a new `VfxKind` arm in
        // `effect_for_vfx`.
        if vis.vfx_id.is_none() {
            let id = renderer
                .vfx_system
                .spawn_bundle(rift_engine::combat::effect_for_vfx(beam_vfx), origin);
            vis.vfx_id = Some(id);
        }
        // Suppress unused warning when the variant is `None`
        // and `effect_for_vfx` returned an empty effect.
        let _ = VfxKind::None;
        if let Some(id) = vis.vfx_id {
            renderer.vfx_system.set_endpoints(id, origin, tip);
            // Anchor drives the per-frame spawn position of the
            // hand-base swirl layer; gameplay owns the hand
            // joint, so we push it every frame.
            renderer.vfx_system.set_anchor(id, origin);
        }

        // ---- Impact bursts at every pierced enemy + the
        // terminal point. Cadence-gated so we don't spew
        // hundreds of particles per second.
        vis.impact_acc += dt;
        if vis.impact_acc >= IMPACT_INTERVAL {
            vis.impact_acc = 0.0;

            // Replicate the server's beam-corridor hit math so
            // visuals match what's actually being damaged.
            // Right vector in XZ plane (rotate aim 90°).
            let right = Vec3::new(dir.z, 0.0, -dir.x);
            let mut hits: Vec<(f32, Vec3)> = Vec::new();
            for ep in &enemy_positions {
                let to = Vec3::new(ep.x - origin.x, 0.0, ep.z - origin.z);
                let along = to.dot(dir);
                if along < 0.0 || along > length {
                    continue;
                }
                if to.dot(right).abs() > beam_corridor_width {
                    continue;
                }
                hits.push((along, *ep));
            }
            hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let cap = (pierce_targets as usize).saturating_add(1);
            hits.truncate(cap);

            for (_along, pos) in &hits {
                // Centre on the enemy's torso, not their feet.
                let burst_pos = *pos + Vec3::Y * 0.9;
                renderer.vfx_system.spawn(
                    rift_engine::renderer::vfx::presets::frost_impact(),
                    burst_pos,
                );
            }

            // Terminal-point burst: when the beam clipped a wall
            // (length < beam_range) or pierced through everything
            // and reached max range, sparkle at the tip.
            let clipped = length + 0.01 < beam_range;
            if clipped || hits.len() < cap {
                renderer
                    .vfx_system
                    .spawn(rift_engine::renderer::vfx::presets::frost_impact(), tip);
            }
        }
    }

    // Remove expired entries (back-to-front so earlier indices
    // stay valid).
    for &i in drop_indices.iter().rev() {
        state.channel.visuals.swap_remove(i);
    }
}

/// Handle a `WorldEvent::Death` for a remote avatar.
///
/// Plays the death animation on the avatar: cancel any
/// in-flight upper-body cast, zero velocity, clear
/// `Player.action`, and cross-fade the body animator to the
/// first matching death clip on the rig. Idempotent — calling
/// it again on an avatar that's already in its death pose is a
/// no-op cross-fade. The local player's death clip is driven
/// from `trigger_player_death` instead (catch-all health
/// detect), so the binary skips this path when the dying
/// `NetId` is our own.
pub fn on_remote_death(world: &mut hecs::World, entity: hecs::Entity) {
    use rift_engine::animation::Animator;
    use rift_engine::ecs::components::{
        AnimationSet, Health, PlayerAction, SpellCast, Velocity,
    };

    let candidates: &[&str] = &["Death01", "Death_01", "Death", "Death02", "Death_02"];
    let clip = match world.get::<&AnimationSet>(entity) {
        Ok(set) => set.find_any(candidates),
        Err(_) => None,
    };
    let Some(clip) = clip else {
        log::warn!("Death animation not found in remote player's clip set");
        return;
    };

    // Match `trigger_player_death`'s SpellCast reset: not just
    // `cancel()` (which moves to Exiting and lets the layer fade
    // out over time), but a hard zero so the upper-body cast pose
    // can't bleed into the death cross-fade.
    if let Ok(mut cast) = world.get::<&mut SpellCast>(entity) {
        cast.phase = rift_engine::ecs::components::SpellPhase::Idle;
        cast.layer_animator = None;
        cast.weight = 0.0;
        cast.pending_oneshot = None;
        cast.oneshot_is_hit = false;
    }
    if let Ok(mut anim) = world.get::<&mut Animator>(entity) {
        anim.cross_fade(clip, false, 0.18);
        anim.speed = 1.0;
    }
    if let Ok(mut vel) = world.get::<&mut Velocity>(entity) {
        vel.linear = Vec3::ZERO;
    }
    if let Ok(mut p) = world.get::<&mut Player>(entity) {
        p.action = PlayerAction::None;
        p.action_timer = 0.0;
        p.vy = 0.0;
        p.airborne = false;
    }
    // Belt-and-braces: stamp Health to zero so `locomotion_anim_system`'s
    // `is_dead()` gate is true on the very next frame, even if the
    // snapshot mirror in `sync_avatars` lags by a tick. Otherwise
    // locomotion can briefly cross-fade Idle/Walk over the death
    // pose before the next snapshot arrives.
    if let Ok(mut h) = world.get::<&mut Health>(entity) {
        h.current = 0.0;
    }
    log::info!("Remote player death animation triggered.");
}
