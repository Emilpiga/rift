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

        // Bind procedural stone textures to floor and walls.
        self.env.ensure(renderer);
        if let Some(set) = self.env.floor_set {
            renderer.set_object_shared_material(floor_obj_idx, set);
        }
        if let Some(set) = self.env.wall_set {
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

        // Wall torches: looping flame VFX (HDR additive → blooms)
        // + a warm point light at each sconce. The renderer caps
        // active lights at 8, but `TorchSystem::update_lights` is
        // called every frame to keep the nearest 8 to the player
        // active.
        self.torches.place(&floor, renderer, seed);

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
        self.nav_grid = NavGrid::from_floor(&floor);

        // Brooding "floating obsidian platform over an abyss"
        // ambience. The platform is dark stone, the sky is a
        // crimson thunderstorm, and the fog is a dark crimson-
        // grey haze (NOT pure black) so it reads as a *visible*
        // wall of mist that swallows the mountain bases and
        // platform underside, instead of blending invisibly
        // into the abyss.
        renderer.clear_color = [0.02, 0.01, 0.02, 1.0];
        // Fog color is matched to the sky's `ground` band so
        // the seam where mountain bases meet the horizon
        // blends seamlessly — no visible cutoff line where
        // geometry ends and sky begins. Keep this in sync with
        // `SkyConfig::abyss_hub`'s `ground` value.
        renderer.fog_color = [0.06, 0.025, 0.035];
        // Tight fog: starts close enough to grip the platform
        // edge, ends just past the mountain ridge so the
        // peaks read as silhouettes rising out of the haze
        // and the bases dissolve completely into it.
        renderer.fog_start = 12.0;
        renderer.fog_end = 50.0;

        // Crimson stormy sky. Brooding overhead, fire-orange
        // band on the horizon so the silhouettes of the
        // distant mountains read as cut-outs against flame.
        renderer.sky = rift_engine::SkyConfig::abyss_hub();
        // Dim crimson key light + low warm ambient — the only
        // source of light is the distant horizon storm.
        renderer.key_light = rift_engine::KeyLight::STORMLIT;

        // Hub floor: a single dark obsidian platform disc.
        // Slightly oversized vs. the playable square so the
        // edge clearly extends past the walkable area before
        // falling into void.
        let hub_centre = Vec3::new(
            (floor.width / 2) as f32,
            -0.01,
            (floor.depth / 2) as f32,
        );
        const PLATFORM_RADIUS: f32 = 42.0;
        let platform = Mesh::ground_disc(
            hub_centre,
            PLATFORM_RADIUS,
            96,
            // White vertex color so the demon-ground texture
            // shows through unmodulated. UVs are scaled so one
            // tile of the procedural crimson-stone texture
            // spans ~14 m of world space — large enough that
            // the eye doesn't latch on to the repeat across
            // the platform.
            Vec3::splat(1.0),
            1.0 / 14.0,
        );
        renderer.add_mesh(&platform, Mat4::IDENTITY)?;
        let platform_obj_idx = renderer.objects.len() - 1;
        // Bind the procedural crimson cracked-stone tile.
        // Cached on `EnvTextures` so re-entering the hub
        // doesn't re-run the generator.
        self.env.ensure_crimson_stone(renderer);
        if let Some(set) = self.env.crimson_stone_set {
            renderer.set_object_shared_material(platform_obj_idx, set);
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

        // Platform underside: a downward squashed ellipsoid
        // hanging below the disc. The ellipsoid is centered at
        // y = -(scale.y) so its TOP vertex sits at y=0 (right
        // at the platform disc) and the entire shape extends
        // *downward* into the abyss — no dome rising above the
        // platform. The lower half drowns in fog.
        const UNDERSIDE_DEPTH: f32 = 32.0;
        let mut underside = Mesh::empty();
        underside.append_ellipsoid(
            // Slightly wider than the top disc so the silhouette
            // bulges before tapering. Tall vertical scale gives
            // a dramatic root.
            Vec3::new(PLATFORM_RADIUS * 1.05, UNDERSIDE_DEPTH, PLATFORM_RADIUS * 1.05),
            // Center sits a hair below the disc surface so the
            // very top of the ellipsoid (y = center + scale.y)
            // tucks just under the platform instead of poking
            // through it.
            hub_centre + Vec3::new(0.0, -UNDERSIDE_DEPTH - 0.02, 0.0),
            Vec3::new(0.025, 0.018, 0.025),
            32,
            18,
        );
        renderer.add_mesh(&underside, Mat4::IDENTITY)?;

        // Distant procedural mountain ring. Bases are sunk
        // ~40 m below the platform so the player→base distance
        // is significantly larger than the player→peak
        // distance, which is what lets distance fog
        // differentiate the two. Result: ridge crests cut
        // crisply against the crimson sky band, bases dissolve
        // into the dark crimson fog wall. Seed is fixed so the
        // skyline is stable across hub returns.
        let mountains = Mesh::mountain_ring(
            hub_centre,
            /*radius*/ 46.0,
            /*base_y*/ -40.0,
            /*min_height*/ 6.0,
            /*max_height*/ 16.0,
            /*segments*/ 96,
            /*seed*/ 0xA855_E575_FACE_BEEF,
            // Color matches the fog/sky-ground band so as the
            // mountain dissolves into haze it does so without
            // a perceptible color shift. Slightly darker than
            // the fog itself so a faint silhouette survives
            // even after fog saturates.
            Vec3::new(0.04, 0.018, 0.025),
        );
        renderer.add_mesh(&mountains, Mat4::IDENTITY)?;

        // Lightning strike anchors. The cloud cover itself is
        // now drawn procedurally by the sky shader (see
        // `SkyConfig::cloud_strength`), so we no longer need
        // any cloud meshes — but the storm driver still wants
        // a handful of world-space points to rim-light the
        // platform from when a strike fires. Spread them in a
        // ring high overhead so the per-strike point light
        // hits the platform from a believable cloud direction.
        const CLOUD_COUNT: usize = 7;
        const CLOUD_RADIUS: f32 = 38.0;
        const CLOUD_HEIGHT: f32 = 22.0;
        let mut cloud_anchors: Vec<Vec3> = Vec::with_capacity(CLOUD_COUNT);
        let mut cloud_rng = super::props::placement::SmallRng::new(0xC10D_5EED_BAAD_F00D);
        for i in 0..CLOUD_COUNT {
            let a = (i as f32 / CLOUD_COUNT as f32) * std::f32::consts::TAU
                + cloud_rng.frange(-0.18, 0.18);
            let r = CLOUD_RADIUS + cloud_rng.frange(-4.0, 4.0);
            let h = CLOUD_HEIGHT + cloud_rng.frange(-4.0, 6.0);
            cloud_anchors.push(hub_centre + Vec3::new(a.cos() * r, h, a.sin() * r));
        }

        // Defensive: drop any previous hub storm before we
        // replace it (e.g. hub-to-hub regenerate during a
        // debug teleport).
        self.hub_storm = None;

        // Hand cloud anchors to the storm driver. Capture
        // calm-state lighting so each frame's flash is purely
        // additive on top of the restored base.
        self.hub_storm = Some(HubStorm {
            base_key_color: renderer.key_light.color,
            base_key_ambient: renderer.key_light.ambient,
            base_fog_color: renderer.fog_color,
            cloud_anchors,
            state: StormState::Idle { cooldown: 1.5 },
            rng: super::props::placement::SmallRng::new(0x57AC_C170_BEAD_5EED),
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

    let mut bind_mesh = Mesh::empty();
    bind_mesh.vertices = asset.mesh.bind_vertices.clone();
    bind_mesh.indices = asset.mesh.indices.clone();
    let scaled = Mat4::from_scale_rotation_translation(
        Vec3::splat(role.scale()),
        glam::Quat::IDENTITY,
        position,
    );
    let obj_index = renderer.add_dynamic_mesh(&bind_mesh, scaled)?;
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
