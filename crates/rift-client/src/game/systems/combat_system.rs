//! Local combat-input subsystem.
//!
//! Translates mouse / number-key presses into `NetCastRequest`s
//! and drives the two client-side combat-input modes:
//!
//! * **Placed targeting** — when an AoE ability is queued the
//!   player drags a ground indicator with the cursor; LMB
//!   confirms the cast at the cursor position, RMB / Esc
//!   cancels and refunds the cooldown.
//! * **Channel hold-to-cast** — infinite-duration channels
//!   (Frost Ray) latch on key-down and end on key-up / move /
//!   RMB / Esc. Finite channels run on the server clock and
//!   are not tracked here.
//!
//! Server is authoritative for damage, projectile spawn, and
//! channel ticks; this module only handles input intent and
//! the optimistic cast pose / particles via
//! [`crate::game::ability::trigger_local_cast`].

use glam::{Mat4, Vec3};
use winit::keyboard::KeyCode;

use rift_engine::animation_profile::{JointKey, SkeletonBindings};
use rift_engine::ecs::components::{LocalPlayer, Player, Skinned, Transform};
use rift_engine::input::Input;
use rift_engine::renderer::Renderer;

use rift_game::abilities::{self, Ability, TargetingMode};

use crate::game::state::GameState;
use crate::game::sub_state::{ActiveChannel, NetCastRequest};
use crate::game::SelectionRelation;

/// Active placed-ability targeting state (player is choosing
/// where to place an AoE). Pure visual / input state — the
/// actual cast is sent to the server when the player confirms.
pub struct PlacedTargeting {
    /// Which ability slot triggered this.
    pub slot_index: usize,
    /// The ability being placed (cloned).
    pub ability: Ability,
    /// Radius of the AoE indicator circle.
    pub radius: f32,
    /// Render object index for the "valid placement" (blue)
    /// indicator. Driven each frame: shown at the cursor when
    /// line-of-sight to the caster is clear, collapsed to
    /// `Mat4::ZERO` otherwise.
    pub indicator_obj: Option<usize>,
    /// Render object index for the "invalid placement" (red)
    /// indicator. Mutually exclusive with [`Self::indicator_obj`]
    /// each frame — only one of the two is visible at a time.
    /// Shown when an XZ raycast from the caster's chest to the
    /// cursor is blocked by a wall, or the cursor lies inside
    /// a wall AABB.
    pub invalid_indicator_obj: Option<usize>,
    /// Cached LoS result from the most recent indicator update
    /// frame. The LMB-confirm path reads this to decide
    /// whether to send the cast or just play a soft rejection
    /// (refund slot, leave targeting active).
    pub los_blocked: bool,
}

/// Active entity-target picking state for friendly
/// single-target abilities (heals). The HUD shows a green
/// ring under the candidate ally; LMB confirms, RMB / Esc
/// cancels and refunds the slot's cooldown the same way
/// [`PlacedTargeting`] does.
pub struct EntityTargeting {
    /// Which ability slot triggered this.
    pub slot_index: usize,
    /// The ability being targeted (cloned).
    pub ability: Ability,
    /// Render object index for the ground hover indicator. We
    /// always allocate one; when no candidate is hovered the
    /// model matrix is collapsed to `Mat4::ZERO` so the mesh
    /// disappears without thrashing the draw list.
    pub indicator_obj: Option<usize>,
    /// Net id of the ally currently under the cursor (if
    /// any). Kept separately from the indicator so confirm /
    /// cancel can run without re-doing the pick math.
    pub hovered: Option<rift_net::NetId>,
}

/// Per-frame combat-input tick. Replaces the old
/// `GameState::tick_combat` body — gameplay phase code calls
/// this once after the player+world updates have run.
pub fn tick(state: &mut GameState, input: &Input, renderer: &mut Renderer, dt: f32) {
    let player_data: Option<(Vec3, glam::Quat)> = state
        .world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| (t.position, t.rotation))
        .next();

    let Some((player_pos, _player_rot)) = player_data else {
        return;
    };

    let aim_dir = crate::game::cursor::aim_dir(input, renderer, player_pos);

    if tick_placed_targeting(state, input, renderer, player_pos, aim_dir) {
        return;
    }

    if tick_entity_targeting(state, input, renderer, player_pos, aim_dir) {
        return;
    }

    if tick_active_channel(state, input, dt) {
        // Channel still active or just ended — suppress further
        // ability presses this frame.
        return;
    }

    tick_ability_keybinds(state, input, renderer, player_pos, aim_dir);
}

