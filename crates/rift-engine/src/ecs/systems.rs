use glam::{Quat, Vec3};
use hecs::World;

use super::components::{
    AnimationSet, Attack, Boss, Collider, Dying, Elite, Enemy, EnemyAnim, Health, LocalPlayer,
    Player, Renderable, Skinned, Transform, Velocity,
};
use crate::animation::{self, Animator};
use crate::input::Input;
use crate::renderer::Renderer;
use rift_math::physics::{self, Aabb, Ray};

/// Process player input and set velocity based on WASD keys.
pub fn player_input_system(world: &mut World, input: &Input, dt: f32) {
    for (_id, (transform, velocity, player, health, ghost, _local)) in world.query_mut::<(
        &Transform,
        &mut Velocity,
        &Player,
        Option<&super::components::Health>,
        Option<&super::components::Ghost>,
        &LocalPlayer,
    )>() {
        // Dead players don't move — freeze velocity so the death
        // animation plays at their final position. Ghosts (HP still
        // 0 but server-permitted to move) bypass this gate.
        if let Some(h) = health {
            if h.is_dead() && ghost.is_none() {
                velocity.linear = Vec3::ZERO;
                continue;
            }
        }
        // Full-body actions (rolls, jumps, lands) own the velocity for
        // their duration. The game-side state machine drives velocity
        // for `Roll`; `JumpStart` / `JumpLand` lock movement entirely;
        // `JumpAir` permits limited air control below.
        match player.action {
            super::components::PlayerAction::Roll
            | super::components::PlayerAction::JumpStart
            | super::components::PlayerAction::JumpLand => {
                continue;
            }
            _ => {}
        }
        let mut dir = Vec3::ZERO;

        if input.is_key_held(winit::keyboard::KeyCode::KeyW) {
            dir.z -= 1.0;
        }
        if input.is_key_held(winit::keyboard::KeyCode::KeyS) {
            dir.z += 1.0;
        }
        if input.is_key_held(winit::keyboard::KeyCode::KeyA) {
            dir.x -= 1.0;
        }
        if input.is_key_held(winit::keyboard::KeyCode::KeyD) {
            dir.x += 1.0;
        }

        // Get camera-relative direction
        let cam_yaw = input.camera_yaw();
        let rot = Quat::from_rotation_y(cam_yaw);
        let move_dir = rot * dir;

        if move_dir.length_squared() > 0.0 {
            // In the air the player keeps lateral velocity but at reduced
            // authority so jumps feel committed.
            let air_factor = if matches!(player.action, super::components::PlayerAction::JumpAir,) {
                0.85
            } else {
                1.0
            };
            let new_xz = move_dir.normalize() * player.speed * air_factor;
            velocity.linear.x = new_xz.x;
            velocity.linear.z = new_xz.z;
            // Face movement direction
            let _ = transform; // We'll update rotation below in movement system
        } else if !matches!(player.action, super::components::PlayerAction::JumpAir) {
            velocity.linear.x = 0.0;
            velocity.linear.z = 0.0;
        }

        let _ = dt;
    }
}

/// Apply velocity to transform.
///
/// `floor` is the dungeon's analytic heightfield used for
/// per-tile ground follow + step-up resolution. When `None`
/// (e.g. menu / loading), we fall back to a y=0 plane.
pub fn movement_system(world: &mut World, dt: f32, floor: Option<&rift_dungeon::Floor>) {
    // Players need vertical-velocity integration (jumping). Other
    // entities stay flat on y=0.
    const GRAVITY: f32 = 22.0;

    // Ground sample: just the tile under the player's centre.
    //
    // Earlier iterations took the max over a 5-tap capsule
    // footprint (centre + 4 cardinal radii) on the theory that
    // it would pre-emptively lift the body the moment the
    // capsule edge overlapped a higher neighbouring tile.
    // That's correct in theory for arbitrary geometry, but
    // wrong for *this* tile grid:
    //
    //   * `tile_floor_y_at` returns 0.0 for walls and OOB
    //     samples, so any capsule tap that grazes a wall
    //     pulls the result up to 0 even when the body is
    //     standing in a sunken pit at -0.5. The body snaps
    //     up by half a metre, the visible mesh ends up half
    //     buried in the actual floor, and the IK pass —
    //     using the same elevated reference plane — can't
    //     recover.
    //   * Vertical transitions between connected tiles are
    //     already handled by stair tiles, which interpolate
    //     their own floor height. The locomotion layer
    //     doesn't need a second mechanism on top.
    //
    // Single-tap is the correct primitive for tile-grid
    // navigation: ground = floor of the tile under the
    // capsule centre, full stop. If a step-up across a hard
    // ledge is ever needed, it should be implemented as an
    // explicit "if destination tile is higher and Δ ≤
    // step_height, snap up" rule, not as a side-effect of a
    // max over neighbouring samples.
    let support_y = |x: f32, z: f32| -> f32 {
        match floor {
            Some(f) => f.tile_floor_y_at(x, z),
            None => 0.0,
        }
    };

    for (_id, (transform, velocity, player, net)) in world.query_mut::<(
        &mut Transform,
        &mut Velocity,
        &mut Player,
        Option<&super::components::NetControlled>,
    )>() {
        // Networked players: horizontal motion is owned by the
        // net client's prediction loop, but we still let vertical
        // jump physics run locally (Phase 4.2 doesn't sync jumps).
        let net_controlled = net.is_some();
        // Net-controlled players: kinematic owns full XYZ
        // including ground follow and gravity. Skipping the
        // movement integrator entirely for them avoids two
        // separate Y solvers fighting (one snapping to 0, the
        // other tracking per-tile elevation), which manifested
        // as the avatar hovering above sunken-pit floors.
        if net_controlled {
            continue;
        }
        // Horizontal motion (XZ).
        let horiz = Vec3::new(velocity.linear.x, 0.0, velocity.linear.z);
        if horiz.length_squared() > 0.001 {
            transform.position += horiz * dt;
            let target_yaw = horiz.x.atan2(horiz.z);
            let target_rot = Quat::from_rotation_y(target_yaw);
            transform.rotation = transform.rotation.slerp(target_rot, (dt * 10.0).min(1.0));
        }
        // Ground height under the (post-XZ-step) capsule.
        let ground_y = support_y(transform.position.x, transform.position.z);
        // Expose the authoritative grounded plane so foot IK
        // can use it as its reference even while
        // `transform.position.y` is mid-lerp catching up after
        // a sudden lift onto a raised tile.
        player.grounded_y = ground_y;
        // Vertical motion (gravity + jump). Integrated at a
        // FIXED 120 Hz substep, with leftover frame time banked
        // in `player.vy_accum`. With variable frame dt the
        // direct `position.y += vy * dt` form produced visible
        // micro-stutter on the ~0.4 s jump arc — the per-frame
        // delta would noticeably overshoot/undershoot whenever
        // dt drifted (vsync hiccups, streaming spikes), giving
        // the impression of "screen shake" or jitter while
        // airborne. Substepping makes the trajectory
        // deterministic regardless of render rate; any leftover
        // (sub-step) time is consumed on the next frame.
        const FIXED_DT: f32 = 1.0 / 120.0;
        let above_ground = transform.position.y - ground_y;
        if player.airborne || player.vy.abs() > 0.001 || above_ground > 0.001 {
            player.vy_accum += dt;
            // Cap the catch-up so a single-frame stall (debugger
            // pause, alt-tab) doesn't dump a whole second of
            // gravity into one frame.
            if player.vy_accum > 0.25 {
                player.vy_accum = 0.25;
            }
            while player.vy_accum >= FIXED_DT {
                player.vy -= GRAVITY * FIXED_DT;
                transform.position.y += player.vy * FIXED_DT;
                if transform.position.y <= ground_y {
                    transform.position.y = ground_y;
                    player.vy = 0.0;
                    player.airborne = false;
                    player.vy_accum = 0.0;
                    break;
                } else {
                    player.airborne = true;
                }
                player.vy_accum -= FIXED_DT;
            }
        } else {
            // Glued to ground: smoothly chase the per-tile
            // support height instead of snapping. The ground
            // sample (a max over four cardinal capsule taps)
            // changes discontinuously by `ELEVATION_STEP` the
            // moment the capsule edge crosses a ledge — if we
            // wrote that straight into the transform the body
            // would teleport up half a metre. Exponential
            // chase with a short tau gives a perceptually
            // continuous lift while still settling fast
            // enough that the avatar reads as "on the
            // platform" within a few frames.
            //
            // Big jumps (e.g. spawn / floor-load / portal)
            // skip the smoothing — anything more than a
            // metre is treated as a teleport and snapped.
            const Y_SMOOTH_TAU: f32 = 0.12; // ~95 % converged in 0.36 s
            const Y_TELEPORT_THRESHOLD: f32 = 1.5;
            let delta = ground_y - transform.position.y;
            if delta.abs() > Y_TELEPORT_THRESHOLD {
                transform.position.y = ground_y;
            } else {
                let alpha = 1.0 - (-(dt / Y_SMOOTH_TAU)).exp();
                transform.position.y += delta * alpha;
            }
            player.airborne = false;
            player.vy = 0.0;
            player.vy_accum = 0.0;
        }
    }

    for (_id, (transform, velocity, enemy, player)) in world.query_mut::<(
        &mut Transform,
        &Velocity,
        Option<&super::components::Enemy>,
        Option<&Player>,
    )>() {
        // Skip players — already handled above.
        if player.is_some() {
            continue;
        }
        if velocity.linear.length_squared() > 0.001 {
            transform.position += velocity.linear * dt;
            // Keep entities on the ground plane. The skinned glTF character
            // is authored with its origin at the feet, so y=0 sits on the
            // floor (procedural cube meshes are centered, see movement of
            // those entities elsewhere if they need a different offset).
            transform.position.y = support_y(transform.position.x, transform.position.z);
            // Smoothly rotate to face movement direction — but only for
            // the player and other non-enemy movers.  Enemies have their
            // facing controlled by the AI system (so they keep looking at
            // the player even when backpedaling), and we don't want to
            // overwrite that here.
            if enemy.is_none() {
                let target_yaw = velocity.linear.x.atan2(velocity.linear.z);
                let target_rot = Quat::from_rotation_y(target_yaw);
                transform.rotation = transform.rotation.slerp(target_rot, (dt * 10.0).min(1.0));
            }
        }
    }
}

