use glam::{Mat4, Vec3};
use rift_dungeon::NavGrid;
use rift_engine::ecs::components::{
    Collider, Enemy, EnemyAnim, EnemyKind, Health, LocalPlayer, NetControlled, Renderable, Skinned, Static, Transform, Velocity,
};
use rift_engine::{Floor, FloorConfig, Mesh, Renderer};

use super::environment::EnvTextures;
use super::monster_assets::MonsterCache;
use rift_game::monsters::MonsterRole;
use crate::game::PlayerState;
use super::props::{self, Props};
use super::rift_state::RiftState;
use super::torches::TorchSystem;

/// Hub thunderstorm driver. Lives on [`FloorManager`] for the
/// duration of the player's stay in the hub and is dropped on
/// rift entry. Each frame `tick` is called from the render
/// phase: it restores the cached "calm" lighting values, then
/// — if a strike is currently flashing — overlays a brighter
/// key/ambient/fog tint and pushes a single very-large-radius
/// point light at the strike's cloud anchor so the platform
/// itself catches the white-blue rim of the bolt.
///
/// Strikes are sequenced as 1–3 sub-pulses (a short on, a
/// short off, another on, …) before settling into a longer
/// dark gap before the next strike. That cadence reads as a
/// real lightning fork rather than a metronome flash.
pub struct HubStorm {
    /// Calm-state values, captured at hub generation. Each
    /// `tick` resets the renderer to these before applying the
    /// active flash so flash bursts are purely additive and a
    /// dropped tick can't leave the hub permanently glowing.
    base_key_color: glam::Vec3,
    base_key_ambient: f32,
    base_fog_color: [f32; 3],
    /// Candidate strike positions — the visible cloud blobs
    /// that ring the platform. A new strike picks one at
    /// random.
    cloud_anchors: Vec<Vec3>,
    state: StormState,
    rng: super::props::placement::SmallRng,
}

enum StormState {
    /// Calm sky; counting down to the next strike.
    Idle { cooldown: f32 },
    /// A bolt is currently lit. `intensity` decays each frame;
    /// when it hits zero we either jump to a `Gap` (if more
    /// sub-pulses are queued) or back to `Idle`.
    Flash {
        intensity: f32,
        decay: f32,
        pos: Vec3,
        color: Vec3,
        pulses_left: u8,
    },
    /// Brief dark beat between sub-pulses of the same strike.
    Gap {
        remaining: f32,
        pos: Vec3,
        color: Vec3,
        pulses_left: u8,
    },
}

impl HubStorm {
    fn schedule_next(rng: &mut super::props::placement::SmallRng) -> StormState {
        // 4–14 seconds between strikes — enough silence that
        // each fork lands as an event rather than wallpaper.
        StormState::Idle { cooldown: rng.frange(4.0, 14.0) }
    }

    fn begin_flash(
        rng: &mut super::props::placement::SmallRng,
        anchors: &[Vec3],
        pulses_left: u8,
        carry_pos: Option<Vec3>,
        carry_color: Option<Vec3>,
    ) -> StormState {
        // Reuse the prior pulse's anchor/color when this is a
        // sub-pulse of an in-flight strike (so the bolt looks
        // like one continuous fork blinking), otherwise sample
        // a fresh cloud + slight color jitter.
        let pos = carry_pos.unwrap_or_else(|| {
            if anchors.is_empty() {
                Vec3::ZERO
            } else {
                let i = (rng.next() as usize) % anchors.len();
                anchors[i]
            }
        });
        let color = carry_color.unwrap_or_else(|| {
            // Cool-white-blue with occasional warm amber tint
            // (rare red-orange storms read as hellfire flares
            // instead of cold sky lightning).
            if rng.frange(0.0, 1.0) < 0.18 {
                Vec3::new(1.6, 0.55, 0.25)
            } else {
                Vec3::new(0.85, 0.95, 1.25)
            }
        });
        // Punchy onset, fast decay — feels like a real strike.
        StormState::Flash {
            intensity: rng.frange(1.4, 2.4),
            decay: rng.frange(8.0, 14.0),
            pos,
            color,
            pulses_left,
        }
    }