/// Hub-safe ability input. Keeps passive movement abilities
/// available without enabling hostile casts or target modes in the
/// sanctuary scene.
pub fn tick_hub_passives(state: &mut GameState, input: &Input, renderer: &mut Renderer) {
    let player_pos = state
        .world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next();
    let Some(player_pos) = player_pos else {
        return;
    };
    let aim_dir = crate::game::cursor::aim_dir(input, renderer, player_pos);
    tick_roll(state, input, renderer, player_pos, aim_dir);
}

/// Drive the placed-ability targeting indicator. Returns
/// `true` if the caller should bail out of this frame's combat
/// tick (placement was confirmed / cancelled, or the indicator
/// is still being dragged).
fn tick_placed_targeting(
    state: &mut GameState,
    input: &Input,
    renderer: &mut Renderer,
    player_pos: Vec3,
    aim_dir: Vec3,
) -> bool {
    if state.frame.targeting.is_none() {
        return false;
    }

    if let Some(cursor_pos) = crate::game::cursor::world_pos(input, renderer, 0.0) {
        // LoS check: trace from the caster's chest height
        // toward the cursor. If a wall sits between them, or
        // the cursor itself is inside a wall AABB, the
        // placement is invalid. We use the same XZ wall
        // colliders the projectile / beam paths consult so
        // the visual matches the server's eventual targeting
        // rules.
        let from = player_pos + Vec3::Y * 1.2;
        let to = cursor_pos + Vec3::Y * 1.2;
        let delta = to - from;
        let dist = delta.length();
        let blocked = if dist > 1e-3 {
            let dir = delta / dist;
            // Strict: any wall between the caster and the
            // cursor blocks placement, full stop. No
            // "near-cursor grace" — if you can't see it,
            // you can't drop a Rain of Fire on it.
            let ray = rift_engine::physics::Ray {
                origin: from,
                direction: dir,
            };
            rift_engine::physics::raycast_any(&ray, dist, &state.floor.wall_aabbs)
        } else {
            false
        };

        let radius = state.frame.targeting.as_ref().unwrap().radius;
        // Pick which colour ring to display this frame.
        let (show_idx, hide_idx, show_color) = {
            let targeting = state.frame.targeting.as_mut().unwrap();
            targeting.los_blocked = blocked;
            if blocked {
                (
                    targeting.invalid_indicator_obj,
                    targeting.indicator_obj,
                    [1.0, 0.2, 0.15],
                )
            } else {
                (
                    targeting.indicator_obj,
                    targeting.invalid_indicator_obj,
                    [0.2, 0.5, 1.0],
                )
            }
        };
        // Rebuild the visible ring's vertices in world space
        // with per-vertex Y sampled from the dungeon's
        // height-field, so the ring bends across raised
        // daises, sunken pits, and stair ramps instead of
        // floating through them. Falls back to a flat ring
        // at y=0 when no dungeon is loaded (hub, transitions).
        if let Some(idx) = show_idx {
            if idx < renderer.objects.len() {
                let conformed = match state.floor_mgr.dungeon.as_ref() {
                    Some(floor) => rift_engine::Mesh::targeting_circle_conformed(
                        show_color,
                        cursor_pos,
                        radius,
                        |x, z| floor.tile_floor_y_at(x, z),
                    ),
                    None => rift_engine::Mesh::targeting_circle_conformed(
                        show_color,
                        cursor_pos,
                        radius,
                        |_, _| 0.0,
                    ),
                };
                renderer.update_dynamic_vertices(idx, &conformed.vertices);
                renderer.objects[idx].model_matrix = Mat4::IDENTITY;
            }
        }
        if let Some(idx) = hide_idx {
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::ZERO;
            }
        }
    }

    // Left-click: confirm placement → forward to server.
    if input.left_clicked() {
        if let Some(cursor_pos) = crate::game::cursor::world_pos(input, renderer, 0.0) {
            // LoS gate — refuse to send the cast through
            // walls. The cooldown was consumed up-front by
            // `try_use`; keep the indicator up so the player
            // can drag to a valid spot without re-pressing
            // the keybind. (Right-click / Esc still
            // cancels and refunds normally.)
            if state
                .frame
                .targeting
                .as_ref()
                .map(|t| t.los_blocked)
                .unwrap_or(false)
            {
                return true;
            }
            let targeting = state.frame.targeting.take().unwrap();
            if let Some(obj_idx) = targeting.indicator_obj {
                if obj_idx < renderer.objects.len() {
                    renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                }
            }
            if let Some(obj_idx) = targeting.invalid_indicator_obj {
                if obj_idx < renderer.objects.len() {
                    renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                }
            }
            state.net.casts.push(NetCastRequest {
                ability_id: targeting.ability.wire_id,
                origin: player_pos,
                aim_dir,
                placed_target: Some(cursor_pos),
                // Placed-AoE casts don't use entity targets.
                target_net_id: None,
            });
        }
        return true;
    }

    // Right-click or Escape: cancel targeting.
    if input.right_clicked() || input.key_just_pressed(KeyCode::Escape) {
        let targeting = state.frame.targeting.take().unwrap();
        if let Some(obj_idx) = targeting.indicator_obj {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(obj_idx) = targeting.invalid_indicator_obj {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(slot) = state.player_state.abilities.slots[targeting.slot_index].as_mut() {
            slot.cooldown_remaining = 0.0;
        }
        return true;
    }

    true
}

/// Drive the entity-target picking indicator. Returns `true`
/// if the caller should bail out of this frame's combat tick
/// (mode is active — confirmed, cancelled, or still picking).
fn tick_entity_targeting(
    state: &mut GameState,
    input: &Input,
    renderer: &mut Renderer,
    player_pos: Vec3,
    aim_dir: Vec3,
) -> bool {
    if state.frame.entity_targeting.is_none() {
        return false;
    }

    // Party-frame click shortcut: if the UI tagged a friendly
    // target this frame (resolved to a `NetId` by main.rs from
    // `state.frame.party_click_target_name`), confirm the cast
    // immediately — no cursor pick, no range check (party
    // members are always valid heal targets if you have line
    // of sight to a frame). The server still validates range
    // server-side, so a player who cheaply clicks an
    // out-of-range frame just eats a rejected cast.
    if let Some(target_net_id) = state.frame.party_click_target_net_id.take() {
        let targeting = state.frame.entity_targeting.take().unwrap();
        if let Some(obj_idx) = targeting.indicator_obj {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
            }
        }
        if let Some(slot) = state.player_state.abilities.slots[targeting.slot_index].as_mut() {
            slot.cooldown_remaining = targeting.ability.cooldown;
        }
        state.net.casts.push(NetCastRequest {
            ability_id: targeting.ability.wire_id,
            origin: player_pos,
            aim_dir,
            placed_target: None,
            target_net_id: Some(target_net_id),
        });
        return true;
    }

    // Resolve hover candidate from the current cursor each
    // frame so the highlight tracks pointer motion.
    let range = state
        .frame
        .entity_targeting
        .as_ref()
        .map(|t| t.ability.range)
        .unwrap_or(15.0);
    let pick = state
        .selection
        .target_for_ability(SelectionRelation::Friendly, range, player_pos)
        .and_then(|target_net_id| {
            state
                .selection
                .candidate(crate::game::SelectionRef {
                    net_id: target_net_id,
                    kind: crate::game::SelectableKind::OwnPlayer,
                    relation: SelectionRelation::SelfUnit,
                })
                .map(|candidate| (target_net_id, candidate.position))
        });

    // Update indicator visual + cached hovered net id.
    {
        let targeting = state.frame.entity_targeting.as_mut().unwrap();
        targeting.hovered = pick.map(|(id, _)| id);
        if let Some(obj_idx) = targeting.indicator_obj {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = match pick {
                    Some((_, pos)) => {
                        Mat4::from_translation(pos) * Mat4::from_scale(Vec3::splat(0.9))
                    }
                    None => Mat4::ZERO,
                };
            }
        }
    }

    // Left-click: confirm if we have a target, otherwise
    // ignore the click (so the player can keep waving the
    // cursor around without burning the cooldown on a miss).
    if input.left_clicked() {
        if let Some((target_net_id, _pos)) = pick {
            let targeting = state.frame.entity_targeting.take().unwrap();
            if let Some(obj_idx) = targeting.indicator_obj {
                if obj_idx < renderer.objects.len() {
                    renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                }
            }
            // Now that the cast is actually committing, start
            // the local cooldown — keeps it aligned with the
            // server's CD which begins only when the cast
            // arrives. (`tick_ability_keybinds` refunded the
            // CD when entering targeting mode.)
            if let Some(slot) = state.player_state.abilities.slots[targeting.slot_index].as_mut() {
                slot.cooldown_remaining = targeting.ability.cooldown;
            }
            state.net.casts.push(NetCastRequest {
                ability_id: targeting.ability.wire_id,
                origin: player_pos,
                aim_dir,
                placed_target: None,
                target_net_id: Some(target_net_id),
            });
        }
        return true;
    }

    // Right-click / Escape: cancel and refund the cooldown.
    if input.right_clicked() || input.key_just_pressed(KeyCode::Escape) {
        let targeting = state.frame.entity_targeting.take().unwrap();
        if let Some(obj_idx) = targeting.indicator_obj {
            if obj_idx < renderer.objects.len() {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
            }
        }
        // Cooldown was already refunded on entry, but a future
        // refactor that consumes it eagerly would still want
        // this — keep the explicit zero so the invariant
        // (cancelled cast = 0 CD) is local to this branch.
        if let Some(slot) = state.player_state.abilities.slots[targeting.slot_index].as_mut() {
            slot.cooldown_remaining = 0.0;
        }
        return true;
    }

    true
}