/// Sync ECS transforms to renderer objects (dynamic entities only).
pub fn render_sync_system(world: &World, renderer: &mut Renderer) {
    for (_id, (transform, renderable, _vel)) in
        world.query::<(&Transform, &Renderable, &Velocity)>().iter()
    {
        if renderable.object_index < renderer.objects.len() {
            renderer.objects[renderable.object_index].model_matrix = transform.matrix();
        }
    }
}

/// For every entity that has both an `AnimationSet` and an `Animator`, pick
/// the appropriate clip based on locomotion state and cross-fade into it.
/// Three-tier locomotion: Idle → Walk → Jog → Sprint, with playback speed
/// scaled to match world speed (no foot-sliding).
pub fn locomotion_anim_system(world: &mut World) {
    // Reference world speeds the animations were authored at, and the speed
    // thresholds for promoting one tier to the next. Tuned by eye against
    // the UAL pack — adjust as needed.
    const WALK_REF_SPEED: f32 = 1.5; // m/s for Walk_Loop
    const JOG_REF_SPEED: f32 = 3.5; // m/s for Jog_Fwd
    const SPRINT_REF_SPEED: f32 = 6.0; // m/s for Sprint
    const WALK_JOG_THRESHOLD: f32 = 2.0;
    const JOG_SPRINT_THRESHOLD: f32 = 4.5;
    const STILL_THRESHOLD_SQ: f32 = 0.04;
    const FADE_LOCOMOTION: f32 = 0.18;
    const FADE_GAIT: f32 = 0.12;

    const SPRINT_NAMES: &[&str] = &[
        "Sprint_Loop",
        "Sprint",
        "Sprint_Fwd",
        "Sprint_Forward_Loop",
        "Run_Loop",
        "Run", // some packs only ship "Run"
    ];
    const JOG_NAMES: &[&str] = &[
        "Jog_Fwd",
        "Jog_Forward",
        "Jog_Forward_Loop",
        "Jog_Loop",
        "Jog",
        "Run_Loop",
        "Run", // jog as a fallback for run-less packs
    ];
    const WALK_NAMES: &[&str] = &["Walk_Loop", "Walk", "Walk_Fwd", "Walk_Forward_Loop"];
    const IDLE_NAMES: &[&str] = &["Idle_Loop", "Idle"];

    for (_id, (vel, set, animator, enemy_anim, health, player, ghost)) in world.query_mut::<(
        &Velocity,
        &AnimationSet,
        &mut Animator,
        Option<&EnemyAnim>,
        Option<&super::components::Health>,
        Option<&Player>,
        Option<&super::components::Ghost>,
    )>() {
        // Skip locomotion overrides while a one-shot reaction (Death,
        // HitRecieve, Bite_Front) is locked.
        if let Some(ea) = enemy_anim {
            if ea.lock_remaining > 0.0 {
                continue;
            }
        }
        // Dead players: leave the death clip running, don't snap back to
        // Idle/Walk. Ghosts (risen-but-dead spectators) bypass this:
        // their HP is still 0 but they're animated like a live player.
        if let Some(h) = health {
            if h.is_dead() && ghost.is_none() {
                continue;
            }
        }
        // Skip while the player is mid-jump or mid-roll — those clips
        // are driven by game-side code.
        if let Some(p) = player {
            if !matches!(p.action, super::components::PlayerAction::None) {
                continue;
            }
        }
        let speed = vel.linear.length();
        let moving = vel.linear.length_squared() > STILL_THRESHOLD_SQ;

        let sprint_clip = set.find_any(SPRINT_NAMES);
        let jog_clip = set.find_any(JOG_NAMES);
        let walk_clip = set.find_any(WALK_NAMES).or_else(|| jog_clip.clone());
        let idle_clip = set.find_any(IDLE_NAMES);

        let (want_clip, target_speed_mult) = if !moving {
            (idle_clip, 1.0)
        } else if speed >= JOG_SPRINT_THRESHOLD && sprint_clip.is_some() {
            (sprint_clip, (speed / SPRINT_REF_SPEED).clamp(0.7, 1.5))
        } else if speed >= WALK_JOG_THRESHOLD && jog_clip.is_some() {
            (jog_clip, (speed / JOG_REF_SPEED).clamp(0.6, 1.6))
        } else {
            (walk_clip, (speed / WALK_REF_SPEED).clamp(0.5, 1.7))
        };

        if let Some(clip) = want_clip {
            let switching_clips = !std::sync::Arc::ptr_eq(&animator.clip, &clip);
            if switching_clips {
                let was_idle = animator.clip.name.to_ascii_lowercase().contains("idle");
                let going_idle = clip.name.to_ascii_lowercase().contains("idle");
                let fade = if was_idle || going_idle {
                    FADE_LOCOMOTION
                } else {
                    FADE_GAIT
                };
                animator.cross_fade(clip, true, fade);
            }
            animator.speed = target_speed_mult;
        }
    }
}