    pub fn tick(&mut self, renderer: &mut Renderer, dt: f32) {
        // Always restore base values before re-applying the
        // active flash. Torches don't run in the hub so the
        // point-light vec is owned exclusively by the storm.
        renderer.key_light.color = self.base_key_color;
        renderer.key_light.ambient = self.base_key_ambient;
        renderer.fog_color = self.base_fog_color;
        renderer.sky.cloud_flash = 0.0;
        renderer.point_lights.clear();

        // State machine step.
        let next = match self.state {
            StormState::Idle { cooldown } => {
                let cd = cooldown - dt;
                if cd <= 0.0 {
                    // 1 = single pop, 2–3 = forked multi-pulse.
                    let pulses = 1 + (self.rng.range(0, 3) as u8);
                    Self::begin_flash(&mut self.rng, &self.cloud_anchors, pulses.saturating_sub(1), None, None)
                } else {
                    StormState::Idle { cooldown: cd }
                }
            }
            StormState::Flash {
                intensity,
                decay,
                pos,
                color,
                pulses_left,
            } => {
                let next_i = intensity - decay * dt;
                if next_i <= 0.0 {
                    if pulses_left > 0 {
                        // Short dark beat, then another sub-pulse.
                        StormState::Gap {
                            remaining: self.rng.frange(0.04, 0.12),
                            pos,
                            color,
                            pulses_left: pulses_left - 1,
                        }
                    } else {
                        Self::schedule_next(&mut self.rng)
                    }
                } else {
                    StormState::Flash {
                        intensity: next_i,
                        decay,
                        pos,
                        color,
                        pulses_left,
                    }
                }
            }
            StormState::Gap {
                remaining,
                pos,
                color,
                pulses_left,
            } => {
                let r = remaining - dt;
                if r <= 0.0 {
                    Self::begin_flash(&mut self.rng, &self.cloud_anchors, pulses_left, Some(pos), Some(color))
                } else {
                    StormState::Gap { remaining: r, pos, color, pulses_left }
                }
            }
        };
        self.state = next;

        // Apply the visual contribution of an active flash.
        if let StormState::Flash { intensity, pos, color, .. } = self.state {
            let i = intensity.clamp(0.0, 3.0);
            // Lift the directional + ambient toward the bolt
            // color so the whole platform catches the strike,
            // not just the rim that the point light reaches.
            let key_lift = color * 0.35 * i;
            renderer.key_light.color = self.base_key_color + key_lift;
            renderer.key_light.ambient = self.base_key_ambient + 0.18 * i;
            // Fog brightens during the flash so the abyss
            // momentarily reveals the cloud silhouettes /
            // mountain tops that were drowning in black.
            renderer.fog_color = [
                (self.base_fog_color[0] + color.x * 0.10 * i).min(0.9),
                (self.base_fog_color[1] + color.y * 0.10 * i).min(0.9),
                (self.base_fog_color[2] + color.z * 0.10 * i).min(0.9),
            ];
            // Single huge-radius point light at the cloud
            // anchor so the bolt also rim-lights the platform
            // / chest from the strike's actual direction. The
            // shader caps point lights at 8; in the hub we
            // own all 8 slots so this is always visible.
            renderer.point_lights.push(rift_engine::PointLight {
                position: pos,
                color,
                radius: 140.0,
                intensity: 8.0 * i,
            });
            // Light up the procedural sky clouds with the
            // bolt's colour. The shader scales the flash by a
            // per-fragment hash so it reads as a fork
            // sweeping through the cumulonimbus rather than a
            // uniform fade.
            renderer.sky.cloud_flash = i;
            renderer.sky.cloud_flash_color = color;
        }
    }
}