/// Channel hold-to-cast / cancel logic. Returns `true` when
/// the channel is still active (so the caller suppresses new
/// ability presses) and `false` when there's no channel and
/// keybinds should run normally. A just-ended channel returns
/// `false` so the next ability press in the same frame still
/// works (rare, but cheap to support).
fn tick_active_channel(state: &mut GameState, input: &Input, dt: f32) -> bool {
    let Some(active) = state.channel.active else {
        return false;
    };
    let key_held = match active.slot_index {
        0 => input.left_mouse_held(),
        1 => input.is_key_held(KeyCode::Digit1),
        2 => input.is_key_held(KeyCode::Digit2),
        3 => input.is_key_held(KeyCode::Digit3),
        4 => input.is_key_held(KeyCode::Digit4),
        5 => input.is_key_held(KeyCode::Digit5),
        _ => false,
    };
    let movement_held = input.is_key_held(KeyCode::KeyW)
        || input.is_key_held(KeyCode::KeyA)
        || input.is_key_held(KeyCode::KeyS)
        || input.is_key_held(KeyCode::KeyD);
    let cancelled = !key_held
        || (active.cancel_on_move && movement_held)
        || input.right_clicked()
        || input.key_just_pressed(KeyCode::Escape);
    if cancelled {
        state.channel.pending_ends.push(active.ability_id.raw());
        state.channel.active = None;
        // Tear our local cast pose down. Server will emit
        // ChannelEnd which the binary handles as well, but
        // doing it here keeps the local view snappy.
        if let Some(pid) = crate::game::ghost_system::player_id(&state.world) {
            if let Ok(mut cast) = state
                .world
                .get::<&mut rift_engine::ecs::components::SpellCast>(pid)
            {
                cast.cancel();
            }
        }
        false
    } else {
        // Decay the local timeout. If the server's ChannelEnd
        // gets dropped this is the safety net.
        let mut a = active;
        a.remaining = (a.remaining - dt).max(0.0);
        state.channel.active = if a.remaining > 0.0 { Some(a) } else { None };
        // While channeling we suppress new ability presses so a
        // frantic player can't queue another cast on top.
        true
    }
}