/// Drive one-shot reaction animations on enemies: `Death` when the
/// entity becomes `Dying`, `HitRecieve` (note: misspelled in the asset
/// pack) on health drops, and `Bite_Front` while in melee contact with
/// the player. Should run AFTER `locomotion_anim_system` so it can
/// override the chosen locomotion clip with the reaction.
pub fn enemy_anim_system(world: &mut World, dt: f32) {
    const HIT_LOCK: f32 = 0.55; // length-ish of HitRecieve clips
    const BITE_LOCK: f32 = 0.65; // length-ish of Bite_Front clips
    const DEATH_LOCK: f32 = 999.0; // hold Death pose until despawn
    const FADE: f32 = 0.10;

    for (_id, (set, animator, health, ea, dying)) in world.query_mut::<(
        &AnimationSet,
        &mut Animator,
        &Health,
        &mut EnemyAnim,
        Option<&Dying>,
    )>() {
        // Tick the lock timer.
        if ea.lock_remaining > 0.0 {
            ea.lock_remaining = (ea.lock_remaining - dt).max(0.0);
        }

        // Death: triggered the moment the entity becomes Dying. We treat
        // a positive `dying` reference as the trigger and use the lock
        // to ensure we only swap once.
        if dying.is_some() && ea.lock_remaining < DEATH_LOCK * 0.5 {
            if let Some(clip) = set.find_any(&["Death"]) {
                animator.cross_fade(clip, false, FADE);
                animator.speed = 1.0;
                ea.lock_remaining = DEATH_LOCK;
            }
            // Don't process hit/bite once dead.
            ea.last_hp = health.current;
            ea.attacking = false;
            continue;
        }

        // HitRecieve: detect a health drop since last frame.
        let took_damage = health.current < ea.last_hp - 0.001;
        ea.last_hp = health.current;
        if took_damage && ea.lock_remaining <= 0.0 {
            if let Some(clip) = set.find_any(&["HitRecieve", "HitReceive", "Hit"]) {
                animator.cross_fade(clip, false, FADE);
                animator.speed = 1.0;
                ea.lock_remaining = HIT_LOCK;
                ea.attacking = false;
                continue;
            }
        }

        // Bite_Front: gameplay code sets `attacking = true` whenever the
        // enemy is currently overlapping the player.
        if ea.attacking && ea.lock_remaining <= 0.0 {
            if let Some(clip) = set.find_any(&["Bite_Front", "Bite", "Attack"]) {
                animator.cross_fade(clip, false, FADE);
                animator.speed = 1.0;
                ea.lock_remaining = BITE_LOCK;
            }
        }
        // Always reset attacking — gameplay re-asserts it each frame
        // while contact is active.
        ea.attacking = false;
    }
}