/// Manages floor generation: creating the dungeon, spawning entities.
pub struct FloorManager {
    pub boss_room_center: Vec3,
    /// Pre-baked anchor pair for the post-boss portals: the
    /// dedicated portal room's two interior spots, in world
    /// coordinates. `(left, right)` matches the dungeon
    /// crate's convention. Mirrors `Floor::portal_anchors` for
    /// the active floor; `None` on the hub or any synthetic
    /// floor that has no portal room.
    pub portal_anchors: Option<(Vec3, Vec3)>,
    pub nav_grid: NavGrid,
    pub monsters: MonsterCache,
    pub props: Props,
    pub env: EnvTextures,
    /// Wall-mounted torch flames + warm point lights, regenerated
    /// per floor. Despawned by [`Self::clear_torches`] on floor
    /// regen.
    pub torches: TorchSystem,
    /// World position of the hub stash chest, set at the end of
    /// [`Self::generate_hub`]. `None` when the active floor is a
    /// rift floor (the chest only exists in the hub). Read by
    /// `GameState::tick_stash_chest` for the proximity prompt.
    pub stash_chest_pos: Option<Vec3>,
    /// World position of the active floor's spawn point. Updated
    /// by both [`Self::generate`] and [`Self::generate_hub`] so
    /// the rift-spawn portal can sit on top of it. Mirrors
    /// `rift_dungeon::Floor::spawn_pos` for the latest floor.
    pub spawn_pos: Vec3,
    /// Hub-only thunderstorm driver. `Some` while the player is
    /// in the hub; `None` on rift floors. Owns the cached base
    /// lighting values so per-frame flash modulation can restore
    /// them, and the cloud-anchor positions used as strike
    /// origins.
    pub hub_storm: Option<HubStorm>,
}

impl FloorManager {
    pub fn new() -> Self {
        let floor = Floor::generate(FloorConfig::for_floor(1), 42);
        Self {
            boss_room_center: Vec3::ZERO,
            portal_anchors: None,
            nav_grid: NavGrid::from_floor(&floor),
            monsters: MonsterCache::default(),
            props: Props::new(),
            env: EnvTextures::default(),
            torches: TorchSystem::new(),
            stash_chest_pos: None,
            spawn_pos: Vec3::ZERO,
            hub_storm: None,
        }
    }

