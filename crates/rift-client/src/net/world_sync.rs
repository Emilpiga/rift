//! Snapshot → ECS world synchronization.
//!
//! The methods here run every frame from the binary's
//! `sync_entities_from_snapshot` phase, after `apply_snapshot` has
//! ingested the latest server state. They reconcile spawned ECS
//! entities (avatars, enemies, projectiles) with the current
//! `remote` table, drive their `Transform` / `Velocity` / `Health`
//! from the interpolated snapshot, and surface server-only state
//! (action flags, debuff masks) onto the components SP systems
//! read.

use std::time::Instant;

use glam::{Mat4, Quat, Vec3};
use rift_engine::animation::Animator;
use rift_engine::ecs::components::{
    AnimationSet, Effects, EnemyAnim, Health, LocalPlayer, NetControlled, Player, PlayerAction,
    RemotePlayer, Renderable, Resource, SkinnedAttachments, Transform, Velocity,
};
use rift_engine::Renderer;
use rift_net::{
    messages::{EntityKind, Gender},
    NetId,
};

use rift_game::character::Gender as GameGender;
use rift_game::monsters::MonsterRole;

use crate::game::character_spawn::{spawn_character_entity, AnimLibraryCache, CharacterSpawn};
use crate::game::floor::spawn_remote_enemy_entity;
use crate::game::monster_assets::MonsterCache;

use super::snapshot::RemoteProfile;
use super::NetClient;