/// Advance every base Animator (and any active SpellCast layer) and re-skin
/// each mesh into the renderer's per-frame dynamic vertex buffer.
///
/// `floor` (when supplied) drives terrain-aware foot IK on the
/// local player: each foot's vertical position is sampled
/// against the dungeon's per-tile elevation grid and the foot
/// bone in the bone palette is translated by the delta so that
/// the foot plants on raised daises, sunken pits, and stair
/// tile slopes instead of floating on the baked clip's
/// flat-ground assumption. Pass `None` for menus / hub / any
/// surface where the flat-ground assumption holds.
pub fn skinning_system(
    world: &mut World,
    renderer: &mut Renderer,
    dt: f32,
    floor: Option<&rift_dungeon::Floor>,
) {
    // CPU skinning + clip sampling is the most expensive per-frame work
    // for skinned characters. We always process the player, but for any
    // other skinned entity we bail out early when:
    //   1. the entity is farther than `SKIN_RADIUS` from the camera, OR
    //   2. its bounding sphere is outside the view frustum.
    // In those cases the dynamic vertex buffer keeps its last-uploaded
    // pose, which is correct because the entity isn't visible anyway.
    const SKIN_RADIUS: f32 = 22.0;
    let skin_radius_sq = SKIN_RADIUS * SKIN_RADIUS;
    let cam_pos = renderer.camera.position;
    let frustum = renderer.camera.frustum_planes();

    let mut palette: Vec<glam::Mat4> = Vec::new();
    for (_id, (renderable, skinned, animator, transform, mut cast, player, atts, foot_ik)) in world
        .query_mut::<(
            &Renderable,
            &mut Skinned,
            &mut Animator,
            &Transform,
            Option<&mut super::components::SpellCast>,
            Option<&Player>,
            Option<&mut super::components::SkinnedAttachments>,
            Option<&mut super::components::FootIkState>,
        )>()
    {
        let is_player = player.is_some();
        if !is_player {
            // Distance cull.
            let dx = transform.position.x - cam_pos.x;
            let dz = transform.position.z - cam_pos.z;
            if dx * dx + dz * dz > skin_radius_sq {
                continue;
            }
            // Frustum cull. Use a generous radius so loose-fitting bind
            // poses aren't rejected at silhouettes.
            if !renderer
                .camera
                .sphere_in_frustum(&frustum, transform.position, 2.0)
            {
                continue;
            }
        }

        animator.advance(dt);
        if animator.clip.joint_count != skinned.mesh.joints.len() {
            continue; // mismatch — skip skinning, render bind pose
        }
        // Advance the cast layer animator (if any) and pick up its weight/mask.
        let (layer_anim, layer_mask, layer_weight): (Option<&Animator>, &[f32], f32) =
            if let Some(c) = cast.as_deref_mut() {
                if let Some(la) = c.layer_animator.as_mut() {
                    la.advance(dt);
                }
                let weight = c.weight;
                let mask: &[f32] = if weight > 0.001 { &c.mask } else { &[] };
                (c.layer_animator.as_ref(), mask, weight)
            } else {
                (None, &[], 0.0)
            };

        // Compute torso twist: difference between aim yaw and body yaw,
        // clamped so we never twist past ~120° (rig would tear apart).
        let twist = if let Some(p) = player {
            let aim = p.aim_dir;
            if aim.length_squared() > 1e-4 {
                let aim_yaw = aim.x.atan2(aim.z);
                let fwd = transform.rotation * Vec3::Z;
                let body_yaw = fwd.x.atan2(fwd.z);
                let mut delta = aim_yaw - body_yaw;
                while delta > std::f32::consts::PI {
                    delta -= std::f32::consts::TAU;
                }
                while delta < -std::f32::consts::PI {
                    delta += std::f32::consts::TAU;
                }
                let limit = std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_6; // ~120°
                let clamped = delta.clamp(-limit, limit);
                if p.spine_joint != u32::MAX {
                    Some((p.spine_joint as usize, clamped))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if layer_anim.is_some() && layer_weight > 0.001 || twist.is_some() {
            animation::build_bone_palette_layered(
                animator,
                layer_anim,
                layer_mask,
                layer_weight,
                twist,
                &skinned.mesh.joints,
                &mut palette,
                Some(&mut skinned.joint_worlds),
            );
        } else {
            animation::build_bone_palette(
                animator,
                &skinned.mesh.joints,
                &mut palette,
                Some(&mut skinned.joint_worlds),
            );
        }

        // Terrain-aware foot IK pass. Runs after the bone
        // palette has been built from animation; corrects each
        // foot to the dungeon's per-tile elevation while
        // preserving the swing-phase arc of the baked clip.
        // Skipped for entities without a `FootIkState` component
        // and on floors with no elevation features (the cost
        // dominated by the per-foot ground sample is negligible
        // either way at single-character scale).
        if let (Some(p), Some(floor), Some(ik)) = (player, floor, foot_ik) {
            let host_xform = transform.matrix();
            let foot_l = if p.foot_l_joint == u32::MAX {
                None
            } else {
                Some(p.foot_l_joint as usize)
            };
            let foot_r = if p.foot_r_joint == u32::MAX {
                None
            } else {
                Some(p.foot_r_joint as usize)
            };
            crate::foot_ik::apply_foot_ik(
                &skinned.mesh.joints,
                &mut skinned.joint_worlds,
                &mut palette,
                &host_xform,
                p.grounded_y,
                foot_l,
                foot_r,
                &floor,
                ik,
                dt,
            );
        }
        // Skin the base body every frame. We never hide it under outfits:
        // the outfit pieces are inflated slightly along their normals so
        // they sit just outside the base skin (no z-fighting), and any
        // body parts the outfit doesn't cover (head, neck, hands when
        // there are no gloves) still render correctly.
        // Hand the freshly built bone palette to the GPU skinner.
        // The compute shader will run once per visible skinned mesh
        // before the shadow pass and write the deformed vertices to
        // a device-local buffer the graphics pipelines bind.
        renderer.update_palette(renderable.object_index, &palette);

        // Re-skin attached outfit pieces with the same palette. They were
        // remapped at load time so their joint indices reference the host
        // skeleton's palette directly.
        if let Some(atts) = atts {
            let host_xform = transform.matrix();
            for piece in &mut atts.pieces {
                if piece.object_index >= renderer.objects.len() {
                    continue;
                }
                if !piece.visible {
                    renderer.objects[piece.object_index].model_matrix = glam::Mat4::ZERO;
                    continue;
                }
                renderer.objects[piece.object_index].model_matrix = host_xform;
                // Outfit shells share the body's bone palette — the
                // per-piece `inflate` flag is baked into each mesh's
                // `SkinningSystem` registration at load time, so we
                // just upload the same palette here.
                renderer.update_palette(piece.object_index, &palette);
            }
        }
    }
}

/// Advance the spell-cast state machine. Returns the list of casts that
/// just transitioned into `Shooting` (i.e. the moment the projectile
/// should be spawned). The caller (gameplay code) can then walk the
/// returned list and emit projectiles at the player's hand position.
pub fn cast_advance_system(world: &mut World, dt: f32) -> Vec<(hecs::Entity, glam::Vec3, f32)> {
    use super::components::{SpellCast, SpellPhase};
    let mut fire_events: Vec<(hecs::Entity, glam::Vec3, f32)> = Vec::new();

    // Tunables.
    const WEIGHT_RAMP: f32 = 8.0; // 1/seconds — about 0.125s to reach full weight

    for (entity, (cast, set)) in world.query_mut::<(&mut SpellCast, &AnimationSet)>() {
        if cast.hit_cooldown > 0.0 {
            cast.hit_cooldown = (cast.hit_cooldown - dt).max(0.0);
        }
        if cast.phase == SpellPhase::Idle {
            cast.weight = (cast.weight - dt * WEIGHT_RAMP).max(0.0);
            continue;
        }

        // Cast layer clips:
        //   * Entering: wind-up clip raises the hand to the casting
        //     pose. Projectile / damage fires on the transition out
        //     of this phase, so the released visual lines up with
        //     the start of the Shoot clip.
        //   * Shooting: release clip (the hand snaps forward as
        //     the spell leaves it). Plays once, then transitions
        //     to Exiting.
        //   * Exiting: recovery clip back to neutral.
        //
        // Channels use a different clip stack so the body keeps a
        // sustained "casting" pose for the whole hold duration.
        let target_clip = match cast.phase {
            SpellPhase::Entering if cast.channeling => set.find_any(&[
                "Spell_Simple_Idle_Loop",
                "Spell_Idle_Loop",
                "Cast_Loop",
                "Channel_Loop",
                "Spell_Double_Shoot",
                "Spell_Simple_Enter",
                "Spell_Enter",
                "Cast_Enter",
            ]),
            SpellPhase::Entering => {
                set.find_any(&["Spell_Simple_Enter", "Spell_Enter", "Cast_Enter"])
            }
            SpellPhase::Shooting => set.find_any(&[
                "Spell_Simple_Shoot",
                "Spell_Shoot",
                "Cast_Shoot",
                "Spell_Simple_Enter",
            ]),
            SpellPhase::Exiting => set.find_any(&["Spell_Simple_Exit", "Spell_Exit", "Cast_Exit"]),
            SpellPhase::OneShot => cast.pending_oneshot.clone(),
            SpellPhase::Idle => None,
        };
        let Some(target_clip) = target_clip else {
            // Missing clip for this phase — fall through gracefully.
            match cast.phase {
                SpellPhase::Entering => {
                    if !cast.fired {
                        fire_events.push((entity, cast.pending_aim_dir, cast.pending_damage));
                        cast.fired = true;
                    }
                    cast.phase = SpellPhase::Shooting;
                }
                SpellPhase::Shooting => cast.phase = SpellPhase::Exiting,
                SpellPhase::Exiting | SpellPhase::OneShot => {
                    cast.phase = SpellPhase::Idle;
                    cast.layer_animator = None;
                    cast.pending_oneshot = None;
                    cast.oneshot_is_hit = false;
                }
                SpellPhase::Idle => {}
            }
            continue;
        };

        let need_swap = match cast.layer_animator.as_ref() {
            None => true,
            Some(la) => !std::sync::Arc::ptr_eq(&la.clip, &target_clip),
        };
        if need_swap {
            match cast.layer_animator.as_mut() {
                Some(la) => la.cross_fade(target_clip.clone(), false, 0.08),
                None => {
                    let mut la = Animator::new(target_clip.clone());
                    la.looping = false;
                    cast.layer_animator = Some(la);
                }
            }
        }
        // Channels keep the Enter clip looping until the player
        // releases the action button (or the server expires it).
        // We flag `looping` here every frame so a fresh cross-fade
        // doesn't reset it.
        if cast.channeling {
            if let Some(la) = cast.layer_animator.as_mut() {
                la.looping = true;
            }
        }

        let target_weight = if cast.phase == SpellPhase::Exiting {
            0.0
        } else {
            1.0
        };
        let dw = dt * WEIGHT_RAMP;
        cast.weight = if target_weight > cast.weight {
            (cast.weight + dw).min(target_weight)
        } else {
            (cast.weight - dw).max(target_weight)
        };
        // OneShot taper: in the last ~weight-ramp window before the clip
        // ends, smoothly bring the layer weight down so the upper body
        // settles back into the locomotion pose without a pop.
        if cast.phase == SpellPhase::OneShot {
            if let Some(la) = cast.layer_animator.as_ref() {
                let remaining = (la.clip.duration - la.time).max(0.0);
                let taper_window = 1.0 / WEIGHT_RAMP;
                if remaining < taper_window {
                    let t = (remaining / taper_window).clamp(0.0, 1.0);
                    cast.weight = cast.weight.min(t);
                }
            }
        }

        if let Some(la) = cast.layer_animator.as_ref() {
            let done = la.time >= la.clip.duration - 1e-3;
            if done {
                match cast.phase {
                    SpellPhase::Entering => {
                        // While channeling we never auto-advance —
                        // the Enter clip is looping and the layer
                        // sits at full weight until `cancel()` is
                        // called.
                        if cast.channeling {
                            // No-op: looping animator handles
                            // wraparound on its own.
                        } else {
                            // Fire at the end of the wind-up so the
                            // projectile leaves the hand at its
                            // highest, fully-extended pose. The
                            // Shoot clip plays next and visually
                            // sells the release.
                            if !cast.fired {
                                fire_events.push((
                                    entity,
                                    cast.pending_aim_dir,
                                    cast.pending_damage,
                                ));
                                cast.fired = true;
                            }
                            cast.phase = SpellPhase::Shooting;
                        }
                    }
                    SpellPhase::Shooting => cast.phase = SpellPhase::Exiting,
                    SpellPhase::Exiting => {
                        if cast.weight <= 0.001 {
                            cast.phase = SpellPhase::Idle;
                            cast.layer_animator = None;
                            cast.fired = false;
                            cast.channeling = false;
                        }
                    }
                    SpellPhase::OneShot => {
                        cast.phase = SpellPhase::Idle;
                        cast.layer_animator = None;
                        cast.pending_oneshot = None;
                        cast.oneshot_is_hit = false;
                        cast.weight = 0.0;
                    }
                    SpellPhase::Idle => {}
                }
            }
        }
    }

    fire_events
}

/// Make the camera follow the player with a third-person offset.
/// Pulls the camera forward if a wall is between the player and the camera.
pub fn camera_follow_system(
    world: &World,
    renderer: &mut Renderer,
    input: &Input,
    wall_aabbs: &[Aabb],
    dt: f32,
) {
    for (_id, (transform, _player, _local)) in world
        .query::<(&Transform, &Player, &super::components::LocalPlayer)>()
        .iter()
    {
        // Anchor the look-at on the player's torso. Vertical
        // motion is smoothed independently of XZ so jumps don't
        // drag the world up and down with the character — that
        // produced a visible "screen shake" each time the player
        // jumped, because the entire scene was being translated
        // by the same delta as the player sprite. With this
        // smoothing the player visibly leaves the ground while
        // the camera holds steady, which is the standard
        // top-down / 3rd-person ARPG behaviour.
        //
        // The XZ components track the player frame-perfect so
        // movement still feels responsive; only Y is damped.
        let target_xz = transform.position + Vec3::new(0.0, 0.8, 0.0);
        let prev_y = renderer.camera.target.y;
        // First-order exponential smoothing. `tau` is the time
        // constant (seconds to reach ~63% of the gap). 0.25 s
        // keeps the camera glued enough for floor / stair
        // changes to feel snappy while still hiding a typical
        // ~0.4 s jump arc almost entirely.
        const TAU: f32 = 0.25;
        let alpha = 1.0 - (-(dt.max(0.0) / TAU)).exp();
        let smoothed_y = if prev_y.is_finite() {
            prev_y + (target_xz.y - prev_y) * alpha
        } else {
            target_xz.y
        };
        let target = Vec3::new(target_xz.x, smoothed_y, target_xz.z);

        let yaw = input.camera_yaw();
        let pitch = input.camera_pitch();
        let distance = input.camera_distance();

        let offset = Vec3::new(
            distance * pitch.cos() * yaw.sin(),
            distance * pitch.sin(),
            distance * pitch.cos() * yaw.cos(),
        );

        let desired = target + offset;

        // We do NOT pull the camera in front of occluding
        // walls (that produced jolting zoom snaps every time
        // the player walked behind cover). Instead we cast a
        // ray from the camera toward the player and ease a
        // strength scalar (`wall_xray_strength`) toward 1.0
        // while a wall occludes / 0.0 otherwise. The cel
        // shader reads that scalar and uses it as a fade
        // multiplier on the porthole, so transitions in and
        // out of cover fade over a few frames instead of
        // popping the instant the camera ray clips a wall.
        // Raycast from the camera past the player by a few
        // metres. The extension is what makes the locked-pitch
        // top-down camera readable when the player walks up
        // to a wall: with no extension, only walls strictly
        // between camera and player carve, and a wall directly
        // *in front of* the player (e.g. a corridor end) would
        // hide the player's torso under it on screen even
        // though the player is technically "in front of" the
        // wall along the camera ray. Extending the raycast
        // (and the shader's `tFrag` allowance) ~3 m past the
        // player picks up those near-front walls and lets the
        // porthole open through them too.
        const XRAY_LOOKAHEAD: f32 = 12.0;
        let cam_to_player = target - desired;
        let dist_to_player = cam_to_player.length();
        let extended_target = if dist_to_player > 1e-3 {
            target + cam_to_player.normalize() * XRAY_LOOKAHEAD
        } else {
            target
        };
        let (ray, ray_len) = Ray::between(desired, extended_target);
        let occluded = physics::raycast(&ray, ray_len, wall_aabbs).is_some();
        let target_strength = if occluded { 1.0 } else { 0.0 };
        // First-order exponential ease. tau = 0.12 s gives a
        // ~85% transition in 240 ms — fast enough that the
        // porthole feels responsive when the camera swings
        // around a corner, slow enough that the appear/disappear
        // is a clear fade rather than a hard pop.
        const XRAY_TAU: f32 = 0.12;
        let alpha_x = 1.0 - (-(dt.max(0.0) / XRAY_TAU)).exp();
        let prev = renderer.wall_xray_strength;
        let smoothed = if prev.is_finite() {
            prev + (target_strength - prev) * alpha_x
        } else {
            target_strength
        };
        renderer.wall_xray_strength = smoothed.clamp(0.0, 1.0);
        let actual_pos = desired;

        renderer.camera.position = actual_pos;
        renderer.camera.target = target;
    }
}

/// Resolve collisions between dynamic entities (player) and static colliders (walls).
/// Uses AABB overlap + minimum penetration push-out.
pub fn collision_system(world: &mut World, wall_colliders: &[(Vec3, Collider)]) {
    // Resolve dynamic entities (those with Velocity) against statics
    for (_id, (transform, collider, _vel, net)) in world.query_mut::<(
        &mut Transform,
        &Collider,
        &mut Velocity,
        Option<&super::components::NetControlled>,
    )>() {
        // Networked players: collision is server-authoritative,
        // resolved by the prediction sim against the dungeon grid.
        if net.is_some() {
            continue;
        }
        let pos = transform.position;
        let (dyn_min, dyn_max) = collider.bounds(pos);

        for &(static_pos, static_col) in wall_colliders {
            // Quick distance reject — walls more than 2 units away can't overlap
            let dx = (pos.x - static_pos.x).abs();
            let dz = (pos.z - static_pos.z).abs();
            if dx > 2.0 || dz > 2.0 {
                continue;
            }

            let (s_min, s_max) = static_col.bounds(static_pos);

            // Check AABB overlap
            if dyn_max.x <= s_min.x || dyn_min.x >= s_max.x {
                continue;
            }
            if dyn_max.y <= s_min.y || dyn_min.y >= s_max.y {
                continue;
            }
            if dyn_max.z <= s_min.z || dyn_min.z >= s_max.z {
                continue;
            }

            // Compute penetration on each axis
            let overlap_x = (dyn_max.x - s_min.x).min(s_max.x - dyn_min.x);
            let overlap_z = (dyn_max.z - s_min.z).min(s_max.z - dyn_min.z);

            // Push out on the axis with smallest overlap (XZ only — no vertical push)
            if overlap_x <= overlap_z {
                let sign = if transform.position.x < static_pos.x {
                    -1.0
                } else {
                    1.0
                };
                transform.position.x += sign * overlap_x;
            } else {
                let sign = if transform.position.z < static_pos.z {
                    -1.0
                } else {
                    1.0
                };
                transform.position.z += sign * overlap_z;
            }
        }
    }
}

/// Enemy AI: chase the player when within detection range.
pub fn enemy_ai_system(world: &mut World) {
    // Find player position
    let player_pos: Option<Vec3> = world
        .query::<(&Transform, &Player)>()
        .iter()
        .map(|(_, (t, _))| t.position)
        .next();

    let Some(player_pos) = player_pos else { return };

    let detection_range = 12.0_f32;

    for (_id, (transform, velocity, enemy)) in
        world.query_mut::<(&Transform, &mut Velocity, &Enemy)>()
    {
        let to_player = player_pos - transform.position;
        let dist = to_player.length();

        if dist < detection_range && dist > 0.5 {
            // Chase player
            let dir = Vec3::new(to_player.x, 0.0, to_player.z).normalize_or_zero();
            velocity.linear = dir * enemy.speed;
        } else {
            velocity.linear = Vec3::ZERO;
        }
    }
}

/// Contact damage: enemies hurt player on overlap.
pub fn contact_damage_system(world: &mut World, dt: f32) {
    // Get player info
    let player_data: Option<(hecs::Entity, Vec3, Collider)> = world
        .query::<(&Transform, &Collider, &Player)>()
        .iter()
        .map(|(e, (t, c, _))| (e, t.position, *c))
        .next();

    let Some((player_entity, player_pos, player_col)) = player_data else {
        return;
    };

    let (p_min, p_max) = player_col.bounds(player_pos);

    // Check each enemy for overlap with player (skip dying enemies).
    // Also collect enemies that are currently in contact so we can
    // mark their `EnemyAnim.attacking` flag for the bite animation.
    let mut damage_total = 0.0_f32;
    let mut biting_enemies: Vec<hecs::Entity> = Vec::new();
    for (id, (transform, collider, enemy)) in world
        .query::<(&Transform, &Collider, &Enemy)>()
        .without::<&Dying>()
        .iter()
    {
        let (e_min, e_max) = collider.bounds(transform.position);

        // AABB overlap test
        if p_max.x > e_min.x
            && p_min.x < e_max.x
            && p_max.y > e_min.y
            && p_min.y < e_max.y
            && p_max.z > e_min.z
            && p_min.z < e_max.z
        {
            damage_total += enemy.speed * 0.5 * dt; // Damage scales with enemy speed
            biting_enemies.push(id);
        }
    }

    for entity in biting_enemies {
        if let Ok(mut ea) = world.get::<&mut EnemyAnim>(entity) {
            ea.attacking = true;
        }
    }

    if damage_total > 0.0 {
        if let Ok(mut health) = world.get::<&mut Health>(player_entity) {
            health.current = (health.current - damage_total).max(0.0);
        }
    }
}

/// Player attack: press Space to damage nearby enemies. Returns (position, damage) pairs.
pub fn player_attack_system(world: &mut World, input: &Input, dt: f32) -> Vec<(glam::Vec3, f32)> {
    // Tick attack cooldowns
    for (_id, attack) in world.query_mut::<&mut Attack>() {
        attack.timer = (attack.timer - dt).max(0.0);
    }

    if !input.is_key_held(winit::keyboard::KeyCode::Space) {
        return Vec::new();
    }

    // Get player transform + attack
    let player_data: Option<(Vec3, Quat, f32, f32)> = world
        .query::<(&Transform, &Attack, &Player)>()
        .iter()
        .map(|(_, (t, a, _))| (t.position, t.rotation, a.damage, a.range))
        .next();

    let Some((player_pos, _player_rot, damage, range)) = player_data else {
        return Vec::new();
    };

    // Check if attack is ready
    let attack_ready = world
        .query::<(&Attack, &Player)>()
        .iter()
        .any(|(_, (a, _))| a.ready());

    if !attack_ready {
        return Vec::new();
    }

    // Reset cooldown
    for (_id, (attack, _player)) in world.query_mut::<(&mut Attack, &Player)>() {
        attack.timer = attack.cooldown;
    }

    // Damage enemies in range
    let enemies_in_range: Vec<(hecs::Entity, Vec3)> = world
        .query::<(&Transform, &Enemy)>()
        .iter()
        .filter(|(_, (t, _))| {
            let dist = (t.position - player_pos).length();
            dist <= range
        })
        .map(|(e, (t, _))| (e, t.position))
        .collect();

    let mut damage_events = Vec::new();
    for (entity, pos) in enemies_in_range {
        if let Ok(mut health) = world.get::<&mut Health>(entity) {
            health.current -= damage;
            damage_events.push((pos, damage));
        }
    }
    damage_events
}

/// Info about a killed entity.
pub struct KillInfo {
    pub position: glam::Vec3,
    pub progress_value: f32,
    pub is_boss: bool,
    pub is_elite: bool,
}

/// Remove dead entities and hide their render objects.
/// Returns list of kills that happened this frame.
pub fn despawn_system(world: &mut World, renderer: &mut Renderer) -> Vec<KillInfo> {
    // Find newly dead entities (not already dying)
    let dead: Vec<(
        hecs::Entity,
        Option<usize>,
        Option<f32>,
        bool,
        bool,
        glam::Vec3,
    )> = world
        .query::<(
            &Health,
            Option<&Renderable>,
            Option<&Enemy>,
            Option<&Boss>,
            Option<&Elite>,
            &Transform,
        )>()
        .without::<&Dying>()
        .without::<&Player>()
        .iter()
        .filter(|(_, (h, _, _, _, _, _))| h.is_dead())
        .map(|(e, (_, r, enemy, boss, elite, t))| {
            (
                e,
                r.map(|r| r.object_index),
                enemy.map(|en| en.progress_value),
                boss.is_some(),
                elite.is_some(),
                t.position,
            )
        })
        .collect();

    let mut kills = Vec::new();

    for (entity, _obj_idx, prog_value, is_boss, is_elite, position) in dead {
        kills.push(KillInfo {
            position,
            progress_value: prog_value.unwrap_or(0.0),
            is_boss,
            is_elite,
        });
        // Skinned enemies play their own Death animation via
        // `enemy_anim_system`, so we keep the corpse around longer
        // (~1.4s) instead of doing the procedural shrink. The squash
        // tick below also detects a Skinned component and skips the
        // matrix override.
        let is_skinned = world.get::<&Skinned>(entity).is_ok();
        let _ = world.insert_one(
            entity,
            Dying {
                timer: 0.0,
                duration: if is_skinned { 1.4 } else { 0.4 },
                original_scale: 1.0,
            },
        );
        // Remove enemy AI/velocity so it stops moving
        let _ = world.remove_one::<Velocity>(entity);
    }

    // Tick dying entities: shrink + flatten animation (procedural enemies
    // only; skinned ones play `Death` instead and the matrix is left
    // untouched so the keyframes drive the pose).
    let dying_data: Vec<(hecs::Entity, usize, f32, f32)> = world
        .query::<(&Dying, &Renderable)>()
        .iter()
        .map(|(e, (d, r))| (e, r.object_index, d.timer, d.duration))
        .collect();

    for (entity, obj_idx, timer, duration) in &dying_data {
        let is_skinned = world.get::<&Skinned>(*entity).is_ok();
        if !is_skinned {
            let progress = (*timer / *duration).min(1.0);
            let y_scale = 1.0 - progress;
            let xz_scale = 1.0 + progress * 0.5 - progress * progress * 1.5;
            let xz_scale = xz_scale.max(0.0);

            if *obj_idx < renderer.objects.len() {
                let pos = renderer.objects[*obj_idx].model_matrix.col(3).truncate();
                renderer.objects[*obj_idx].model_matrix = glam::Mat4::from_translation(pos)
                    * glam::Mat4::from_scale(glam::Vec3::new(xz_scale, y_scale, xz_scale));
            }
        }

        // Advance timer
        if let Ok(mut dying) = world.get::<&mut Dying>(*entity) {
            dying.timer += 1.0 / 60.0; // approximate dt
        }
    }

    // Remove fully dead entities
    let to_remove: Vec<hecs::Entity> = world
        .query::<&Dying>()
        .iter()
        .filter(|(_, d)| d.timer >= d.duration)
        .map(|(e, _)| e)
        .collect();

    for entity in to_remove {
        // Hide render object
        if let Ok(r) = world.get::<&Renderable>(entity) {
            let idx = r.object_index;
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
            }
        }
        // Stop any debuff aura emitters glued to this entity so they
        // don't keep spawning particles at the corpse. (No-op now —
        // debuff system is server-authoritative; kept as a marker.)
        let _ = entity;
        let _ = renderer;
        let _ = world.despawn(entity);
    }

    kills
}

/// Tunables and clip-name fallbacks for the player full-body action FSM.
/// Game crates pass one of these into `player_action_pre_system` /
/// `player_action_post_system`, so adding a new full-body action only
/// needs an extra clip-name list — no code edits to the FSM itself.
#[derive(Clone, Copy)]
pub struct PlayerActionConfig {
    /// Initial vertical velocity on a fresh jump (m/s).
    pub jump_velocity: f32,
    /// How long the JumpStart wind-up lasts before going airborne.
    pub jump_start_dur: f32,
    /// JumpLand recovery time before returning control to locomotion.
    pub jump_land_dur: f32,
    /// Constant XZ speed during a Roll.
    pub roll_speed: f32,
    /// Cross-fade duration into action clips (seconds).
    pub fade_in: f32,
    /// Cross-fade duration into the landing clip.
    pub fade_in_land: f32,

    pub jump_start_clips: &'static [&'static str],
    pub jump_air_clips: &'static [&'static str],
    pub jump_land_clips: &'static [&'static str],
}