    /// Generate a new floor: clear world, create dungeon, spawn player + enemies.
    pub fn generate(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        rift: &RiftState,
        player_state: &PlayerState,
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
        cosmetics: &mut super::avatar_cosmetics::AvatarCosmeticsCache,
        seed_override: Option<u64>,
    ) -> anyhow::Result<()> {
        *world = hecs::World::new();
        renderer.clear_objects();
        // Rift floors don't host the chest; clear any stale
        // hub-floor position so proximity tests can't false-fire.
        self.stash_chest_pos = None;
        // Rift floors have no thunderstorm — drop the hub
        // storm driver so it doesn't keep stomping on the
        // dungeon's torch lights / fog tint.
        self.hub_storm = None;
        // Despawn the previous floor's torch VFX before we
        // regenerate. Their `EffectId`s belong to the old
        // particle system slots; leaving them around would
        // leak emitter capacity.
        self.torches.clear(renderer);

        let config = FloorConfig::for_floor(rift.floor);
        let seed = seed_override
            .map(|s| s + rift.floor as u64 * 7)
            .unwrap_or(42 + rift.floor as u64 * 7);
        let floor = Floor::generate(config, seed);

        self.boss_room_center = floor.boss_room_center;
        self.portal_anchors = floor.portal_anchors;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Set floor theme clear color — cave-dark Diablo ambience.
        // Torches carry the warm punctuation, so the unlit base
        // is intentionally near-black.
        renderer.clear_color = match rift.floor % 4 {
            0 => [0.006, 0.004, 0.003, 1.0], // dark stone dungeon
            1 => [0.004, 0.007, 0.004, 1.0], // mossy crypts
            2 => [0.014, 0.004, 0.003, 1.0], // infernal red tint
            _ => [0.003, 0.005, 0.010, 1.0], // icy depths
        };
        // Fog color: near-black so the wall reads as smoke, not
        // distance haze. The sky horizon is tinted to match
        // (oxblood at the very edge, not crimson) so the seam
        // between fog wall and sky dome blends instead of banding.
        renderer.fog_color = [0.020, 0.008, 0.010];
        // Tighter fog for damp, claustrophobic rift floors. The
        // player still has line-of-sight to the room they're in,
        // but anything past the next doorway dissolves into the
        // black, keeping torches dramatic.
        renderer.fog_start = 6.0;
        renderer.fog_end = 22.0;

        // Crimson-and-black gloom dome. The dungeon is roofless,
        // so the player sees the sky above the parapets — paired
        // with the black fog wall it reads as a bleeding sky
        // strangled by smoke, which is the rift's whole vibe.
        renderer.sky = rift_engine::SkyConfig::rift();
        // Cave-dark key + low ambient so torches drive the look.
        renderer.key_light = rift_engine::KeyLight::DUNGEON;

        // Floor mesh — only walkable tiles, batched into one draw
        let floor_positions = floor.floor_positions();
        let floor_mesh = Mesh::dungeon_floor(&floor_positions, rift.floor);
        renderer.add_mesh(&floor_mesh, Mat4::IDENTITY)?;
        let floor_obj_idx = renderer.objects.len() - 1;

        // Walls — batched into a single draw call, themed per floor
        // (dark + slightly desaturated; torches will warm them up).
        let wall_color = match rift.floor % 4 {
            0 => Vec3::new(0.18, 0.16, 0.14), // damp weathered stone
            1 => Vec3::new(0.13, 0.18, 0.11), // deep mossy green
            2 => Vec3::new(0.24, 0.10, 0.08), // dried-blood crimson
            _ => Vec3::new(0.11, 0.15, 0.21), // glacial blue-gray
        };
        let wall_mesh = Mesh::wall_colored(wall_color);
        let wall_positions = floor.wall_positions();

        // Batch all walls into one big mesh for rendering
        let batched_walls = Mesh::batch_at_positions(&wall_mesh, &wall_positions);
        renderer.add_mesh(&batched_walls, Mat4::IDENTITY)?;
        let wall_obj_idx = renderer.objects.len() - 1;

        // Bind authored PBR materials to floor and walls. We
        // still call `ensure(...)` to keep the procedural sets
        // available as a fallback when an asset fails to load,
        // but prefer the authored brick / ground tile maps when
        // they're ready.
        self.env.ensure(renderer);
        self.env.ensure_ground_tiles(renderer);
        self.env.ensure_bricks_wall(renderer);

        // PBR material params: bit 0 of `flags` enables the PBR
        // shader path; `parallax_scale` adds a small height-map
        // displacement (tangent-space units) for stone depth;
        // `uv_scale` is multiplied into the per-vertex UVs.
        //
        // Both the floor and wall meshes ship UVs in world units
        // (1 mesh unit = 1 texture tile), so anything > 1 here
        // would shrink the pattern below 1 m per tile and look
        // both noisy and very expensive (small UVs blow out
        // mipmap caches and force the parallax march to walk
        // across many texels). We use uvScale < 1 so each tile
        // covers ~3 m of floor / wall surface, which matches
        // the apparent scale of the shipped 2k brick + ground
        // tile maps.
        //
        // Parallax stays small (and zero on floors viewed from
        // a steep top-down angle barely benefits from it) to
        // keep the per-fragment cost down.
        let pbr_flags = f32::from_bits(1u32);
        let floor_params = [1.0 / 3.0, 0.0,  pbr_flags, 0.0];
        let wall_params  = [1.0 / 3.0, 0.02, pbr_flags, 0.0];

        if let Some(set) = self.env.ground_tiles_set {
            renderer.set_object_shared_material(floor_obj_idx, set);
            renderer.set_object_material_params(floor_obj_idx, floor_params);
        } else if let Some(set) = self.env.floor_set {
            renderer.set_object_shared_material(floor_obj_idx, set);
        }
        if let Some(set) = self.env.bricks_wall_set {
            renderer.set_object_shared_material(wall_obj_idx, set);
            renderer.set_object_material_params(wall_obj_idx, wall_params);
        } else if let Some(set) = self.env.wall_set {
            renderer.set_object_shared_material(wall_obj_idx, set);
        }

        // Still need individual ECS entities for collision
        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Decorate rooms with static fantasy props (barrels, benches, candles, …).
        // Done before enemies spawn so the same seed picks consistent positions.
        props::fantasy::decorate_dungeon(&mut self.props, world, renderer, &floor, seed);

        // Wall torches: candlestick prop + looping flame VFX
        // (HDR additive → blooms) + a warm point light at each
        // sconce. The renderer caps active lights at 8, but
        // `TorchSystem::update_lights` is called every frame to
        // keep the nearest 8 to the player active.
        self.torches.place(&floor, renderer, &mut self.props, world, seed);

        // Player — spawned via shared helper so the hub generator can
        // reuse the same skinned-character + animation-set bring-up.
        let spawn = floor.spawn_pos;
        self.spawn_pos = spawn;
        self.spawn_player(world, renderer, spawn, player_state, anim_cache, cosmetics)?;

        // Enemies — server-authoritative. The floor visuals (walls,
        // props, player) are spawned here but enemy entities arrive
        // via `sync_enemies` once the server's snapshot lands.
        log::info!(
            "=== RIFT LEVEL {} === | {} rooms | enemies: server-authoritative | Kill progress needed: {:.0}",
            rift.floor,
            floor.rooms.len(),
            rift.progress_required,
        );

        Ok(())
    }