impl NetClient {
    /// Reconcile remote-player ECS state with the latest snapshot.
    ///
    /// For every remote `NetId` in `remote` that has a known
    /// `RemoteProfile` and isn't yet spawned, we instantiate a full
    /// skinned character entity (sharing `anim_cache` with the local
    /// player path) and tag it as `RemotePlayer + NetControlled`.
    /// Then for every spawned remote, we drive Transform / Velocity
    /// / aim from the snapshot row so `locomotion_anim_system`,
    /// `skinning_system`, and `render_sync_system` (all of which
    /// run inside `GameState::update`) treat the remote like any
    /// other animated character.
    ///
    /// Local player (`our_net_id`) is intentionally skipped — its
    /// avatar lives in the SP-spawned entity that
    /// [`Self::sync_local_player`] already drives.
    pub fn sync_avatars(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        anim_cache: &mut AnimLibraryCache,
        cosmetics: &mut crate::game::avatar_cosmetics::AvatarCosmeticsCache,
    ) {
        let Some(our_net_id) = self.our_net_id else {
            // Wait for Welcome before spawning any avatars: we need
            // to know which net id is ours so we don't render an
            // avatar on top of the local player.
            return;
        };

        // ─── Despawn vanished remotes ────────────────────────────
        // Three cases drop a remote: explicit `PlayerLeft` (no
        // longer in `profiles`), a snapshot that omits the net id
        // (no longer in `remote`), or a world reset (e.g. a floor
        // regeneration `*world = World::new()` from
        // `floor_mgr.generate`) that invalidated our cached entity
        // id. The last case is tricky because hecs reuses entity
        // ids across `World::new()` resets, so `world.contains` may
        // return true for a completely unrelated new entity. We
        // verify by checking the entity still carries our
        // `RemotePlayer { net_id }` tag with the expected id.
        //
        // The `is_leave` flag distinguishes "real" departures
        // (profile dropped via `PlayerLeft` / disconnect) from
        // floor-regen-driven world resets, so we only fire the
        // rapture VFX in the leave case — a regen would otherwise
        // beam every party member up at every floor transition.
        let stale: Vec<(NetId, bool)> = self
            .avatar_entities
            .iter()
            .filter_map(|(nid, entity)| {
                let profile_gone = !self.profiles.contains_key(nid);
                let snapshot_gone = !self.remote.contains_key(nid);
                let entity_reused = match world.get::<&RemotePlayer>(*entity) {
                    Ok(rp) => rp.net_id != nid.0,
                    Err(_) => true,
                };
                if profile_gone || snapshot_gone || entity_reused {
                    // Beam-up cue only on real departures: profile
                    // dropped is unambiguous, snapshot-only drops
                    // (without a profile drop) are usually floor
                    // regens that the avatar will respawn from.
                    Some((*nid, profile_gone))
                } else {
                    None
                }
            })
            .collect();
        for (net_id, is_leave) in stale {
            if let Some(entity) = self.avatar_entities.remove(&net_id) {
                self.prev_action_byte.remove(&net_id);
                // Capture the avatar's last transform position
                // *before* despawn so we can anchor the rapture
                // VFX where the body actually was. Falls back to
                // `last_positions` if the transform was already
                // wiped (world reset path).
                let last_pos = world
                    .get::<&Transform>(entity)
                    .ok()
                    .map(|t| t.position)
                    .or_else(|| self.last_positions.get(&net_id).copied());

                // Hide the renderer slot before despawning so we
                // don't leak a frame of the old pose. Skip if the
                // entity is already dead or got reused (world reset).
                if world
                    .get::<&RemotePlayer>(entity)
                    .map(|rp| rp.net_id == net_id.0)
                    .unwrap_or(false)
                {
                    if let Ok(r) = world.get::<&Renderable>(entity) {
                        let idx = r.object_index;
                        if idx < renderer.objects.len() {
                            // Reclaim the GPU skinning slot so the
                            // freed buffers can be reused by the
                            // next monster / cosmetic to spawn.
                            renderer.free_skinned_mesh(idx);
                        }
                    }
                    // Also collapse every modular-equipment /
                    // cosmetic attachment slot. Without this the
                    // chest piece (and any other gear) the
                    // disconnecting player was wearing keeps
                    // rendering in place because its dynamic-mesh
                    // slot still has a non-zero model matrix.
                    if let Ok(atts) = world.get::<&SkinnedAttachments>(entity) {
                        for piece in &atts.pieces {
                            if piece.object_index < renderer.objects.len() {
                                renderer.free_skinned_mesh(piece.object_index);
                            }
                        }
                    }
                    // And the rigid weapon prop, if any. Dynamic
                    // meshes don't have a free path — once the
                    // entity is despawned, `update_weapon_transforms`
                    // stops touching the renderer slot, so without
                    // this the wand/sword stays frozen in mid-air
                    // at the disconnecting player's last hand pose.
                    // Zero the model matrix so it draws nothing;
                    // the slot itself is leaked (same policy used
                    // for weapon swaps).
                    if let Ok(att) =
                        world.get::<&crate::game::weapon_visuals::WeaponAttachment>(entity)
                    {
                        if let Some(obj) = renderer.objects.get_mut(att.object_index) {
                            obj.model_matrix = Mat4::ZERO;
                        }
                    }
                    let _ = world.despawn(entity);
                }

                // Beam-up: a remote player who actually left
                // (profile dropped) should read as leaving, not
                // as popping out of the world. Anchor just
                // above the feet — the body-flash layer is
                // sized to engulf the avatar silhouette
                // upward, the SilkStrand pillar shoots from
                // here toward the sky, and the ground halo
                // hugs the floor.
                if is_leave {
                    if let Some(pos) = last_pos {
                        renderer.vfx_system.spawn(
                            rift_engine::renderer::vfx::presets::player_rapture(),
                            pos + Vec3::new(0.0, 0.2, 0.0),
                        );
                    }
                }
                log::info!(
                    "net: despawned remote avatar {net_id:?} \
                     (leave={is_leave})"
                );
            }
        }

        // ─── Spawn newcomers ─────────────────────────────────────
        // Collect first to avoid holding an immutable borrow on
        // `self.remote` during `spawn_character_entity`'s mutable
        // world+renderer borrows.
        let to_spawn: Vec<(NetId, RemoteProfile, Vec3)> = self
            .remote
            .iter()
            .filter(|(nid, _)| **nid != our_net_id)
            .filter(|(nid, _)| !self.avatar_entities.contains_key(nid))
            .filter_map(|(nid, re)| {
                self.profiles
                    .get(nid)
                    .cloned()
                    .map(|p| (*nid, p, re.position))
            })
            .collect();

        for (net_id, profile, position) in to_spawn {
            let cfg = CharacterSpawn {
                position,
                gender: gender_to_game(profile.gender),
                // Speed/HP placeholders: server is authoritative for
                // both, but the components need *some* value for the
                // SP systems we share with locals.
                move_speed: rift_game::kinematic::PLAYER_SPEED,
                max_hp: 100.0,
            };
            let entity = match spawn_character_entity(world, renderer, anim_cache, cosmetics, cfg) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("net: failed to spawn remote avatar {net_id:?}: {e:?}");
                    continue;
                }
            };
            // Mark as remote + net-controlled so SP systems
            // (player_input, movement, collision) leave the entity
            // alone — we own its kinematics.
            world
                .insert(entity, (RemotePlayer { net_id: net_id.0 }, NetControlled))
                .ok();
            self.avatar_entities.insert(net_id, entity);
            log::info!(
                "net: spawned remote avatar {net_id:?} as {:?} ({:?})",
                profile.character_name,
                profile.gender,
            );
            // If we already learned this peer's visible
            // equipment from a `PeerEquipmentVisuals` that
            // arrived before the avatar existed, re-queue it
            // now so the binary's apply pass dresses the
            // avatar this frame.
            if let Some(base_ids) = self.peer_visuals_mirror.get(&profile.client_id).cloned() {
                self.pending_peer_equipment_visuals
                    .retain(|(cid, _)| *cid != profile.client_id);
                self.pending_peer_equipment_visuals
                    .push_back((profile.client_id, base_ids));
            }
        }

        // ─── Drive remote kinematics from snapshot ───────────────
        // Position + yaw come from the per-remote interp buffer:
        // we render `prev → curr` blended by an alpha derived from
        // the time since `curr` arrived, with one snapshot period
        // of intentional lag so we always have a sample to
        // interpolate towards. Velocity is the latest known value
        // (not interpolated) so the animation tier picker can react
        // immediately when the remote starts/stops moving.
        let now = Instant::now();
        for (&net_id, &entity) in &self.avatar_entities {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            let (display_pos, display_yaw, display_aim_yaw) = match self.interp_sample(net_id, now)
            {
                Some(s) => (s.position, s.yaw, s.aim_yaw),
                None => {
                    let aim_yaw = match re.kind {
                        EntityKind::Player { aim_yaw, .. } => aim_yaw,
                        _ => re.yaw,
                    };
                    (re.position, re.yaw, aim_yaw)
                }
            };
            if let Ok(mut t) = world.get::<&mut Transform>(entity) {
                t.position = display_pos;
                t.rotation = Quat::from_rotation_y(display_yaw);
            }
            // Foot-IK reference plane for remote avatars. The
            // server sends position with the resolved ground Y
            // already baked in, so `display_pos.y` is the
            // grounded plane (modulo airborne windows, which
            // the IK gates off via swing-phase weight anyway).
            // Remote players don't have a kinematic running
            // locally so this is the only place their
            // `grounded_y` ever gets written.
            if matches!(re.kind, EntityKind::Player { .. }) {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.grounded_y = display_pos.y;
                }
            }
            // Velocity drives `locomotion_anim_system`'s
            // Idle/Walk/Jog/Sprint pick. Server already sends
            // world-space horizontal velocity. Take the latest
            // value (not interpolated) so the animation tier
            // changes the same frame movement starts/stops.
            if let Ok(mut v) = world.get::<&mut Velocity>(entity) {
                v.linear = re.velocity;
            }
            // Aim direction (for spine twist + remote channel
            // beams). Slerped above so it tracks at render rate
            // instead of jumping at the snapshot rate.
            if matches!(re.kind, EntityKind::Player { .. }) {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.aim_dir = Vec3::new(display_aim_yaw.sin(), 0.0, display_aim_yaw.cos());
                }
                // Mirror health_pct onto the remote avatar's Health
                // component so HUD widgets (e.g. world-space remote
                // health bars) can read it the same way they read
                // enemy / local-player health. `Health.max` was set
                // to a placeholder at spawn — the bar code only
                // looks at `current / max`, so the placeholder is
                // fine as long as we keep `current` in sync.
                if let Ok(mut h) = world.get::<&mut Health>(entity) {
                    h.current = h.max * re.health_pct;
                }
                // Same trick for the essence / mana bar — mirror
                // the snapshot's `resource_pct` onto the remote
                // avatar's Resource component so the world-space
                // teammate essence bar can read it the same way
                // it reads health. `Resource.max` was placeholder
                // 1.0 at spawn; the bar code reads only the ratio.
                if let Ok(mut r) = world.get::<&mut Resource>(entity) {
                    r.current = r.max * re.resource_pct;
                }
            }
            // Jump: when the snapshot says the remote is airborne,
            // tag its `Player.action = JumpAir` and cross-fade to
            // the air clip. `locomotion_anim_system` early-returns
            // when `action != None`, so the air pose stays put for
            // as long as the snapshot reports airborne. On
            // touchdown we snap back to None and locomotion takes
            // over the next frame.
            let was_airborne = world
                .get::<&Player>(entity)
                .map(|p| matches!(p.action, PlayerAction::JumpAir))
                .unwrap_or(false);
            if re.airborne != was_airborne {
                if re.airborne {
                    if let Ok(mut p) = world.get::<&mut Player>(entity) {
                        p.action = PlayerAction::JumpAir;
                        p.action_timer = 0.0;
                    }
                    let clip = world
                        .get::<&AnimationSet>(entity)
                        .ok()
                        .and_then(|s| s.find_any(&["Jump", "Jump_Loop", "Jump_Air"]));
                    if let Some(clip) = clip {
                        if let Ok(mut anim) = world.get::<&mut Animator>(entity) {
                            anim.cross_fade(clip, true, 0.10);
                            anim.speed = 1.0;
                        }
                    }
                } else if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::None;
                    p.action_timer = 0.0;
                }
            }

            // Dodge-roll: drive the roll clip on the remote avatar
            // while the snapshot reports an active roll action.
            // Mirrors what `set_player_action` does on the local
            // path — sets `Player.action = Roll` so the SP
            // locomotion picker steps aside, then cross-fades the
            // roll clip. Cleared as soon as the snapshot flips back
            // to `NONE` (server's roll timer expired).
            let snap_rolling = re.action == rift_game::kinematic::action::ROLL;
            let was_rolling = world
                .get::<&Player>(entity)
                .map(|p| matches!(p.action, PlayerAction::Roll))
                .unwrap_or(false);
            if snap_rolling && !was_rolling {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::Roll;
                    p.action_timer = rift_game::kinematic::ROLL_DURATION;
                    p.aim_dir = Vec3::new(re.yaw.sin(), 0.0, re.yaw.cos());
                }
                let clip = world
                    .get::<&AnimationSet>(entity)
                    .ok()
                    .and_then(|s| s.find_any(&["Roll", "Roll_Forward", "Dodge_Roll", "Dodge"]));
                if let Some(clip) = clip {
                    if let Ok(mut anim) = world.get::<&mut Animator>(entity) {
                        anim.cross_fade(clip, false, 0.08);
                        anim.speed = 1.0;
                    }
                }
            } else if !snap_rolling && was_rolling {
                if let Ok(mut p) = world.get::<&mut Player>(entity) {
                    p.action = PlayerAction::None;
                    p.action_timer = 0.0;
                }
            }

            // Melee swing mirror. The server stamps one of
            // `action::ATTACK_A..ATTACK_D` on the swinger's
            // kinematic when an `AbilityKind::MeleeArc` cast
            // dispatches, with the byte encoding the combo
            // step (0\u20133). We detect *any* transition into
            // (or between) those bytes and play the matching
            // upper-body swing clip via the remote avatar's
            // `SpellCast` layer so the swing rides on top of
            // locomotion via the upper-body mask. The
            // shared clip table in
            // `rift_game::kinematic::MELEE_COMBO_CLIPS`
            // keeps the local and remote paths in sync.
            let prev = self.prev_action_byte.get(&net_id).copied().unwrap_or(0);
            let curr_attack = rift_game::kinematic::action::is_attack(re.action);
            let prev_attack = rift_game::kinematic::action::is_attack(prev);
            let stepped = curr_attack && (!prev_attack || prev != re.action);
            if stepped {
                let step = rift_game::kinematic::action::attack_step(re.action);
                let candidates = rift_game::kinematic::MELEE_COMBO_CLIPS[(step & 0b11) as usize];
                let clip = world
                    .get::<&AnimationSet>(entity)
                    .ok()
                    .and_then(|s| s.find_any(candidates));
                if let Some(clip) = clip {
                    if let Ok(mut cast) =
                        world.get::<&mut rift_engine::ecs::components::SpellCast>(entity)
                    {
                        cast.play_oneshot(clip);
                    }
                }
            }
            self.prev_action_byte.insert(net_id, re.action);

            // Mirror the snapshot's active-effect list into the
            // remote avatar's `Effects` component. Drives buff /
            // debuff icons + duration rings in the HUD the same
            // way enemy effects do.
            sync_effects(world, entity, &re.effects);
        }
        // Drop interp buffers for entities that have despawned so
        // the map doesn't grow unbounded across long sessions.
        self.interp
            .retain(|nid, _| self.avatar_entities.contains_key(nid));
    }

    /// Reconcile server-replicated enemy entities with the latest
    /// snapshot. Spawns a skinned monster ECS entity for any new
    /// `EntityKind::Enemy` row, drives its `Transform` / `Velocity`
    /// / `Health` from the snapshot, and despawns any previously
    /// known enemy that's no longer in the snapshot (server-side
    /// death or floor change).
    ///
    /// The enemy entity intentionally does NOT carry the SP
    /// `Enemy` / `AiAgent` / `Collider` components — server is
    /// authoritative for movement, hits, and death. We add
    /// `NetControlled` so any future SP gate that filters by it
    /// short-circuits cleanly.
    pub fn sync_enemies(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        monsters: &mut MonsterCache,
    ) {
        if self.our_net_id.is_none() {
            return;
        }

        // ── Despawn vanished enemies ────────────────────────────
        let stale: Vec<NetId> = self
            .enemy_entities
            .iter()
            .filter(|(nid, _)| !self.remote.contains_key(nid))
            .map(|(nid, _)| *nid)
            .collect();
        for net_id in stale {
            if let Some(entity) = self.enemy_entities.remove(&net_id) {
                // Decide whether this despawn is a *real death*
                // or just a view-cull. The snapshot is view-
                // culled server-side (see
                // `crates/rift-server/src/sim/snapshot.rs`), so
                // an enemy walking out of the player's bubble
                // also drops out of `self.remote` — without the
                // explicit `dead_net_ids` set we'd happily play
                // a soul-return puff for an enemy that's still
                // very much alive 30m away.
                //
                // `dead_net_ids` is populated by the reliable
                // `WorldEvent::Death` handler in `main.rs` the
                // instant the kill lands, but we deliberately
                // don't *spawn* the soul-return puff there —
                // the server holds the corpse on snapshots for
                // `DEATH_FADE_DUR` (~1.6s) playing the death
                // clip, so the VFX needs to fire here, when the
                // body actually drops, to read as "smoking back
                // down to hell" instead of floating above a
                // still-visible enemy.
                let died = self.dead_net_ids.remove(&net_id);
                let died_pos = if died {
                    world
                        .get::<&Transform>(entity)
                        .ok()
                        .map(|t| t.position)
                        .or_else(|| self.last_positions.get(&net_id).copied())
                } else {
                    None
                };
                if let Some(pos) = died_pos {
                    let eid = renderer.vfx_system.spawn(
                        rift_engine::renderer::vfx::presets::enemy_soul_return(),
                        pos + Vec3::new(0.0, 0.6, 0.0),
                    );
                    log::info!(
                        "vfx: spawned enemy_soul_return at despawn \
                         net_id={net_id:?} pos={pos:?} eid={eid:?}"
                    );
                }
                if let Ok(r) = world.get::<&Renderable>(entity) {
                    let idx = r.object_index;
                    if idx < renderer.objects.len() {
                        // Reclaim the GPU skinning slot — the
                        // dispatch queue, output VB and palette
                        // UBOs free up after MAX_FRAMES_IN_FLIGHT
                        // frames and become available for the next
                        // spawn. Critical for endless-density
                        // floors where hundreds of monsters die
                        // before the floor wipes.
                        renderer.free_skinned_mesh(idx);
                    }
                }
                let _ = world.despawn(entity);
            }
        }

        // ── Spawn newcomers ─────────────────────────────────────
        // Cap at a few spawns per frame: each spawn does a
        // synchronous GPU mesh upload + texture bind, and a fresh
        // floor can have hundreds of enemies. Doing them all in a
        // single frame stalls the renderer for seconds. Remaining
        // enemies stream in over the next handful of frames as
        // their NetIds keep showing up in snapshots.
        //
        // Sized so a typical deep-floor pack (40–60 enemies in a
        // single arena) finishes streaming in within ~3 frames
        // after first sight — the previous cap of 8 left enemies
        // invisible for ~200 ms on entry, which on deeper floors
        // showed up as "client takes damage from nothing".
        const MAX_SPAWNS_PER_FRAME: usize = 24;
        let to_spawn: Vec<(NetId, u8, Vec3, f32)> = self
            .remote
            .iter()
            .filter(|(nid, _)| !self.enemy_entities.contains_key(nid))
            .filter_map(|(nid, re)| match re.kind {
                EntityKind::Enemy { role, .. } => Some((*nid, role, re.position, re.health_pct)),
                _ => None,
            })
            .take(MAX_SPAWNS_PER_FRAME)
            .collect();
        if !to_spawn.is_empty() {
            log::info!(
                "net: sync_enemies spawning {} of {} enemy rows in `remote`",
                to_spawn.len(),
                self.remote
                    .values()
                    .filter(|re| matches!(re.kind, EntityKind::Enemy { .. }))
                    .count(),
            );
        }
        for (net_id, role_byte, position, hp_pct) in to_spawn {
            let role = match MonsterRole::from_wire_byte(role_byte) {
                Some(r) => r,
                None => continue,
            };
            // We don't know hp_max from the wire (only health_pct).
            // Pick a sane default so HUD bar math works; the actual
            // current value is overwritten from health_pct each
            // frame anyway.
            let hp_max = 100.0_f32;
            let hp = hp_max * hp_pct;
            match spawn_remote_enemy_entity(world, renderer, monsters, role, position, hp_max) {
                Ok(entity) => {
                    if let Ok(mut h) = world.get::<&mut Health>(entity) {
                        h.current = hp;
                    }
                    self.enemy_entities.insert(net_id, entity);
                    log::info!(
                        "net: spawned remote enemy {net_id:?} role={role:?} at {position:?}"
                    );
                }
                Err(e) => {
                    log::warn!("net: failed to spawn remote enemy {net_id:?} role={role:?}: {e:?}");
                }
            }
        }

        // ── Drive remote-enemy kinematics from snapshot ─────────
        let now = Instant::now();
        for (&net_id, &entity) in &self.enemy_entities {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            let (display_pos, display_yaw) = match self.interp_sample(net_id, now) {
                Some(s) => (s.position, s.yaw),
                None => (re.position, re.yaw),
            };
            if let Ok(mut t) = world.get::<&mut Transform>(entity) {
                t.position = display_pos;
                t.rotation = Quat::from_rotation_y(display_yaw);
            }
            if let Ok(mut v) = world.get::<&mut Velocity>(entity) {
                v.linear = re.velocity;
            }
            if let Ok(mut h) = world.get::<&mut Health>(entity) {
                // Treat health_pct as the canonical source of truth
                // for current/max ratio. Keep `max` stable from
                // spawn so HUD bars don't jitter when the server's
                // hp_max disagrees with our placeholder.
                h.current = h.max * re.health_pct;
            }
            // Surface the server's anim byte by writing into
            // EnemyAnim.attacking — the SP animation tier picker
            // for skinned enemies reads it to swap to the attack
            // clip. WALK / IDLE are picked by the locomotion
            // animation system off `Velocity` (already set above).
            if let EntityKind::Enemy { anim, .. } = re.kind {
                if let Ok(mut ea) = world.get::<&mut EnemyAnim>(entity) {
                    ea.attacking = anim == 2; // server::sim::enemy_anim::ATTACK
                }
            }
            sync_effects(world, entity, &re.effects);
        }
    }

    /// Reconcile renderer projectile slots with the latest snapshot.
    ///
    /// Each projectile gets:
    ///   * a glowing fireball mesh (driven from the dead-reckoned
    ///     render position, not the raw 20 Hz snapshot — see
    ///     [`super::ProjectileRender`]),
    ///   * a persistent VFX trail emitter that's re-anchored to
    ///     the same render position every frame, and
    ///   * a one-shot `fireball_explosion` spawned at the last
    ///     known position the instant it disappears from
    ///     snapshots (server hit, expiry, wall collision), so
    ///     the projectile reads as detonating instead of just
    ///     popping out of existence.
    pub fn sync_projectiles(
        &mut self,
        renderer: &mut Renderer,
        audio: Option<&mut rift_audio::AudioSystem>,
        dt: f32,
    ) {
        if self.our_net_id.is_none() {
            return;
        }
        // Borrow the audio system for the whole pass. We use it
        // both in the despawn (impact one-shot + travel emitter
        // teardown) and the spawn (travel loop attach) branches,
        // so a single `&mut` here is simpler than threading the
        // option through each helper.
        let mut audio = audio;

        // Despawn vanished projectiles: hide the mesh slot, kill
        // the trail emitter, and detonate at the last known
        // render position so the projectile reads as exploding.
        let stale: Vec<NetId> = self
            .projectile_objects
            .iter()
            .filter(|(nid, _)| !self.remote.contains_key(nid))
            .map(|(nid, _)| *nid)
            .collect();
        for net_id in stale {
            if let Some(idx) = self.projectile_objects.remove(&net_id) {
                if idx < renderer.objects.len() {
                    renderer.objects[idx].model_matrix = Mat4::ZERO;
                }
            }
            if let Some(trail_id) = self.projectile_trails.remove(&net_id) {
                renderer.vfx_system.despawn(trail_id);
            }
            // Tear down the travel-loop emitter (silence the
            // whoosh) before we play the impact one-shot so
            // the two cues don't briefly overlap on top of
            // each other at the same position.
            let travel_em = self.projectile_audio.remove(&net_id);
            let ability_id = self.projectile_ability.remove(&net_id);
            if let (Some(em), Some(audio)) = (travel_em, audio.as_deref_mut()) {
                audio.despawn_emitter(em);
            }
            if let Some(visual) = self.projectile_render.remove(&net_id) {
                // Stored at spawn, so no per-ability branch here.
                let burst = rift_engine::combat::effect_for_vfx(visual.impact);
                // Pull the burst back along the last known
                // travel direction so it sits *in front of*
                // the wall it just hit instead of buried
                // inside the geometry. The client has been
                // dead-reckoning `render_pos` forward at the
                // snapshot velocity every frame; on a wall
                // collision the server simply stops including
                // the projectile, and the last extrapolated
                // position is therefore typically a half-frame
                // past the contact surface. Without this nudge
                // the entire fireball — light, smoke, ember
                // shockwave — drowns inside the wall and reads
                // as "the projectile vanished".
                let speed = visual.anchor_vel.length();
                let impact_pos = if speed > 1e-3 {
                    let dir = visual.anchor_vel / speed;
                    // ~0.6 m back-off: enough to clear common
                    // wall thicknesses and the projectile's own
                    // mesh radius, small enough that the player
                    // still reads it as a wall hit and not a
                    // mid-air detonation.
                    visual.render_pos - dir * 0.6
                } else {
                    visual.render_pos
                };
                renderer.vfx_system.spawn_bundle(burst, impact_pos);

                // Impact SFX, anchored at the same back-off-
                // adjusted position the burst uses so the
                // sound and the ember shockwave originate
                // from the same point. Silent for abilities
                // whose audio table entry has no `impact`.
                if let (Some(ability_id), Some(audio)) = (ability_id, audio.as_deref_mut()) {
                    let recipe = crate::game::ability_audio::audio_for(
                        rift_game::abilities::AbilityWireId::new(ability_id),
                    );
                    if let Some(path) = recipe.impact {
                        let mut spec = crate::game::ability_audio::impact_spec(path);
                        crate::game::ability_audio::jitter_one_shot(&mut spec);
                        audio.play_one_shot(&spec, impact_pos);
                    }
                }
            }
        }

        // Spawn newcomers. Allocate the mesh slot first, then
        // attach a trail emitter at the spawn position so the
        // first frame already has visible embers. Mesh / trail /
        // impact are pulled directly from the ability's
        // `ShapeVisuals::Projectile` recipe — no per-ability
        // branches in this file.
        use rift_game::abilities::ShapeVisuals;
        let to_spawn: Vec<(NetId, Vec3, Vec3, f32, u16)> = self
            .remote
            .iter()
            .filter(|(nid, _)| !self.projectile_objects.contains_key(nid))
            .filter_map(|(nid, re)| match re.kind {
                EntityKind::Projectile { ability } => {
                    Some((*nid, re.position, re.velocity, re.yaw, ability))
                }
                _ => None,
            })
            .collect();
        for (net_id, pos, vel, yaw, ability) in to_spawn {
            // Look up the projectile's visual recipe. Skip the
            // spawn if either the ability is unknown or it
            // declares a non-projectile shape — defensive
            // guards, both should always succeed for a
            // snapshot-borne `EntityKind::Projectile`.
            let Some(ab) = rift_game::abilities::lookup(rift_game::abilities::AbilityWireId::new(
                ability as u8,
            )) else {
                continue;
            };
            let ShapeVisuals::Projectile {
                mesh,
                trail,
                impact,
                scale,
            } = ab.visuals.shape
            else {
                continue;
            };
            let mesh_obj = rift_engine::combat::mesh_for_kind(mesh);
            if renderer.add_mesh(&mesh_obj, Mat4::ZERO).is_ok() {
                let idx = renderer.objects.len() - 1;
                self.projectile_objects.insert(net_id, idx);
                let trail_id = renderer
                    .vfx_system
                    .spawn_bundle(rift_engine::combat::effect_for_vfx(trail), pos);
                self.projectile_trails.insert(net_id, trail_id);

                // Travel-loop audio. Looping spatial emitter
                // anchored at the spawn point — the per-frame
                // pass below re-anchors it to the dead-reckoned
                // render position so the loop tracks the
                // projectile through its flight, not just the
                // 20 Hz snapshot positions. Cast as `u8`
                // because `EntityKind::Projectile.ability` is
                // wire-encoded — same value the abilities
                // registry uses.
                self.projectile_ability.insert(net_id, ability as u8);
                if let Some(audio) = audio.as_deref_mut() {
                    let recipe = crate::game::ability_audio::audio_for(
                        rift_game::abilities::AbilityWireId::new(ability as u8),
                    );
                    if let Some(path) = recipe.travel {
                        let spec = crate::game::ability_audio::travel_spec(path);
                        if let Some(em) = audio.spawn_emitter(&spec, pos) {
                            self.projectile_audio.insert(net_id, em);
                        }
                    }
                }
                self.projectile_render.insert(
                    net_id,
                    super::ProjectileRender {
                        render_pos: pos,
                        anchor_pos: pos,
                        anchor_vel: vel,
                        yaw,
                        impact,
                        scale,
                    },
                );
            }
        }

        // Drive transforms + trail anchors. For each projectile:
        // detect when a fresh snapshot has landed (anchor_pos no
        // longer matches re.position) and snap to it; otherwise
        // dead-reckon forward at the snapshot's velocity. This
        // keeps the visual silky between the 20 Hz snapshots
        // without introducing the snapshot-rate stutter that
        // raw `re.position` produced.
        for (&net_id, &idx) in &self.projectile_objects {
            let Some(re) = self.remote.get(&net_id) else {
                continue;
            };
            let Some(visual) = self.projectile_render.get_mut(&net_id) else {
                continue;
            };
            // New snapshot? Snap to the authoritative position
            // and refresh the velocity used for extrapolation.
            // We allow a small epsilon so float jitter doesn't
            // re-trigger snaps every frame.
            if (re.position - visual.anchor_pos).length_squared() > 1e-6 {
                visual.render_pos = re.position;
                visual.anchor_pos = re.position;
                visual.anchor_vel = re.velocity;
                visual.yaw = re.yaw;
            }
            // Extrapolate forward this frame. Even on snap
            // frames this just nudges by `vel * dt` past the
            // snap point, which is the correct continuation.
            visual.render_pos += visual.anchor_vel * dt;

            if idx < renderer.objects.len() {
                let scale = Vec3::splat(visual.scale);
                renderer.objects[idx].model_matrix = Mat4::from_translation(visual.render_pos)
                    * Mat4::from_rotation_y(visual.yaw)
                    * Mat4::from_scale(scale);
            }
            if let Some(&trail_id) = self.projectile_trails.get(&net_id) {
                renderer.vfx_system.set_anchor(trail_id, visual.render_pos);
            }
            // Re-anchor the travel-loop emitter so the whoosh
            // tracks the projectile's render position, not
            // its (jittery) raw snapshot position. Audio's
            // own short tween smooths any per-frame motion.
            if let (Some(&em), Some(audio)) =
                (self.projectile_audio.get(&net_id), audio.as_deref_mut())
            {
                audio.set_emitter_position(em, visual.render_pos);
            }
        }
    }

    /// Drive the local SP `Player` entity's `Transform` from our
    /// predicted state, plus the residual smooth-correction error.
    /// Called from the binary BEFORE `GameState::update` so SP's
    /// `camera_follow_system` and `render_sync_system` (both run
    /// inside `update`) see the predicted position. We also zero
    /// the player's `Velocity` so `movement_system` becomes a
    /// no-op for the local player — we own kinematics now.
    ///
    /// Y is intentionally preserved from whatever the SP path
    /// last wrote so we don't fight any vertical animation/bob/
    /// foot-placement logic the engine owns. The server only
    /// cares about XZ collision anyway.
    ///
    /// SP code keeps owning `Player.action`, animations,
    /// abilities, equipment, etc.
    pub fn sync_local_player(&mut self, world: &mut hecs::World) {
        if !self.predicted_ready {
            return;
        }
        // Visible position bleeds the residual error away over
        // time so corrections aren't visually abrupt. We also
        // extrapolate the XZ component by `predicted.velocity *
        // time_since_last_predict` so the visual stays smooth at
        // render rates above the 60 Hz prediction cadence — without
        // this the avatar visibly steps every 16.6 ms while
        // running, which is especially obvious mid-jump (the
        // vertical axis is silky 120 Hz, so the diagonal motion
        // reads as a stutter even though XZ alone moves at 60 Hz).
        let extrap_dt = self.input_accumulator.as_secs_f32().min(1.0 / 30.0);
        let mut extrap = self.predicted.velocity * extrap_dt;
        // Vertical extrap is zero — Y is owned by the local
        // gravity sim and we don't want this XZ-only smoothing
        // to perturb it.
        extrap.y = 0.0;
        let mut visible = self.predicted.position + self.correction_error + extrap;

        // Vertical smoothing: the kinematic snaps to per-tile
        // floor elevation instantly (so collision and projectile
        // arcs use the authoritative height), but the visible
        // mesh + camera are driven through `visual_y` which
        // exponentially chases the kinematic Y. Stepping onto
        // a raised dais then reads as a smooth lift over a
        // few frames instead of a teleport. While airborne we
        // disable the smoothing — the gravity arc is already
        // continuous, and lerping toward it would visibly lag
        // the jump apex.
        const Y_SMOOTH_TAU: f32 = 0.12; // ~95 % converged in 0.36 s
        const Y_TELEPORT_THRESHOLD: f32 = 1.5; // step taller than this = portal/respawn
        let target_y = visible.y;
        let smoothed_y = match self.visual_y {
            Some(prev) => {
                let dt = extrap_dt.max(1.0e-4);
                if (target_y - prev).abs() > Y_TELEPORT_THRESHOLD || self.predicted.airborne {
                    target_y
                } else {
                    let alpha = 1.0 - (-(dt / Y_SMOOTH_TAU)).exp();
                    prev + (target_y - prev) * alpha
                }
            }
            None => target_y,
        };
        self.visual_y = Some(smoothed_y);
        visible.y = smoothed_y;

        let yaw = self.predicted.yaw;
        // Authoritative grounded plane Y for the foot-IK pass.
        // For net-controlled players `movement_system` skips
        // vertical integration entirely (the prediction loop
        // owns it), which means `Player.grounded_y` would
        // otherwise stay frozen at its spawn value. The IK
        // reads `grounded_y` as its reference plane, so a
        // stale value translates directly into a constant
        // vertical offset where the avatar appears wedged
        // half a metre into the ground (one elevation step).
        // The kinematic's `predicted.position.y` is exactly
        // the resolved ground height when not airborne — use
        // it directly. While airborne we still write through
        // so a falling player keeps a sensible reference (the
        // last grounded plane), but the IK pass already gates
        // off via swing-phase weight so the value won't be
        // applied wrongly.
        let predicted_y = self.predicted.position.y;
        let mut needs_marker: Vec<hecs::Entity> = Vec::new();
        for (entity, (transform, player, _local, marker)) in world.query_mut::<(
            &mut Transform,
            &mut Player,
            &LocalPlayer,
            Option<&NetControlled>,
        )>() {
            // Override XZ + Y from the predicted state. The
            // kinematic now owns vertical motion too: gravity
            // + per-tile ground follow including stair-tile
            // slope interpolation. Letting `movement_system`
            // run its own y=0 clamp instead would warp the
            // avatar back up out of sunken pits and snap it
            // off raised daises.
            transform.position.x = visible.x;
            transform.position.y = visible.y;
            transform.position.z = visible.z;
            transform.rotation = Quat::from_rotation_y(yaw);
            player.grounded_y = predicted_y;
            if marker.is_none() {
                needs_marker.push(entity);
            }
        }
        for e in needs_marker {
            let _ = world.insert_one(e, NetControlled);
        }

        // Server-authoritative HP: the local player's snapshot row
        // carries `health_pct`. Mirror it onto the SP `Health`
        // component so the HUD HP bar reflects damage taken from
        // server-side enemy hits without us locally subtracting.
        if let Some(our_id) = self.our_net_id {
            if let Some(re) = self.remote.get(&our_id) {
                let target_pct = re.health_pct;
                for (_e, (_p, _l, h)) in world.query_mut::<(&Player, &LocalPlayer, &mut Health)>() {
                    h.current = h.max * target_pct;
                }
                // Mirror active effects onto the local player's
                // `Effects` so the HUD can render buff icons +
                // duration rings on the player nameplate the same
                // way it does for remotes.
                let entity = world
                    .query::<(&Player, &LocalPlayer)>()
                    .iter()
                    .map(|(e, _)| e)
                    .next();
                if let Some(entity) = entity {
                    sync_effects(world, entity, &re.effects);
                }
            }
        }
    }
}

