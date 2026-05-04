use glam::{Quat, Vec3};
use hecs::World;

use super::components::{AnimationSet, Attack, Boss, Collider, Dying, Elite, Enemy, Health, Player, Renderable, Skinned, Transform, Velocity};
use crate::animation::{self, Animator};
use crate::input::Input;
use crate::physics::{self, Aabb, Ray};
use crate::renderer::Renderer;

/// Process player input and set velocity based on WASD keys.
pub fn player_input_system(world: &mut World, input: &Input, dt: f32) {
    for (_id, (transform, velocity, player)) in
        world.query_mut::<(&Transform, &mut Velocity, &Player)>()
    {
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
            velocity.linear = move_dir.normalize() * player.speed;
            // Face movement direction
            let _ = transform; // We'll update rotation below in movement system
        } else {
            velocity.linear = Vec3::ZERO;
        }

        let _ = dt;
    }
}

/// Apply velocity to transform.
pub fn movement_system(world: &mut World, dt: f32) {
    for (_id, (transform, velocity)) in world.query_mut::<(&mut Transform, &Velocity)>() {
        if velocity.linear.length_squared() > 0.001 {
            transform.position += velocity.linear * dt;
            // Keep entities on the ground plane. The skinned glTF character
            // is authored with its origin at the feet, so y=0 sits on the
            // floor (procedural cube meshes are centered, see movement of
            // those entities elsewhere if they need a different offset).
            transform.position.y = 0.0;
            // Smoothly rotate to face movement direction.
            // glTF convention: characters look down +Z in bind pose, so use
            // the unsigned atan2 to face the velocity vector.
            let target_yaw = velocity.linear.x.atan2(velocity.linear.z);
            let target_rot = Quat::from_rotation_y(target_yaw);
            transform.rotation = transform.rotation.slerp(target_rot, (dt * 10.0).min(1.0));
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
    const WALK_REF_SPEED: f32 = 1.5;     // m/s for Walk_Loop
    const JOG_REF_SPEED: f32 = 3.5;      // m/s for Jog_Fwd
    const SPRINT_REF_SPEED: f32 = 6.0;   // m/s for Sprint
    const WALK_JOG_THRESHOLD: f32 = 2.0;
    const JOG_SPRINT_THRESHOLD: f32 = 4.5;
    const STILL_THRESHOLD_SQ: f32 = 0.04;
    const FADE_LOCOMOTION: f32 = 0.18;
    const FADE_GAIT: f32 = 0.12;

    const SPRINT_NAMES: &[&str] = &[
        "Sprint_Loop", "Sprint", "Sprint_Fwd", "Sprint_Forward_Loop",
        "Run_Loop", "Run", // some packs only ship "Run"
    ];
    const JOG_NAMES: &[&str] = &[
        "Jog_Fwd", "Jog_Forward", "Jog_Forward_Loop", "Jog_Loop", "Jog",
        "Run_Loop", "Run", // jog as a fallback for run-less packs
    ];
    const WALK_NAMES: &[&str] = &[
        "Walk_Loop", "Walk", "Walk_Fwd", "Walk_Forward_Loop",
    ];
    const IDLE_NAMES: &[&str] = &["Idle_Loop", "Idle"];

    for (_id, (vel, set, animator)) in
        world.query_mut::<(&Velocity, &AnimationSet, &mut Animator)>()
    {
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
                let fade = if was_idle || going_idle { FADE_LOCOMOTION } else { FADE_GAIT };
                animator.cross_fade(clip, true, fade);
            }
            animator.speed = target_speed_mult;
        }
    }
}