impl Default for PlayerActionConfig {
    fn default() -> Self {
        Self {
            jump_velocity: 9.5,
            jump_start_dur: 0.10,
            jump_land_dur: 0.30,
            roll_speed: 11.0,
            fade_in: 0.10,
            fade_in_land: 0.08,
            jump_start_clips: &["Jump_Start", "Jump_Begin", "Jump"],
            jump_air_clips: &["Jump", "Jump_Loop", "Jump_Air"],
            jump_land_clips: &["Jump_Land", "Land", "Landing"],
        }
    }
}

/// Pre-movement player FSM step:
///   - decrements `Player::action_timer`,
///   - advances JumpStart → JumpAir, expires JumpLand / Roll,
///   - keeps roll velocity glued to `Player::aim_dir`,
///   - consumes Space (when `accept_input`) to start a new jump.
///
/// `accept_input` should be `false` while the player is dying or a
/// fade-to-black is in progress so input is ignored cleanly.
pub fn player_action_pre_system(
    world: &mut World,
    input: &Input,
    dt: f32,
    cfg: &PlayerActionConfig,
    accept_input: bool,
) {
    use super::components::PlayerAction;
    // Find the *local* player. The remote-player entities also
    // carry `Player`, so an unfiltered query would non-deterministically
    // pick a remote and stomp its FSM state with our keyboard.
    let Some(player_id) = world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(e, _)| e)
        .next()
    else {
        return;
    };

    let dead = world
        .get::<&Health>(player_id)
        .map(|h| h.is_dead())
        .unwrap_or(false);
    let is_ghost = world.get::<&super::components::Ghost>(player_id).is_ok();

    let (action, action_timer, airborne, aim_dir) = match world.get::<&Player>(player_id) {
        Ok(p) => (p.action, p.action_timer, p.airborne, p.aim_dir),
        Err(_) => return,
    };

    if (dead && !is_ghost) || !accept_input {
        return;
    }

    // Hold roll velocity in `aim_dir` for the entire duration so the
    // body slides instead of teleporting. Ease the speed down over the
    // last ~third so the roll settles instead of stopping abruptly.
    if matches!(action, PlayerAction::Roll) {
        let dir = if aim_dir.length_squared() > 0.0001 {
            aim_dir.normalize()
        } else {
            Vec3::Z
        };
        // `action_timer` is time-remaining. Hold full speed until the
        // last `decel_window` seconds, then ease-out (cubic) to ~15%.
        let decel_window = 0.35;
        let min_scale = 0.15;
        let scale = if action_timer >= decel_window {
            1.0
        } else {
            let t = (action_timer / decel_window).clamp(0.0, 1.0);
            // ease-out cubic: starts fast, settles smoothly
            let eased = 1.0 - (1.0 - t).powi(3);
            min_scale + (1.0 - min_scale) * eased
        };
        if let Ok(mut v) = world.get::<&mut Velocity>(player_id) {
            v.linear.x = dir.x * cfg.roll_speed * scale;
            v.linear.z = dir.z * cfg.roll_speed * scale;
        }
    }

    // Advance timer and decide transitions.
    let mut new_action = action;
    let mut new_timer = (action_timer - dt).max(0.0);
    let mut next_clip: Option<&[&str]> = None;
    let mut next_loop = false;

    match action {
        PlayerAction::JumpStart => {
            if new_timer <= 0.0 {
                new_action = PlayerAction::JumpAir;
                new_timer = 0.0;
                next_clip = Some(cfg.jump_air_clips);
                next_loop = true;
            }
        }
        PlayerAction::JumpLand => {
            if new_timer <= 0.0 {
                new_action = PlayerAction::None;
                new_timer = 0.0;
            }
        }
        PlayerAction::Roll => {
            if new_timer <= 0.0 {
                new_action = PlayerAction::None;
                new_timer = 0.0;
                if let Ok(mut v) = world.get::<&mut Velocity>(player_id) {
                    v.linear.x = 0.0;
                    v.linear.z = 0.0;
                }
            }
        }
        PlayerAction::JumpAir | PlayerAction::None => {}
    }

    // Jumping is intentionally disabled — this is an ARPG with
    // grid-based locomotion, jumping adds nothing to the
    // gameplay loop and the legacy bindings just got in the
    // way of the new step-up resolution. The PlayerAction
    // variants and animation hookup are kept so the network
    // protocol and clip table don't have to be reshuffled,
    // but the trigger is gone. Falling (walking off a ledge
    // into a sunken pit) still works through gravity in
    // `movement_system` — it just can't be initiated by the
    // player.

    if new_action != action || (new_timer - action_timer).abs() > f32::EPSILON {
        if let Ok(mut p) = world.get::<&mut Player>(player_id) {
            p.action = new_action;
            p.action_timer = new_timer;
        }
    }

    if let Some(names) = next_clip {
        let clip = world
            .get::<&AnimationSet>(player_id)
            .ok()
            .and_then(|s| s.find_any(names));
        if let Some(clip) = clip {
            if let Ok(mut anim) = world.get::<&mut Animator>(player_id) {
                anim.cross_fade(clip, next_loop, cfg.fade_in);
                anim.speed = 1.0;
            }
        }
    }
}