    /// Generate the safe hub / starting zone: a single small stone room
    /// with no enemies, no fog wall, no boss progress.  Returns the
    /// world-space position of the centre point where the caller should
    /// spawn the "enter the rift" portal.
    pub fn generate_hub(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        player_state: &PlayerState,
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
        cosmetics: &mut super::avatar_cosmetics::AvatarCosmeticsCache,
    ) -> anyhow::Result<Vec3> {
        *world = hecs::World::new();
        renderer.clear_objects();

        let floor = Floor::hub();
        self.boss_room_center = Vec3::ZERO;
        self.portal_anchors = None;
        self.nav_grid = NavGrid::from_floor(&floor);

        // Brooding "floating obsidian platform in a sandstorm"
        // ambience. The platform is dark stone, the sky is a
        // tan dust dome, and the fog is a warm dust haze
        // tight enough to limit visibility to ~25 m so the
        // play area reads as enclosed by airborne sand
        // rather than open desert.
        renderer.clear_color = [0.30, 0.20, 0.12, 1.0];
        // Fog colour: matches the sky's horizon band so the
        // foggy platform edge fades smoothly into the dust
        // horizon instead of showing a darker rust-coloured
        // ring against a lighter sky.
        renderer.fog_color = [0.78, 0.55, 0.30];
        // Tight fog limits the player's view: the dust
        // wall closes in within ~18 m so the platform reads
        // as a small island in a roaring storm rather than
        // a wide-open arena. Past the wall is fully veiled.
        renderer.fog_start = 4.0;
        renderer.fog_end = 18.0;
        // Default camera far plane is fine — fog swallows
        // everything past 28 m, so geometry past that won't
        // contribute even if it existed.
        renderer.camera.far = 80.0;

        // Sandstorm sky preset. Warm tan dome, no sun disc
        // (fully veiled by dust), drifting dust streaks
        // overhead.
        renderer.sky = rift_engine::SkyConfig::sandstorm_hub();
        // Diffuse warm fill — in a sandstorm the dust scatters
        // skylight uniformly so the directional contribution
        // is soft and the ambient does most of the work.
        renderer.key_light = rift_engine::KeyLight::SANDSTORM;

        // Hub floor: a single dark obsidian platform disc.
        // Slightly oversized vs. the playable square so the
        // edge clearly extends past the walkable area before
        // falling into void. Y is a hair above zero so the
        // disc top sits *just* under the avatar's feet —
        // skinned bind poses on our character pack render
        // their feet a few cm above the transform origin, so
        // a disc at exactly y=0 reads as "the character is
        // floating". Two cm of lift hides the gap without
        // looking like a step.
        let hub_centre = Vec3::new(
            (floor.width / 2) as f32,
            0.02,
            (floor.depth / 2) as f32,
        );
        const PLATFORM_RADIUS: f32 = 16.0;
        let platform = Mesh::ground_disc(
            hub_centre,
            PLATFORM_RADIUS,
            96,
            // White vertex color so the lava-rocks texture
            // shows through unmodulated. Mesh `uv_scale` of
            // `1.0/2.0` means one tile of the 2k texture
            // spans 2 m of world space — fine enough that
            // grain detail (sand specks, pebbles) reads
            // clearly at walking range, while the new
            // tightened fog (28 m) and the underlying
            // pseudo-random tile-rotation in the cel path
            // hide the repeat across the platform.
            Vec3::splat(1.0),
            1.0 / 2.0,
        );
        renderer.add_mesh(&platform, Mat4::IDENTITY)?;
        let platform_obj_idx = renderer.objects.len() - 1;
        // Bind the desert-rocks basecolor only on the cel-
        // shading path. PBR is intentionally skipped here —
        // the disc is huge and viewed top-down, so PBR
        // specular tends to wander distractingly across it as
        // the camera nudges and the normal / height detail
        // is invisible at that distance. The cel path produces
        // a calm painterly finish that doesn't shimmer. Falls
        // back to the procedural crimson-stone tile if the
        // asset fails to load.
        //
        // The actual PNG decode happens in the background
        // during character-select via
        // `EnvTextures::tick_world_preload`, so this call is
        // typically a no-op cache hit by the time we hit
        // `generate_hub`. Calling it here defensively covers
        // the corner case where the user clicked Play very
        // quickly and pre-warm hasn't reached desert_rocks
        // yet — at worst we pay the decode for one pack
        // (instead of all four like before).
        self.env.ensure_desert_rocks(renderer);
        if let Some(set) = self.env.desert_rocks_set {
            renderer.set_object_shared_material(platform_obj_idx, set);
            // Enable the PBR shading path now that the disc is
            // bound to a full sand PBR pack. uvScale stays at
            // the platform mesh's baked `1/2` so one tile of
            // the 2 k sand maps spans 2 m of world space (the
            // mesh emits world-space UVs pre-multiplied by
            // that factor, so passing `1.0` here yields the
            // intended 2 m / tile coverage). Parallax stays
            // off — the disc is viewed nearly top-down at the
            // hub camera angle, where parallax detail isn't
            // visible and would just burn fragment shader
            // cycles.
            let pbr_flags = f32::from_bits(1u32);
            renderer.set_object_material_params(
                platform_obj_idx,
                [1.0, 0.0, pbr_flags, 0.0],
            );
        } else {
            self.env.ensure_crimson_stone(renderer);
            if let Some(set) = self.env.crimson_stone_set {
                renderer.set_object_shared_material(platform_obj_idx, set);
            }
        }
        // Thin glowing crimson rim along the platform edge so
        // the floating-island silhouette reads when the player
        // walks toward it.
        let rim = Mesh::ring(
            hub_centre + Vec3::new(0.0, 0.012, 0.0),
            PLATFORM_RADIUS - 0.25,
            PLATFORM_RADIUS,
            128,
            Vec3::new(2.4, 0.35, 0.20),
        );
        renderer.add_mesh(&rim, Mat4::IDENTITY)?;

        // No underside geometry: the platform reads as a
        // floating disc above an unbounded abyss. Anything
        // hung below the disc (the old squashed ellipsoid
        // "root") tended to poke through the rim and fight
        // with the fog wall, so we just let the disc cast a
        // hard silhouette and let the fog do the rest.

        // No distant skyline. The sandstorm replaces the old
        // mountain ring: the warm-tan fog wall starts at 6 m
        // and saturates by 28 m, so anything that would sit
        // outside the platform is invisible anyway. Removing
        // the geometry also means we no longer pay for the
        // 256×24 vertex heightfield, the cliff-rocks PBR
        // pack upload, or the per-frame draw call.

        // No storm driver in the sandstorm hub — a sandstorm
        // doesn't fork lightning. We drop any pre-existing
        // driver (in case of hub-to-hub regenerate) and skip
        // creating a new one; `tick_hub_storm` is a no-op
        // when `hub_storm` is `None`.
        self.hub_storm = None;

        // Sky-anchored point light: a single warm sun-source.
        // The dungeon's dramatic shadows come from two
        // separate properties of the torch lights:
        //
        //   1. They sit *low* relative to the caster, so
        //      rays rake across the floor at a shallow angle
        //      and project shadows several metres long.
        //   2. They have a tight radius (~11 m), so the
        //      shadow look is only visible while the caster
        //      is within range.
        //
        // Property (1) is what produces the dramatic look;
        // property (2) is just a consequence of dungeon
        // geometry. For the hub we keep (1) — the anchor
        // sits low and well to the side — and *drop* (2):
        // the radius is bumped to fully cover the playable
        // disc so the player gets the same effect anywhere
        // on the platform, not just within 11 m of one
        // fixture. The cube-shadow pass produces equally
        // crisp shadows at any radius (the PCF kernel is in
        // normalised direction space, so softness is
        // constant); larger radius only means the cube's far
        // plane = radius, which still gives plenty of depth
        // precision for the small-scale casters here.
        renderer.point_lights.clear();
        // Low-angle sun: a horizontal-heavy axis (small Y)
        // so the shadow rays sweep ACROSS the platform
        // rather than punching down through it.
        let sun_axis = Vec3::new(0.70, 0.32, 0.65).normalize();
        let sun_anchor = hub_centre + sun_axis * 32.0;
        renderer.point_lights.push(rift_engine::PointLight {
            position: sun_anchor,
            color: Vec3::new(1.50, 1.00, 0.55),
            // Covers the full 16 m disc — player worst-case
            // distance from the sun anchor is ~16 m + 32 m =
            // 48 m, well inside.
            radius: 60.0,
            intensity: 4.0,
        });

        // Portal point light: the rift portal is a hot red
        // emissive ring, so we anchor a saturated crimson
        // light at its centre. Two jobs:
        //   1. Throw a red rim onto the platform stones so
        //      the disc catches the portal's glow as the
        //      player approaches — it reads as a heat
        //      source in the haze, not just a sticker.
        //   2. Cast cube-mapped shadows: anything between
        //      the portal and the camera (the player, the
        //      stash chest, dropped loot) silhouettes
        //      against the red light. Combined with the
        //      warm sun light's shadows from the opposite
        //      side this gives every prop a two-tone rim.
        // Position is at the portal centre (`first_room_center`
        // matches the `portal_pos` returned at the bottom of
        // this function) lifted to disc height (~1 m), so the
        // light sits in the middle of the visible ring. The
        // bloom pass picks up the saturated red core and the
        // portal's own emissive vertices for a strong glow.
        let portal_centre_xz = floor.first_room_center();
        let portal_light_pos =
            portal_centre_xz + Vec3::new(0.0, 1.55, 0.0);
        renderer.point_lights.push(rift_engine::PointLight {
            position: portal_light_pos,
            // Strong saturated red — push past 1.0 on the
            // red channel so HDR bloom catches it cleanly
            // even when the warm sun light is washing the
            // platform.
            color: Vec3::new(2.50, 0.20, 0.10),
            // Tight radius so the falloff is steep and the
            // red glow only paints a clear pool around the
            // portal disc; outside ~6 m it fades to nothing
            // and the warm sun light dominates again.
            radius: 7.0,
            intensity: 8.0,
        });

        // Wall colliders only — no wall mesh. Instead of the
        // dungeon grid wall ring, we ring the platform edge
        // with a chain of small AABB colliders so the player
        // can roam the full circular ground out to the
        // mountain bases. AABB-only collider type means we
        // approximate the circle with densely-packed boxes;
        // 160 segments around the circumference overlap enough
        // that no axis-aligned wedge can squeeze through.
        const COLLIDER_RING_SEGMENTS: usize = 160;
        // Sit the wall a hair inside the visible rim so the
        // player doesn't scrape against geometry that looks
        // like it should still be walkable.
        let collider_ring_radius = PLATFORM_RADIUS - 0.6;
        for i in 0..COLLIDER_RING_SEGMENTS {
            let a = (i as f32 / COLLIDER_RING_SEGMENTS as f32)
                * std::f32::consts::TAU;
            let p = hub_centre
                + Vec3::new(
                    a.cos() * collider_ring_radius,
                    2.5,
                    a.sin() * collider_ring_radius,
                );
            world.spawn((
                Transform::from_position(p),
                Collider::new(0.9, 2.5, 0.9),
                Static,
            ));
        }

        // Player stash chest. Sits a couple of tiles to the south-east
        // of the central portal so it's visible from the spawn point
        // without blocking the walk-up to the portal. Yaw rotates it
        // ~30° so the lid faces the spawn approach.
        let portal_centre = floor.first_room_center();
        let stash_pos = portal_centre + Vec3::new(2.6, 0.0, 2.2);
        self.props.spawn(
            world,
            renderer,
            &props::nature::STASH_CHEST,
            stash_pos,
            std::f32::consts::FRAC_PI_6 * -1.0,
            (0, 0),
            None,
        );
        self.stash_chest_pos = Some(stash_pos);

        let spawn = floor.spawn_pos;
        self.spawn_pos = spawn;
        self.spawn_player(world, renderer, spawn, player_state, anim_cache, cosmetics)?;

        let portal_pos = floor.first_room_center() + Vec3::new(0.0, 0.5, 0.0);
        log::info!("Hub generated. Portal at {:?}", portal_pos);
        Ok(portal_pos)
    }