/// Bridge between the wire enum (`rift_net`) and the in-game
/// enum (`rift_game::character`). Done here, not in either crate,
/// so neither has to depend on the other.
fn gender_to_game(g: Gender) -> GameGender {
    match g {
        Gender::Male => GameGender::Male,
        Gender::Female => GameGender::Female,
    }
}

/// Public re-export so other client modules (the main loop's
/// equipment-visuals dispatcher in particular) can convert a
/// peer's wire gender into the gameplay enum without touching
/// the conversion glue here.
pub fn wire_gender_to_game(g: Gender) -> GameGender {
    gender_to_game(g)
}

/// Mirror a snapshot row's `effects` list into the entity's
/// `Effects` component, inserting it on first sight. The wire
/// type and the engine type carry the same fields but live in
/// separate crates (engine doesn't depend on rift-net), so we
/// convert here.
fn sync_effects(
    world: &mut hecs::World,
    entity: hecs::Entity,
    src: &[rift_net::messages::ActiveEffect],
) {
    let effects: Vec<rift_engine::ecs::components::ActiveEffect> = src
        .iter()
        .map(|e| rift_engine::ecs::components::ActiveEffect {
            id: e.id,
            remaining: e.remaining,
            duration: e.duration,
        })
        .collect();
    if let Ok(mut d) = world.get::<&mut Effects>(entity) {
        d.effects = effects;
        return;
    }
    let _ = world.insert_one(entity, Effects { effects });
}