/// Post-movement player FSM step: detects ground contact while
/// airborne and transitions JumpAir → JumpLand so the landing clip
/// plays as soon as the feet touch.
pub fn player_action_post_system(world: &mut World, cfg: &PlayerActionConfig) {
    use super::components::PlayerAction;
    // Filter to the local player; remote-player entities carry
    // `Player` too and we mustn't transition their FSM here.
    let Some(player_id) = world
        .query::<(&Player, &LocalPlayer)>()
        .iter()
        .map(|(e, _)| e)
        .next()
    else {
        return;
    };

    // Dead players keep whatever full-body animation `trigger_player_death`
    // set up — never overwrite it with a JumpLand clip on touchdown.
    // Ghosts (HP still 0 but spectator-mobile) bypass this gate so
    // their landing pose is animated like a live player.
    let dead = world
        .get::<&Health>(player_id)
        .map(|h| h.is_dead())
        .unwrap_or(false);
    let is_ghost = world.get::<&super::components::Ghost>(player_id).is_ok();
    if dead && !is_ghost {
        return;
    }

    let should_land = match world.get::<&Player>(player_id) {
        Ok(p) => matches!(p.action, PlayerAction::JumpAir) && !p.airborne,
        Err(_) => return,
    };
    if !should_land {
        return;
    }

    if let Ok(mut p) = world.get::<&mut Player>(player_id) {
        p.action = PlayerAction::JumpLand;
        p.action_timer = cfg.jump_land_dur;
    }
    // Kill horizontal velocity on touchdown so the landing animation
    // doesn't slide forward — the player gets control back as soon as
    // JumpLand expires (`player_action_pre_system`).
    if let Ok(mut v) = world.get::<&mut Velocity>(player_id) {
        v.linear.x = 0.0;
        v.linear.z = 0.0;
    }

    let clip = world
        .get::<&AnimationSet>(player_id)
        .ok()
        .and_then(|s| s.find_any(cfg.jump_land_clips));
    if let Some(clip) = clip {
        if let Ok(mut anim) = world.get::<&mut Animator>(player_id) {
            anim.cross_fade(clip, false, cfg.fade_in_land);
            anim.speed = 1.0;
        }
    }
}