/// LMB + Digit1..5 keybind dispatch. Tries each pressed slot
/// against `PlayerState::abilities`; on a successful `try_use`
/// either enters placed-targeting mode or sends the cast to
/// the server (with optimistic local cast pose / particles).
fn tick_ability_keybinds(
    state: &mut GameState,
    input: &Input,
    renderer: &mut Renderer,
    player_pos: Vec3,
    aim_dir: Vec3,
) {
    let ability_inputs = [
        input.left_clicked(),
        input.key_just_pressed(KeyCode::Digit1),
        input.key_just_pressed(KeyCode::Digit2),
        input.key_just_pressed(KeyCode::Digit3),
        input.key_just_pressed(KeyCode::Digit4),
        input.key_just_pressed(KeyCode::Digit5),
    ];

    for (i, &pressed) in ability_inputs.iter().enumerate() {
        if !pressed {
            continue;
        }
        // Resource gate: refuse the cast locally if the
        // player can't afford the ability's `resource_cost`.
        // The server runs the same check authoritatively
        // (`ServerPlayer::try_spend_resource` in
        // `crates/rift-server/src/sim/ability.rs`); blocking
        // on the client too prevents a wasted RTT and keeps
        // the cooldown / cast animation from playing for an
        // input that will be rejected. Channel costs (per-sec
        // drain) are not gated here — they're enforced by
        // the channel tick on the server.
        if let Some(Some(slot_state)) = state.player_state.abilities.slots.get(i) {
            let cost = slot_state.ability.resource_cost;
            if cost > 0.0 {
                let current_essence =
                    state.player_state.resource_pct * state.player_state.stats().max_resource;
                if cost > current_essence + 1e-3 {
                    continue;
                }
            }
        }
        let Some(ability) = state.player_state.abilities.try_use(i) else {
            continue;
        };
        let ability_clone = ability.clone();

        // Placed ability → enter targeting mode locally.
        if let TargetingMode::Placed { radius } = ability_clone.targeting {
            // Two stacked indicators: a blue "valid" ring and
            // a red "invalid" ring. Each frame we run an XZ
            // line-of-sight raycast from the caster to the
            // cursor and show exactly one of the two. Both
            // get allocated upfront so we can swap visibility
            // by editing the model matrix rather than
            // pushing/popping render objects.
            //
            // Registered as *dynamic* meshes so
            // `tick_placed_targeting` can re-bake their
            // vertices each frame against the dungeon's
            // height-field — the ring bends across raised
            // daises and sunken pits instead of clipping
            // through them.
            let initial_pos =
                crate::game::cursor::world_pos(input, renderer, 0.0).unwrap_or(player_pos);
            let height_fn = |x: f32, z: f32| match state.floor_mgr.dungeon.as_ref() {
                Some(floor) => floor.tile_floor_y_at(x, z),
                None => 0.0,
            };
            let valid_mesh = rift_engine::Mesh::targeting_circle_conformed(
                [0.2, 0.5, 1.0],
                initial_pos,
                radius,
                height_fn,
            );
            let invalid_mesh = rift_engine::Mesh::targeting_circle_conformed(
                [1.0, 0.2, 0.15],
                initial_pos,
                radius,
                height_fn,
            );
            let indicator_obj = match renderer.add_dynamic_mesh(&valid_mesh, Mat4::IDENTITY) {
                Ok(idx) => Some(idx),
                Err(_) => None,
            };
            let invalid_indicator_obj = match renderer.add_dynamic_mesh(&invalid_mesh, Mat4::ZERO) {
                Ok(idx) => Some(idx),
                Err(_) => None,
            };

            state.frame.targeting = Some(PlacedTargeting {
                slot_index: i,
                ability: ability_clone,
                radius,
                indicator_obj,
                invalid_indicator_obj,
                los_blocked: false,
            });
            break;
        }

        // Friendly target-entity ability (heals). Shift = fast
        // self-cast, otherwise enter pick-mode and let the
        // player click an ally (or themselves).
        if matches!(ability_clone.targeting, TargetingMode::TargetEntity) {
            let shift_held =
                input.is_key_held(KeyCode::ShiftLeft) || input.is_key_held(KeyCode::ShiftRight);
            if shift_held {
                if let Some(self_id) = state.net.our_net_id_cached {
                    state.net.casts.push(NetCastRequest {
                        ability_id: ability_clone.wire_id,
                        origin: player_pos,
                        aim_dir,
                        placed_target: None,
                        target_net_id: Some(self_id),
                    });
                    crate::game::ability::trigger_local_cast(
                        &ability_clone,
                        aim_dir,
                        player_pos,
                        &mut state.world,
                        renderer,
                        &mut state.player_state,
                    );
                } else {
                    // Welcome hasn't landed yet — refund the
                    // cooldown the slot just consumed so the
                    // press doesn't disappear into the void.
                    if let Some(slot) = state.player_state.abilities.slots[i].as_mut() {
                        slot.cooldown_remaining = 0.0;
                    }
                }
            } else if let Some(target_net_id) = state.selection.target_for_ability(
                SelectionRelation::Friendly,
                ability_clone.range,
                player_pos,
            ) {
                state.net.casts.push(NetCastRequest {
                    ability_id: ability_clone.wire_id,
                    origin: player_pos,
                    aim_dir,
                    placed_target: None,
                    target_net_id: Some(target_net_id),
                });
                crate::game::ability::trigger_local_cast(
                    &ability_clone,
                    aim_dir,
                    player_pos,
                    &mut state.world,
                    renderer,
                    &mut state.player_state,
                );
            } else {
                // Refund the cooldown that `try_use` just
                // consumed: targeting mode hasn't actually
                // committed the cast yet, and the server's
                // CD doesn't start until the cast arrives. If
                // we left it consumed here, picking a target
                // a few seconds later would desync our local
                // CD ahead of the server's by exactly that
                // hover time, and the *next* press at local-
                // CD-elapsed would be silently rejected by
                // the still-cooling server. Re-consumed on
                // LMB-confirm in `tick_entity_targeting`.
                if let Some(slot) = state.player_state.abilities.slots[i].as_mut() {
                    slot.cooldown_remaining = 0.0;
                }
                // Soft green hover ring under the candidate ally.
                let indicator_mesh = rift_engine::Mesh::targeting_circle([0.30, 1.00, 0.50]);
                let indicator_obj = if let Ok(()) = renderer.add_mesh(&indicator_mesh, Mat4::ZERO) {
                    Some(renderer.objects.len() - 1)
                } else {
                    None
                };
                state.frame.entity_targeting = Some(EntityTargeting {
                    slot_index: i,
                    ability: ability_clone,
                    indicator_obj,
                    hovered: None,
                });
            }
            break;
        }

        let cast_aim_dir = melee_assisted_aim(state, &ability_clone, player_pos, aim_dir);
        send_cast(
            state,
            renderer,
            &ability_clone,
            player_pos,
            cast_aim_dir,
            input,
        );
        if i == 0 {
            let _ = state
                .selection
                .select_hovered_with_relation(SelectionRelation::Hostile);
        }

        // Hold-to-channel latch. Only infinite-duration
        // channels (Frost Ray) need client-side hold/release
        // tracking — finite-duration channels (Fire Wave,
        // Whirlwind) run on the server's own clock and would
        // otherwise be cancelled by the very next frame's
        // "key not held" check, which strips the
        // ServerChannel before its first tick interval has
        // elapsed and no enemies are ever hit.
        if let Some(def) = abilities::lookup(ability_clone.wire_id) {
            if let abilities::AbilityKind::Channel {
                duration,
                cancel_on_move,
                ..
            } = def.kind
            {
                if duration.is_infinite() {
                    state.channel.active = Some(ActiveChannel {
                        ability_id: ability_clone.wire_id,
                        slot_index: i,
                        cancel_on_move,
                        // Grace period: server's ChannelEnd may
                        // arrive a frame late; this prevents a
                        // stale release from firing.
                        remaining: duration + 0.25,
                    });
                }
            }
        }

        // Local visual feedback. The server still owns the
        // damage / projectile spawn — we just play the cast
        // animation + any client-side particles immediately so
        // the input feels responsive. Melee is a regular
        // ability: its `SetPlayerAction` effect drives the
        // full-body swing pose through the same path as every
        // other client-runnable effect.
        crate::game::ability::trigger_local_cast(
            &ability_clone,
            cast_aim_dir,
            player_pos,
            &mut state.world,
            renderer,
            &mut state.player_state,
        );
    }

    tick_roll(state, input, renderer, player_pos, aim_dir);
}