/// Advance every base Animator (and any active SpellCast layer) and re-skin
/// each mesh into the renderer's per-frame dynamic vertex buffer.
pub fn skinning_system(world: &mut World, renderer: &mut Renderer, dt: f32) {
    let mut palette: Vec<glam::Mat4> = Vec::new();
    for (_id, (renderable, skinned, animator, transform, mut cast, player)) in world
        .query_mut::<(
            &Renderable,
            &mut Skinned,
            &mut Animator,
            &Transform,
            Option<&mut super::components::SpellCast>,
            Option<&Player>,
        )>()
    {
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

        // Compute torso twist: the difference between aim yaw and body
        // yaw, clamped so we never twist past ~120° (you'd see the rig
        // tear apart). When the offset would exceed the limit, the body
        // itself catches up via the player_input/movement systems.
        let twist = if let Some(p) = player {
            let aim = p.aim_dir;
            if aim.length_squared() > 1e-4 {
                let aim_yaw = aim.x.atan2(aim.z);
                // Extract the body's yaw directly from its forward vector
                // (rotation * +Z) rather than via Euler decomposition,
                // which has axis-ordering pitfalls. This matches the
                // movement system's atan2(x, z) convention exactly.
                let fwd = transform.rotation * Vec3::Z;
                let body_yaw = fwd.x.atan2(fwd.z);
                let mut delta = aim_yaw - body_yaw;
                while delta > std::f32::consts::PI { delta -= std::f32::consts::TAU; }
                while delta < -std::f32::consts::PI { delta += std::f32::consts::TAU; }
                let limit = std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_6; // ~120°
                let clamped = delta.clamp(-limit, limit);
                if p.spine_joint != u32::MAX {
                    Some((p.spine_joint as usize, clamped))
                } else { None }
            } else { None }
        } else { None };

        if layer_anim.is_some() && layer_weight > 0.001 || twist.is_some() {
            animation::build_bone_palette_layered(
                animator, layer_anim, layer_mask, layer_weight, twist,
                &skinned.mesh.joints, &mut palette,
            );
        } else {
            animation::build_bone_palette(animator, &skinned.mesh.joints, &mut palette);
        }
        skinned.mesh.skin_to(&palette, &mut skinned.scratch);
        renderer.update_dynamic_vertices(renderable.object_index, &skinned.scratch);
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
        if cast.phase == SpellPhase::Idle {
            cast.weight = (cast.weight - dt * WEIGHT_RAMP).max(0.0);
            continue;
        }

        // We deliberately skip the Spell_Simple_Shoot clip on the cast
        // layer: it dips the hand briefly which looks like a flinch right
        // as the projectile spawns. Instead we fire at the Enter→Exit
        // boundary, so the hand stays raised through the release and the
        // Exit clip provides the natural recovery motion.
        let target_clip = match cast.phase {
            SpellPhase::Entering => set.find_any(&["Spell_Simple_Enter", "Spell_Enter", "Cast_Enter"]),
            SpellPhase::Shooting => set.find_any(&["Spell_Simple_Enter", "Spell_Enter", "Cast_Enter"]),
            SpellPhase::Exiting => set.find_any(&["Spell_Simple_Exit", "Spell_Exit", "Cast_Exit"]),
            SpellPhase::Idle => None,
        };
        let Some(target_clip) = target_clip else {
            // Missing clip for this phase — fall through gracefully.
            match cast.phase {
                SpellPhase::Entering => cast.phase = SpellPhase::Shooting,
                SpellPhase::Shooting => {
                    if !cast.fired {
                        fire_events.push((entity, cast.pending_aim_dir, cast.pending_damage));
                        cast.fired = true;
                    }
                    cast.phase = SpellPhase::Exiting;
                }
                SpellPhase::Exiting => {
                    cast.phase = SpellPhase::Idle;
                    cast.layer_animator = None;
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

        let target_weight = if cast.phase == SpellPhase::Exiting { 0.0 } else { 1.0 };
        let dw = dt * WEIGHT_RAMP;
        cast.weight = if target_weight > cast.weight {
            (cast.weight + dw).min(target_weight)
        } else {
            (cast.weight - dw).max(target_weight)
        };

        if let Some(la) = cast.layer_animator.as_ref() {
            let done = la.time >= la.clip.duration - 1e-3;
            if done {
                match cast.phase {
                    SpellPhase::Entering => {
                        // Fire at the end of the wind-up so the projectile
                        // leaves the hand at its highest, fully-extended pose.
                        if !cast.fired {
                            fire_events.push((entity, cast.pending_aim_dir, cast.pending_damage));
                            cast.fired = true;
                        }
                        cast.phase = SpellPhase::Exiting;
                    }
                    SpellPhase::Shooting => cast.phase = SpellPhase::Exiting,
                    SpellPhase::Exiting => {
                        if cast.weight <= 0.001 {
                            cast.phase = SpellPhase::Idle;
                            cast.layer_animator = None;
                            cast.fired = false;
                        }
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
pub fn camera_follow_system(world: &World, renderer: &mut Renderer, input: &Input, wall_aabbs: &[Aabb]) {
    for (_id, (transform, _player)) in world.query::<(&Transform, &Player)>().iter() {
        let target = transform.position + Vec3::new(0.0, 0.8, 0.0);

        let yaw = input.camera_yaw();
        let pitch = input.camera_pitch();
        let distance = input.camera_distance();

        let offset = Vec3::new(
            distance * pitch.cos() * yaw.sin(),
            distance * pitch.sin(),
            distance * pitch.cos() * yaw.cos(),
        );

        let desired = target + offset;

        // Raycast from target toward desired camera position
        let (ray, ray_len) = Ray::between(target, desired);

        let actual_pos = if let Some(hit) = physics::raycast(&ray, ray_len, wall_aabbs) {
            // Pull camera in front of the wall
            let safe_dist = (hit.distance - 0.5).max(0.5);
            ray.at(safe_dist)
        } else {
            desired
        };

        renderer.camera.position = actual_pos;
        renderer.camera.target = target;
    }
}

/// Resolve collisions between dynamic entities (player) and static colliders (walls).
/// Uses AABB overlap + minimum penetration push-out.
pub fn collision_system(world: &mut World, wall_colliders: &[(Vec3, Collider)]) {
    // Resolve dynamic entities (those with Velocity) against statics
    for (_id, (transform, collider, _vel)) in
        world.query_mut::<(&mut Transform, &Collider, &mut Velocity)>()
    {
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
                let sign = if transform.position.x < static_pos.x { -1.0 } else { 1.0 };
                transform.position.x += sign * overlap_x;
            } else {
                let sign = if transform.position.z < static_pos.z { -1.0 } else { 1.0 };
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

    let Some((player_entity, player_pos, player_col)) = player_data else { return };

    let (p_min, p_max) = player_col.bounds(player_pos);

    // Check each enemy for overlap with player (skip dying enemies)
    let mut damage_total = 0.0_f32;
    for (_id, (transform, collider, enemy)) in
        world.query::<(&Transform, &Collider, &Enemy)>()
            .without::<&Dying>()
            .iter()
    {
        let (e_min, e_max) = collider.bounds(transform.position);

        // AABB overlap test
        if p_max.x > e_min.x && p_min.x < e_max.x
            && p_max.y > e_min.y && p_min.y < e_max.y
            && p_max.z > e_min.z && p_min.z < e_max.z
        {
            damage_total += enemy.speed * 0.5 * dt; // Damage scales with enemy speed
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

    let Some((player_pos, _player_rot, damage, range)) = player_data else { return Vec::new() };

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
    let dead: Vec<(hecs::Entity, Option<usize>, Option<f32>, bool, bool, glam::Vec3)> = world
        .query::<(&Health, Option<&Renderable>, Option<&Enemy>, Option<&Boss>, Option<&Elite>, &Transform)>()
        .without::<&Dying>()
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
        // Start death animation instead of instant removal
        let _ = world.insert_one(entity, Dying {
            timer: 0.0,
            duration: 0.4,
            original_scale: 1.0,
        });
        // Remove enemy AI/velocity so it stops moving
        let _ = world.remove_one::<Velocity>(entity);
    }

    // Tick dying entities: shrink + flatten animation
    let dying_data: Vec<(hecs::Entity, usize, f32, f32, glam::Vec3)> = world
        .query::<(&Dying, &Renderable, &Transform)>()
        .iter()
        .map(|(e, (d, r, t))| (e, r.object_index, d.timer, d.duration, t.position))
        .collect();

    for (entity, obj_idx, timer, duration, _pos) in &dying_data {
        let progress = (*timer / *duration).min(1.0);
        // Shrink + squash: Y scale goes to 0, XZ expands slightly then collapses
        let y_scale = 1.0 - progress;
        let xz_scale = 1.0 + progress * 0.5 - progress * progress * 1.5;
        let xz_scale = xz_scale.max(0.0);

        if *obj_idx < renderer.objects.len() {
            let pos = renderer.objects[*obj_idx].model_matrix.col(3).truncate();
            renderer.objects[*obj_idx].model_matrix =
                glam::Mat4::from_translation(pos)
                * glam::Mat4::from_scale(glam::Vec3::new(xz_scale, y_scale, xz_scale));
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
        let _ = world.despawn(entity);
    }

    kills
}