    /// Shared local-player entity construction used by both rift and
    /// hub generation. Delegates to `character_spawn::spawn_character_entity`
    /// for the heavy lifting and then attaches the `LocalPlayer`
    /// marker so SP systems (camera, HUD, abilities) recognise it.
    fn spawn_player(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        spawn: Vec3,
        player_state: &PlayerState,
        anim_cache: &mut super::character_spawn::AnimLibraryCache,
        cosmetics: &mut super::avatar_cosmetics::AvatarCosmeticsCache,
    ) -> anyhow::Result<()> {
        let entity = super::character_spawn::spawn_character_entity(
            world,
            renderer,
            anim_cache,
            cosmetics,
            super::character_spawn::CharacterSpawn {
                position: spawn,
                gender: player_state.gender,
                move_speed: player_state.config.base_move_speed,
                max_hp: player_state.max_hp(),
            },
        )?;
        world.insert_one(entity, LocalPlayer).ok();
        Ok(())
    }
}

/// Spawn a remote (server-replicated) enemy's visual + ECS shell.
/// Used by `net_client::sync_enemies` when a fresh enemy `NetId`
/// shows up in a snapshot. We omit `Enemy`, `AiAgent`, and
/// `Collider` on purpose so SP combat / AI / damage systems leave
/// the entity alone — the server is sole authority for movement,
/// hits, and HP. The entity still gets `Health` so the HUD HP
/// bar can render off the snapshot's `health_pct`, and
/// `NetControlled` so any future SP gates that look for it can
/// short-circuit.
pub fn spawn_remote_enemy_entity(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    monsters: &mut MonsterCache,
    role: MonsterRole,
    position: Vec3,
    hp_max: f32,
) -> anyhow::Result<hecs::Entity> {
    // Make sure the role's shared texture is uploaded before we
    // bind it. First call per role does the upload; subsequent
    // calls return the cached descriptor set.
    let shared_set = monsters
        .slot_mut(role)
        .as_mut()
        .and_then(|a| a.ensure_shared_material(renderer));
    let asset = monsters
        .get(role)
        .ok_or_else(|| anyhow::anyhow!("monster role {role:?} not loaded"))?;

    let scaled = Mat4::from_scale_rotation_translation(
        Vec3::splat(role.scale()),
        glam::Quat::IDENTITY,
        position,
    );
    let obj_index = renderer.add_skinned_mesh(
        &asset.mesh.bind_vertices,
        &asset.mesh.vertex_skin,
        &asset.mesh.indices,
        scaled,
        0.0,
    )?;
    if let Some(set) = shared_set {
        renderer.set_object_shared_material(obj_index, set);
    }
    let skinned = Skinned { mesh: asset.mesh.clone(), scratch: Vec::new(), joint_worlds: Vec::new() };
    let initial_clip = asset
        .anims
        .find_any(&["Idle", "Idle_Loop"])
        .or_else(|| asset.anims.find_any(&["Walk", "Walk_Loop"]))
        .or_else(|| asset.anims.clips.values().next().cloned());
    let animator = initial_clip.map(rift_engine::animation::Animator::new);

    let mut builder = hecs::EntityBuilder::new();
    builder.add(Transform::from_position(position));
    builder.add(Velocity::default());
    builder.add(Health::new(hp_max));
    builder.add(Renderable { object_index: obj_index });
    builder.add(NetControlled);
    // Tag as `Enemy` so the HUD pass picks it up for floating health
    // bars + boss arrow. Speed/progress_value are server-authoritative
    // so we leave them at safe defaults; only `kind` matters visually.
    builder.add(Enemy {
        speed: 0.0,
        progress_value: 0.0,
        kind: match role {
            MonsterRole::Brute | MonsterRole::Elite | MonsterRole::Boss => EnemyKind::Brute,
            MonsterRole::Stalker => EnemyKind::Stalker,
            MonsterRole::Caster => EnemyKind::Caster,
        },
    });
    builder.add(skinned);
    builder.add(asset.anims.clone());
    if let Some(a) = animator {
        builder.add(a);
    }
    builder.add(EnemyAnim {
        last_hp: hp_max,
        attacking: false,
        lock_remaining: 0.0,
    });
    if matches!(role, MonsterRole::Boss) {
        builder.add(rift_engine::ecs::components::Boss);
    }
    Ok(world.spawn(builder.build()))
}