fn tick_roll(
    state: &mut GameState,
    input: &Input,
    renderer: &mut Renderer,
    player_pos: Vec3,
    aim_dir: Vec3,
) {
    // Space → Evasive Roll. Roll is a passive bound to a fixed
    // key rather than the action bar.
    if input.key_just_pressed(KeyCode::Space) {
        if let Some(ability) = state.player_state.abilities.try_use_roll() {
            let ability_clone = ability.clone();
            send_cast(state, renderer, &ability_clone, player_pos, aim_dir, input);
            crate::game::ability::trigger_local_cast(
                &ability_clone,
                aim_dir,
                player_pos,
                &mut state.world,
                renderer,
                &mut state.player_state,
            );
        }
    }
}

fn melee_assisted_aim(
    state: &GameState,
    ability: &Ability,
    player_pos: Vec3,
    aim_dir: Vec3,
) -> Vec3 {
    let Some(def) = abilities::lookup(ability.wire_id) else {
        return aim_dir;
    };
    if !matches!(def.kind, abilities::AbilityKind::MeleeArc { .. }) {
        return aim_dir;
    }
    state
        .selection
        .melee_auto_aim(player_pos, aim_dir, ability.range)
        .unwrap_or(aim_dir)
}

/// Push a `NetCastRequest` for `ability` into the outbound
/// queue. Resolves the projectile origin to the casting hand
/// joint when the rig has one (so server-spawned projectiles
/// emerge from the hand), falling back to a torso-height
/// offset above the foot anchor.
///
/// Server is authoritative. Send the cast request immediately
/// for every ability kind — including projectiles — so remote
/// observers start their upper-body cast pose at network-RTT
/// latency instead of `wind_up_clip_duration + RTT` (the
/// earlier "defer until apex" path made remote poses lag the
/// local one by the full wind-up animation, which felt heavy
/// on rapid LMB attacks and Fireball Volley but not on Frost Ray
/// because channels were always sent immediately). The
/// trade-off: the server projectile now spawns at chest height
/// when the click lands, rather than from the casting hand at
/// swing apex. The local player still plays the full wind-up
/// clip for input-feedback feel.
fn send_cast(
    state: &mut GameState,
    renderer: &mut Renderer,
    ability: &Ability,
    player_pos: Vec3,
    aim_dir: Vec3,
    input: &Input,
) {
    let placed_target = if let TargetingMode::Placed { .. } = ability.targeting {
        crate::game::cursor::world_pos(input, renderer, 0.0)
    } else {
        None
    };

    // Compute a chest-height (or hand-joint) origin so
    // server-spawned projectiles don't appear to come out of
    // the ground. `player_pos` is the foot anchor (y≈0). Prefer
    // the right-hand joint's current world position from the
    // last skinning pass; fall back to a fixed +1.25m torso
    // offset which the server accepts as "trusted" within its
    // 2m sanity radius.
    let origin = {
        let pid = state
            .world
            .query::<(&Player, &LocalPlayer)>()
            .iter()
            .map(|(e, _)| e)
            .next();
        let mut hand: Option<Vec3> = None;
        if let Some(pid) = pid {
            let mut q = state
                .world
                .query_one::<(
                    &Transform,
                    &Player,
                    Option<&SkeletonBindings>,
                    Option<&Skinned>,
                )>(pid)
                .ok();
            hand = q
                .as_mut()
                .and_then(|q| q.get())
                .and_then(|(t, p, bindings, s)| {
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
                });
        }
        hand.unwrap_or(player_pos + Vec3::Y * 1.25)
    };
    state.net.casts.push(NetCastRequest {
        ability_id: ability.wire_id,
        origin,
        aim_dir,
        placed_target,
        // Landing 1 ships heal abilities without a hover-pick
        // UI — the server defaults a `None` target to the
        // caster (self-cast). Real targeting lands in a
        // follow-up.
        target_net_id: None,
    });
}
