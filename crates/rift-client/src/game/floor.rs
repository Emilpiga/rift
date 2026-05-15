use glam::{Mat4, Vec3};
use rift_dungeon::{FloorMood, NavGrid, RoomTheme};
use rift_engine::animation_profile::AnimClipKey;
use rift_engine::ash::vk;
use rift_engine::ecs::components::{
    Collider, Duration, Enemy, EnemyAnim, EnemyKind, FloatingVisual, Health, LocalPlayer,
    NetControlled, RemoteEnemy, RemoteMinion, Renderable, Skinned, Static, Transform, Velocity,
};
use rift_engine::{Floor, FloorConfig, Mesh, Renderer};

use super::environment::EnvTextures;
use super::monster_assets::MonsterCache;
use super::props::Props;
use super::rift_state::RiftState;
use super::torches::TorchSystem;
use crate::game::PlayerState;
use rift_game::monsters::MonsterRole;

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
    rng: rift_dungeon::props_placement::SmallRng,
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
    fn schedule_next(rng: &mut rift_dungeon::props_placement::SmallRng) -> StormState {
        // 4–14 seconds between strikes — enough silence that
        // each fork lands as an event rather than wallpaper.
        StormState::Idle {
            cooldown: rng.frange(4.0, 14.0),
        }
    }

    fn begin_flash(
        rng: &mut rift_dungeon::props_placement::SmallRng,
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
                    Self::begin_flash(
                        &mut self.rng,
                        &self.cloud_anchors,
                        pulses.saturating_sub(1),
                        None,
                        None,
                    )
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
                    Self::begin_flash(
                        &mut self.rng,
                        &self.cloud_anchors,
                        pulses_left,
                        Some(pos),
                        Some(color),
                    )
                } else {
                    StormState::Gap {
                        remaining: r,
                        pos,
                        color,
                        pulses_left,
                    }
                }
            }
        };
        self.state = next;

        // Apply the visual contribution of an active flash.
        if let StormState::Flash {
            intensity,
            pos,
            color,
            ..
        } = self.state
        {
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
    /// Persistent minimap exploration mask for the active hub or
    /// rift floor. Row-major, same dimensions as `nav_grid` and
    /// `dungeon.tiles`. Updated from local/party positions by the
    /// HUD pass so explored rooms remain visible but dimmed.
    pub minimap_seen: Vec<bool>,
    /// Live dungeon tile grid for the current floor. Owned
    /// here so client-side systems that need per-tile
    /// elevation (terrain-pitch animation, future foot IK)
    /// don't have to reach into the network layer's
    /// `predict_floor`. Regenerated on every floor change.
    /// `None` only at the moment between `FloorManager::new`
    /// and the first `generate` / `generate_hub`.
    pub dungeon: Option<rift_dungeon::Floor>,
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
    /// Hub-only sandstorm haze emitter. Spawned at hub
    /// generation, anchored on the player every frame so the
    /// dust field travels with the camera. `None` outside the
    /// hub. Stored as an opaque `EffectId` so the renderer
    /// owns the actual particle state.
    pub hub_haze: Option<rift_engine::renderer::vfx::EffectId>,
    /// Hub-only looping wind audio emitter. Lives in lockstep
    /// with `hub_haze`: spawned at hub generation, despawned
    /// on rift entry / hub teardown, anchored on the player
    /// every frame. Its volume is driven by the same gust
    /// envelope that modulates the haze brightness so the
    /// soundscape and the visual sandstorm pulse together.
    /// `None` when the audio system is unavailable or the
    /// `ambient/wind.mp3` asset failed to load.
    pub hub_wind: Option<rift_audio::EmitterId>,
    /// `true` once the character-select sandstorm backdrop
    /// (sand disc + dune ring + atmosphere + drifting haze)
    /// has been installed into the renderer for this entry
    /// into the screen. Reset to `false` by `generate_hub`
    /// and `generate` (rift floors) since they call
    /// `renderer.clear_objects()` and rebuild the world
    /// from scratch, so a future re-entry into char-select
    /// (after a disconnect) regenerates the backdrop.
    pub char_select_backdrop_built: bool,
    /// Rift-floor void embers. Spawned at the end of every
    /// rift-floor [`Self::generate`], anchored ~10 m below
    /// the player every frame so the field of glowing motes
    /// rises through the abyss around the playable area.
    /// `None` on the hub and on char-select.
    pub void_embers: Option<rift_engine::renderer::vfx::EffectId>,
}

impl FloorManager {
    pub fn new() -> Self {
        let floor = Floor::generate(FloorConfig::for_floor(1), 42);
        Self {
            boss_room_center: Vec3::ZERO,
            portal_anchors: None,
            nav_grid: NavGrid::from_floor(&floor),
            minimap_seen: vec![false; floor.width * floor.depth],
            dungeon: None,
            monsters: MonsterCache::default(),
            props: Props::new(),
            env: EnvTextures::default(),
            torches: TorchSystem::new(),
            stash_chest_pos: None,
            spawn_pos: Vec3::ZERO,
            hub_storm: None,
            hub_haze: None,
            hub_wind: None,
            char_select_backdrop_built: false,
            void_embers: None,
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
        // Drop the hub's drifting sand haze. Its `EffectId`
        // belonged to the previous hub instance and the
        // renderer-side particle system is wiped during
        // floor regen anyway; clearing the handle prevents
        // a stale `set_anchor` call from stomping a fresh
        // emitter that happens to land on the same slot.
        self.hub_haze = None;
        // Char-select shared the same disc + haze rig; the
        // upcoming `clear_objects()` will wipe it, so reset
        // the flag so a future re-entry into char-select
        // re-installs the backdrop on its first tick.
        self.char_select_backdrop_built = false;
        // The wind emitter id is dropped here too; the
        // audio crate's own teardown is owned by the caller
        // (see `transition.rs` — it must run before this
        // `generate` so the emitter is freed cleanly. We
        // just clear the local handle so we don't try to
        // poke a recycled slot.
        self.hub_wind = None;
        // Despawn the previous floor's torch VFX before we
        // regenerate. Their `EffectId`s belong to the old
        // particle system slots; leaving them around would
        // leak emitter capacity.
        self.torches.clear(renderer);
        // Same story for the previous rift floor's void
        // embers. Stops spawning; the in-flight particles
        // finish their natural lifetimes (4–8 s) and the
        // slot is reused once the pool drains. Cheaper than
        // a hard `clear_all` and keeps existing wisps from
        // popping out of frame mid-transition.
        if let Some(id) = self.void_embers.take() {
            renderer.vfx_system.despawn(id);
        }

        let config = FloorConfig::for_floor(rift.floor);
        let seed = seed_override
            .map(|s| s + rift.floor as u64 * 7)
            .unwrap_or(42 + rift.floor as u64 * 7);
        let floor = Floor::generate(config, seed);
        let mood = floor.mood;

        self.boss_room_center = floor.boss_room_center;
        self.portal_anchors = floor.portal_anchors;
        self.nav_grid = NavGrid::from_floor(&floor);
        self.minimap_seen = vec![false; floor.width * floor.depth];

        let atmosphere = mood_atmosphere(mood);
        renderer.clear_color = atmosphere.clear_color;
        renderer.ssao_strength = 0.7;
        renderer.fog_color = atmosphere.fog_color;
        renderer.fog_start = atmosphere.fog_start;
        renderer.fog_end = atmosphere.fog_end;

        // Rift gloom dome tinted by the floor mood. The fog wall
        // and the abyss-ocean shader now share the same chromatic
        // signature, so a crypt floor reads cold-blue below the
        // platforms, an archive reads violet, an infernal floor
        // stays blood-red, etc.
        renderer.sky = rift_sky_for_atmosphere(&atmosphere);
        // Cave-dark key + low ambient so torches drive the look.
        renderer.key_light = rift_engine::KeyLight::DUNGEON;

        // -----------------------------------------------------------
        // Per-room texture-pack split.
        //
        // Every floor tile gets bucketed into one of three
        // material packs based on the [`RoomTheme`] of its
        // owning room. Corridor tiles (which sit outside
        // every BSP rectangle) inherit the theme of the
        // *nearest reachable room* via a multi-source BFS
        // seeded from every in-room walkable tile. The
        // resulting Voronoi-by-walking-distance partition
        // means each corridor segment adopts whichever
        // themed room is closer along its actual path —
        // avoiding the previous "stone corridor between two
        // shrines" jarring transition. A 50:50 corridor
        // (equidistant from both rooms) gets split across
        // the two packs at its midpoint, which reads as the
        // material gradually changing along the corridor as
        // the player walks through.
        //
        // Wall tiles (no neighbouring walkable lookup) reuse
        // the same Voronoi map: they take the theme of the
        // nearest walkable tile they touch, so a wall on
        // the room side adopts the room's theme and a wall
        // on the corridor side adopts whichever room
        // claimed that corridor stretch.
        //
        // Stair/skirt geometry (raised daises, sunken pits)
        // remains on the default Stone pack — these are
        // small bridging surfaces, mixing them per-room
        // would cost an extra mesh build for ~tens of
        // triangles' worth of pixels.
        #[derive(Copy, Clone, PartialEq, Eq, Hash)]
        enum MatPack {
            Stone,
            Temple,
            Wood,
        }
        const PACK_COUNT: usize = 3;
        let pack_of = |theme: RoomTheme| match theme {
            RoomTheme::Shrine => MatPack::Temple,
            RoomTheme::Barracks | RoomTheme::Library | RoomTheme::Storage => MatPack::Wood,
            _ => MatPack::Stone,
        };
        let pack_idx = |p: MatPack| match p {
            MatPack::Stone => 0,
            MatPack::Temple => 1,
            MatPack::Wood => 2,
        };

        // Multi-source BFS over walkable tiles. Each room's
        // interior walkable tiles seed the queue at distance
        // 0 carrying that room's id; the BFS propagates
        // outward through every walkable neighbour, and the
        // first room to reach a corridor tile claims it.
        // Walls are not traversed (they're not walkable);
        // they inherit a theme from the nearest walkable
        // neighbour as a separate per-wall lookup below.
        let n_tiles = floor.width * floor.depth;
        let mut owner: Vec<Option<usize>> = vec![None; n_tiles];
        let mut bfs: std::collections::VecDeque<(usize, usize, usize)> =
            std::collections::VecDeque::new();
        for (rid, room) in floor.rooms.iter().enumerate() {
            for z in room.z..(room.z + room.depth) {
                for x in room.x..(room.x + room.width) {
                    if x >= floor.width || z >= floor.depth {
                        continue;
                    }
                    let i = z * floor.width + x;
                    if !floor.tiles[i].is_walkable() {
                        continue;
                    }
                    if owner[i].is_some() {
                        continue;
                    }
                    owner[i] = Some(rid);
                    bfs.push_back((x, z, rid));
                }
            }
        }
        while let Some((x, z, rid)) = bfs.pop_front() {
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if nx < 0 || nz < 0 {
                    continue;
                }
                let (nx, nz) = (nx as usize, nz as usize);
                if nx >= floor.width || nz >= floor.depth {
                    continue;
                }
                let ni = nz * floor.width + nx;
                if !floor.tiles[ni].is_walkable() {
                    continue;
                }
                if owner[ni].is_some() {
                    continue;
                }
                owner[ni] = Some(rid);
                bfs.push_back((nx, nz, rid));
            }
        }
        // Theme lookup: returns the BFS-assigned owner room's
        // theme, or `Generic` for a walkable tile that no
        // room could reach (shouldn't happen on connected
        // floors, but keep the fallback for robustness).
        let tile_pack = |x: usize, z: usize| -> MatPack {
            if x >= floor.width || z >= floor.depth {
                return MatPack::Stone;
            }
            let i = z * floor.width + x;
            match owner[i] {
                Some(rid) => pack_of(floor.rooms[rid].theme),
                None => MatPack::Stone,
            }
        };

        // Bucket floor positions by pack.
        let mut floor_pos_by_pack: [Vec<Vec3>; PACK_COUNT] = Default::default();
        for pos in floor.floor_positions() {
            let ix = pos.x as usize;
            let iz = pos.z as usize;
            floor_pos_by_pack[pack_idx(tile_pack(ix, iz))].push(pos);
        }
        // Extend each room/corridor's floor underneath its
        // bordering walls. With the see-through-wall x-ray
        // porthole now dither-cutting walls between camera
        // and player, the player can briefly see what sits
        // below those walls — without floor coverage that's
        // the empty fog/skybox void, which reads as the walls
        // "floating in mid-air".
        //
        // Each under-wall floor tile is themed by the same
        // pack the wall itself uses (the pack of the nearest
        // walkable neighbour), so the corridor stone floor
        // extends under the corridor's walls and the shrine's
        // blue-gold floor extends under the shrine's walls —
        // no theme seam halfway down a hallway.
        //
        // Y is forced to 0 to match the wall base regardless
        // of any neighbouring elevation feature; raised /
        // sunken sub-regions of a room never touch a wall
        // tile (verified in `dungeon::wall_positions`), so we
        // never hide a stair or a pit floor by doing this.
        //
        // Restricted to walls listed by `wall_positions()` so
        // we don't cover the entire void of perimeter walls
        // beyond the dungeon — only walls actually adjacent
        // to a walkable tile (i.e. visible from gameplay) get
        // a floor underneath them.
        let under_wall_pack = |x: usize, z: usize| -> MatPack {
            for (dx, dz) in [
                (-1i32, 0i32),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ] {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if nx < 0 || nz < 0 {
                    continue;
                }
                let (nx, nz) = (nx as usize, nz as usize);
                if nx >= floor.width || nz >= floor.depth {
                    continue;
                }
                let ni = nz * floor.width + nx;
                if floor.tiles[ni].is_walkable() {
                    return tile_pack(nx, nz);
                }
            }
            MatPack::Stone
        };
        for pos in floor.wall_positions() {
            let x = pos.x as usize;
            let z = pos.z as usize;
            let pack = under_wall_pack(x, z);
            floor_pos_by_pack[pack_idx(pack)].push(Vec3::new(x as f32, 0.0, z as f32));
        }

        // Floor meshes — one batched draw per non-empty pack.
        let mut floor_obj_indices: [Option<usize>; PACK_COUNT] = [None; PACK_COUNT];
        for (i, positions) in floor_pos_by_pack.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let mesh = Mesh::dungeon_floor(positions, rift.floor);
            renderer.add_mesh(&mesh, Mat4::IDENTITY)?;
            floor_obj_indices[i] = Some(renderer.objects.len() - 1);
        }
        // Used downstream for shadow-cast suppression /
        // collision iteration. The first non-empty floor
        // pack acts as the "primary" floor object index for
        // legacy code that references a single handle.
        let floor_obj_idx = floor_obj_indices
            .iter()
            .find_map(|o| *o)
            .expect("dungeon must have at least one floor tile");

        // Stair / ramp mesh — slanted quads bridging tiles at
        // adjacent elevations. Built per material pack so
        // each ramp inherits the floor texture of its
        // owning room (a wood-floor barracks dais climbs on
        // wood planks; a shrine dais climbs on blue-gold
        // tiles). The ramp tile sits at the *low* end of
        // the elevation step, so we bucket by the high end's
        // pack — same convention as the skirts — to keep
        // the ramp's surface consistent with the platform
        // it climbs onto.
        let stair_positions = floor.stair_positions();
        let mut stair_pos_by_pack: [Vec<(Vec3, rift_dungeon::StairDir)>; PACK_COUNT] =
            Default::default();
        for (pos, dir) in &stair_positions {
            // The high end of a ramp sits one tile in the
            // direction `dir` from the ramp tile. Sample
            // the pack of that destination tile so the
            // ramp's surface matches the platform it leads
            // onto.
            let (hx, hz) = match dir {
                rift_dungeon::StairDir::PosX => (pos.x + 1.0, pos.z),
                rift_dungeon::StairDir::NegX => (pos.x - 1.0, pos.z),
                rift_dungeon::StairDir::PosZ => (pos.x, pos.z + 1.0),
                rift_dungeon::StairDir::NegZ => (pos.x, pos.z - 1.0),
            };
            let hxi = hx.round().max(0.0) as usize;
            let hzi = hz.round().max(0.0) as usize;
            let pack = tile_pack(hxi, hzi);
            stair_pos_by_pack[pack_idx(pack)].push((*pos, *dir));
        }
        let mut stair_obj_indices: [Option<usize>; PACK_COUNT] = [None; PACK_COUNT];
        for (i, group) in stair_pos_by_pack.iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let stair_mesh = Mesh::dungeon_stairs(group, rift.floor, rift_dungeon::ELEVATION_STEP);
            renderer.add_mesh(&stair_mesh, Mat4::IDENTITY)?;
            stair_obj_indices[i] = Some(renderer.objects.len() - 1);
        }

        // Vertical "skirt" geometry between adjacent flat
        // floor tiles at different elevations (e.g. the lip
        // of a sunken pit, the outer rim of a raised dais).
        // Without these, the player can see straight through
        // the world at every elevation discontinuity. Empty
        // when the floor is uniformly flat.
        //
        // Built per material pack and filtered to seams
        // whose two endpoints share that pack so each strip
        // carries the same authored material as the floor it
        // bridges. Cross-pack seams (where the pit lip
        // straddles a Voronoi boundary between, say, a
        // shrine and a wood-floor barracks) get assigned to
        // whichever pack the *higher* tile belongs to — a
        // raised dais's vertical face reads as the dais's
        // material more naturally than the floor below it.
        let mut skirt_obj_indices: [Option<usize>; PACK_COUNT] = [None; PACK_COUNT];
        for pack_i in 0..PACK_COUNT {
            let skirt_mesh =
                Mesh::dungeon_floor_skirts_filtered(&floor, rift.floor, |(ax, az), (bx, bz)| {
                    use rift_dungeon::ELEVATION_STEP;
                    let ya = floor.elevation[az * floor.width + ax] as f32 * ELEVATION_STEP;
                    let yb = floor.elevation[bz * floor.width + bx] as f32 * ELEVATION_STEP;
                    let (hx, hz) = if ya >= yb { (ax, az) } else { (bx, bz) };
                    pack_idx(tile_pack(hx, hz)) == pack_i
                });
            if skirt_mesh.indices.is_empty() {
                continue;
            }
            renderer.add_mesh(&skirt_mesh, Mat4::IDENTITY)?;
            skirt_obj_indices[pack_i] = Some(renderer.objects.len() - 1);
        }

        // Walls — per-pack batches.
        // A wall tile inherits the pack of its nearest
        // walkable neighbour (which itself was Voronoi'd to
        // a room above). 4-cardinal scan with the first hit
        // winning matches the corridor's local owner so the
        // wall reads as "belonging to the corridor stretch
        // it borders" — no more theme seam halfway down a
        // hallway.
        let wall_color = match rift.floor % 4 {
            0 => Vec3::new(0.18, 0.16, 0.14), // damp weathered stone
            1 => Vec3::new(0.13, 0.18, 0.11), // deep mossy green
            2 => Vec3::new(0.24, 0.10, 0.08), // dried-blood crimson
            _ => Vec3::new(0.11, 0.15, 0.21), // glacial blue-gray
        };
        let wall_mesh = Mesh::wall_colored(wall_color);
        let wall_positions = floor.wall_positions();

        let wall_pack_for = |pos: &Vec3| -> MatPack {
            let x = pos.x as i32;
            let z = pos.z as i32;
            for (dx, dz) in [
                (-1, 0),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ] {
                let nx = x + dx;
                let nz = z + dz;
                if nx < 0 || nz < 0 {
                    continue;
                }
                let (nx, nz) = (nx as usize, nz as usize);
                if nx >= floor.width || nz >= floor.depth {
                    continue;
                }
                let ni = nz * floor.width + nx;
                if floor.tiles[ni].is_walkable() {
                    return tile_pack(nx, nz);
                }
            }
            MatPack::Stone
        };

        let mut wall_pos_by_pack: [Vec<Vec3>; PACK_COUNT] = Default::default();
        for pos in &wall_positions {
            let pack = wall_pack_for(pos);
            wall_pos_by_pack[pack_idx(pack)].push(*pos);
        }

        let mut wall_obj_indices: [Option<usize>; PACK_COUNT] = [None; PACK_COUNT];
        for (i, positions) in wall_pos_by_pack.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let batched = Mesh::batch_at_positions(&wall_mesh, positions);
            renderer.add_mesh(&batched, Mat4::IDENTITY)?;
            wall_obj_indices[i] = Some(renderer.objects.len() - 1);
        }

        // Bind authored PBR materials to floor and walls. We
        // still call `ensure(...)` to keep the procedural sets
        // available as a fallback when an asset fails to load,
        // but prefer the authored brick / ground tile maps when
        // they're ready. The themed packs (white_bricks +
        // blue_gold floor for shrines, wood planks for
        // barracks/library/storage) are pre-warmed by the
        // background decode worker; they fall back to the
        // default stone pack if their decode hasn't landed by
        // the time the dungeon binds.
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
        // Parallax is kept small but non-zero on both floors
        // and walls. The floor is the main surface lit by
        // torches at grazing-to-side angles, where the
        // height-map self-shadow pass really sells the
        // ground-tile relief — without it, the torch's
        // shadow boundary cuts a knife edge across the
        // bumpy floor instead of feathering along its grout
        // lines and chips. Walls get a slightly stronger
        // scale because they're seen more head-on, where the
        // POM offset itself is the bigger perceptual win.
        let pbr_flags = f32::from_bits(1u32);
        // Wall flag bits: bit 0 = PBR shader path (same as
        // floor), bit 3 = "see-through occluder" — the cel
        // shader's `main()` reads this to dither-cut a porthole
        // around the camera→player segment so the player stays
        // visible even when a wall is between them and the
        // camera. Replaces the older "snap camera in front of
        // the nearest wall" hack in `camera_follow_system`,
        // which produced jolting zoom snaps every time the
        // player walked behind cover.
        let wall_flags = f32::from_bits(1u32 | (1u32 << 3));
        let floor_params = [1.0 / 3.0, 0.012, pbr_flags, 0.0];
        let wall_params = [1.0 / 3.0, 0.02, wall_flags, 0.0];

        // Resolve per-pack descriptor sets, falling back to
        // the default stone pack while themed packs are still
        // decoding in the background.
        let stone_floor = self.env.ground_tiles_set;
        let stone_wall = self.env.bricks_wall_set;
        let pack_floor = |i: usize| -> Option<vk::DescriptorSet> {
            match i {
                1 => self.env.blue_gold_floor_set.or(stone_floor),
                2 => self.env.wood_planks_set.or(stone_floor),
                _ => stone_floor,
            }
        };
        let pack_wall = |i: usize| -> Option<vk::DescriptorSet> {
            match i {
                1 => self.env.white_bricks_wall_set.or(stone_wall),
                _ => stone_wall,
            }
        };

        for (i, obj) in floor_obj_indices.iter().enumerate() {
            let Some(idx) = *obj else {
                continue;
            };
            if let Some(set) = pack_floor(i) {
                renderer.set_object_shared_material(idx, set);
                renderer.set_object_material_params(idx, floor_params);
            } else if let Some(set) = self.env.floor_set {
                renderer.set_object_shared_material(idx, set);
            }
        }
        // Stairs and skirts get bound per pack so each ramp
        // / elevation lip carries the floor texture of the
        // platform it climbs onto / drops from.
        for (i, obj) in stair_obj_indices.iter().enumerate() {
            let Some(idx) = *obj else {
                continue;
            };
            if let Some(set) = pack_floor(i) {
                renderer.set_object_shared_material(idx, set);
                renderer.set_object_material_params(idx, floor_params);
            } else if let Some(set) = self.env.floor_set {
                renderer.set_object_shared_material(idx, set);
            }
        }
        for (i, obj) in skirt_obj_indices.iter().enumerate() {
            let Some(idx) = *obj else {
                continue;
            };
            if let Some(set) = pack_floor(i) {
                renderer.set_object_shared_material(idx, set);
                renderer.set_object_material_params(idx, floor_params);
            } else if let Some(set) = self.env.floor_set {
                renderer.set_object_shared_material(idx, set);
            }
        }

        for (i, obj) in wall_obj_indices.iter().enumerate() {
            let Some(idx) = *obj else {
                continue;
            };
            if let Some(set) = pack_wall(i) {
                renderer.set_object_shared_material(idx, set);
                renderer.set_object_material_params(idx, wall_params);
            } else if let Some(set) = self.env.wall_set {
                renderer.set_object_shared_material(idx, set);
            }
        }
        // Floor is a giant flat slab \u2014 it would only contribute
        // self-shadow against the directional light, which is
        // already handled by the heightmap self-shadow + height-
        // map-displaced shadow lookup in the lit pass. Keeping
        // it out of the shadow draw list saves rasterising
        // ~thousands of triangles per dir-shadow + per cube
        // face of every shadow-casting torch in range.
        for obj in floor_obj_indices.iter().flatten() {
            renderer.set_object_casts_shadow(*obj, false);
        }
        let _ = floor_obj_idx; // kept for legacy single-handle reads
        for obj in stair_obj_indices.iter().flatten() {
            renderer.set_object_casts_shadow(*obj, false);
        }
        for obj in skirt_obj_indices.iter().flatten() {
            renderer.set_object_casts_shadow(*obj, false);
        }

        // Still need individual ECS entities for collision
        for pos in &wall_positions {
            world.spawn((
                Transform::from_position(*pos + Vec3::new(0.0, 2.5, 0.0)),
                Collider::new(0.5, 2.5, 0.5),
                Static,
            ));
        }

        // Render every prop the dungeon placed for this floor,
        // including light-flagged candlestick torches —
        // `Props::render_floor` doesn't care which is which,
        // they're all just `PlacedProp` entries with the same
        // wall-snap pipeline. Placement (including torches)
        // is owned by `rift_dungeon::props_placement` so the
        // server's authoritative `kinematic::integrate` collides
        // the player capsule against the same prop set the
        // client renders here.
        self.props.render_floor(renderer, &floor);

        // Now stand up the per-torch flame VFX, point light,
        // and (later) audio emitter for every prop the
        // dungeon flagged with `light = true`. The candle mesh
        // itself was already drawn by `render_floor` above.
        self.torches.build(&floor, renderer, seed);

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

        // Stash the live floor so render-side systems (terrain
        // pitch, future foot-IK) can sample per-tile elevation
        // without going through the network layer.
        self.dungeon = Some(floor);

        // Spawn the mood-tinted void particle field. Anchor is
        // set every frame in `render_phase` ~10 m below the
        // player so particles rise from the abyss past the
        // floor's outer edges. Initial anchor is the spawn point
        // at the same depth so a player who pauses on step zero
        // still sees a populated field.
        self.void_embers = Some(renderer.vfx_system.spawn(
            rift_engine::renderer::vfx::presets::rift_void_embers_tinted(atmosphere.fog_color),
            self.spawn_pos - Vec3::new(0.0, 10.0, 0.0),
        ));

        Ok(())
    }

    /// Install the sandstorm visual backdrop used by the
    /// character-select screen: the sand-PBR ground disc, the
    /// dune ring, the warm-tan atmosphere (clear / fog / sky /
    /// key-light), and the drifting sand-haze emitter.
    ///
    /// Idempotent — the disc + dunes are added once, then the
    /// method just retargets the haze brightness each frame
    /// using the same gust envelope the hub uses. Anchor is
    /// the avatar-podium position (camera offset baked in).
    ///
    /// Mirrors the visual section of [`Self::generate_hub`]
    /// without world reset, floor data, props, portals, or
    /// torches — char-select shows just the desert and the
    /// preview avatar.
    pub fn ensure_char_select_backdrop(&mut self, renderer: &mut Renderer) {
        // Anchor under the camera podium. `OFFSET_X = -0.95`
        // matches `character_select::update_preview_camera`.
        let anchor = Vec3::new(-0.95, 0.6, 0.0);
        let centre = Vec3::new(-0.95, 0.02, 0.0);

        // Atmosphere is cheap to set every frame. Character
        // select gets its own quieter variant of the hub
        // sandstorm so the preview model stays legible.
        renderer.clear_color = [0.22, 0.15, 0.10, 1.0];
        renderer.fog_color = [0.58, 0.40, 0.24];
        renderer.fog_start = 14.0;
        renderer.fog_end = 80.0;
        renderer.ssao_strength = 0.08;
        renderer.camera.far = 260.0;
        renderer.sky = rift_engine::SkyConfig::sandstorm_hub();
        renderer.sky.sun_dir = Vec3::new(0.24, 0.46, -0.86).normalize();
        renderer.sky.sun_strength = 0.32;
        renderer.sky.sun_size = 0.9992;
        renderer.sky.cloud_flash_color = Vec3::new(0.95, 0.76, 0.48);
        renderer.key_light = rift_engine::KeyLight {
            direction: Vec3::new(0.24, 0.62, -0.74),
            color: Vec3::new(1.65, 1.22, 0.76),
            ambient: 0.42,
        };
        renderer.point_lights.clear();
        renderer.point_lights.push(rift_engine::PointLight {
            position: Vec3::new(-1.30, 2.05, 2.65),
            color: Vec3::new(1.0, 0.72, 0.42),
            radius: 9.5,
            intensity: 2.4,
        });
        renderer.point_lights.push(rift_engine::PointLight {
            position: Vec3::new(1.25, 2.80, -2.25),
            color: Vec3::new(0.95, 0.62, 0.34),
            radius: 12.0,
            intensity: 1.15,
        });

        if !self.char_select_backdrop_built {
            // ── Ground disc (sand PBR) ──────────────────────
            const PLATFORM_RADIUS: f32 = 64.0;
            let platform =
                Mesh::ground_disc(centre, PLATFORM_RADIUS, 96, Vec3::splat(1.0), 1.0 / 2.0);
            if renderer.add_mesh(&platform, Mat4::IDENTITY).is_ok() {
                let platform_obj_idx = renderer.objects.len() - 1;
                self.env.ensure_desert_rocks(renderer);
                if let Some(set) = self.env.desert_rocks_set {
                    renderer.set_object_shared_material(platform_obj_idx, set);
                    let pbr_flags = f32::from_bits(1u32);
                    renderer
                        .set_object_material_params(platform_obj_idx, [1.0, 0.012, pbr_flags, 0.0]);
                }
                renderer.set_object_casts_shadow(platform_obj_idx, false);
            }

            // ── Dune ring ───────────────────────────────────
            let dune_params = rift_math::terrain::MountainRingParams {
                inner_radius: PLATFORM_RADIUS - 2.0,
                outer_radius: 180.0,
                base_y: centre.y,
                peak_height: 3.5,
                angular_segments: 192,
                radial_segments: 28,
                noise_frequency: 0.025,
                ridged_blend: 0.0,
                seed: 0xD0_5E_5A_4D_5A_4D_5A_4D,
            };
            let dunes =
                Mesh::mountain_terrain(&dune_params, centre, Vec3::new(1.0, 0.92, 0.78), 6.0);
            if renderer.add_mesh(&dunes, Mat4::IDENTITY).is_ok() {
                let dunes_obj_idx = renderer.objects.len() - 1;
                if let Some(set) = self.env.desert_rocks_set {
                    renderer.set_object_shared_material(dunes_obj_idx, set);
                    let pbr_flags = f32::from_bits(1u32);
                    renderer
                        .set_object_material_params(dunes_obj_idx, [1.0, 0.008, pbr_flags, 0.0]);
                }
                renderer.set_object_casts_shadow(dunes_obj_idx, false);
            }

            // ── Haze emitter ────────────────────────────────
            self.hub_haze = Some(renderer.vfx_system.spawn(
                rift_engine::renderer::vfx::presets::sandstorm_haze(),
                anchor,
            ));

            self.char_select_backdrop_built = true;
        }

        // Drive the gust envelope on the haze each frame so
        // the dust pulses (same curve `render_phase` runs on
        // the real hub).
        if let Some(h) = self.hub_haze {
            let t = renderer.elapsed_secs();
            let slow = (t * 0.35).sin() * 0.55
                + (t * 0.17 + 1.7).sin() * 0.35
                + (t * 0.08 + 0.4).sin() * 0.10;
            let gust = (1.0 + slow * 0.45).max(0.05);
            renderer.vfx_system.set_anchor(h, anchor);
            renderer.vfx_system.set_brightness(h, gust * 0.10);
        }
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
        // Drop any pre-existing haze emitter (e.g. spawned by
        // the char-select backdrop on the previous screen) so
        // the fresh hub spawn below doesn't leak the prior
        // handle and so the gust envelope re-anchors on the
        // player instead of the screen-select avatar.
        if let Some(prev) = self.hub_haze.take() {
            renderer.vfx_system.despawn(prev);
        }
        // Char-select disc + dune ring were just wiped by
        // `clear_objects()`; reset the flag so a future
        // re-entry into char-select (after a disconnect)
        // re-installs them on its first tick.
        self.char_select_backdrop_built = false;
        // Despawn any previous floor's torch VFX + drop their
        // cached `PointLight` entries. Without this, the
        // dungeon's wall-torch positions would keep being
        // pushed into `point_lights` every frame by
        // `TorchSystem::update_lights`, scattering stale lights
        // across the hub at random dungeon coordinates.
        self.torches.clear(renderer);
        // Same idea for rift-floor void embers — the hub has
        // its own backdrop and doesn't want crimson motes
        // rising past its platform.
        if let Some(id) = self.void_embers.take() {
            renderer.vfx_system.despawn(id);
        }
        // Wipe the per-frame light vecs so no leftover entries
        // from the previous floor (torch sconces, vfx trails)
        // can paint a single stray frame in the hub before the
        // per-frame systems repopulate. `update_lights` and the
        // vfx pass clear these every frame, but doing it here
        // closes the one-frame window during the regen.
        renderer.point_lights.clear();
        renderer.vfx_lights.clear();

        let floor = Floor::hub();
        self.boss_room_center = Vec3::ZERO;
        self.portal_anchors = None;
        self.nav_grid = NavGrid::from_floor(&floor);
        self.minimap_seen = vec![false; floor.width * floor.depth];

        // Brooding "floating obsidian platform in a sandstorm"
        // ambience. The platform is dark stone, the sky is a
        // tan dust dome, and the fog is a warm dust haze
        // tight enough to limit visibility to ~25 m so the
        // play area reads as enclosed by airborne sand
        // rather than open desert.
        renderer.clear_color = [0.30, 0.20, 0.12, 1.0];
        renderer.ssao_strength = 0.7;
        // Fog colour: matches the sky's horizon band so the
        // foggy platform edge fades smoothly into the dust
        // horizon instead of showing a darker rust-coloured
        // ring against a lighter sky.
        renderer.fog_color = [0.78, 0.55, 0.30];
        // Loose-but-still-veiling fog. We want the player to
        // sense a vast desert beyond the immediate clear
        // zone — dune silhouettes barely visible through the
        // haze — without ever being able to see a hard edge
        // of the world. The visible platform extends well
        // past `fog_end`, so distant dunes silhouette into
        // the dust horizon and the platform "feels infinite".
        renderer.fog_start = 8.0;
        renderer.fog_end = 55.0;
        // Camera far plane needs to clear the dune ring
        // (~180 m max radius) so distant silhouettes don't
        // pop out at oblique camera angles.
        renderer.camera.far = 260.0;

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
        let hub_centre = Vec3::new((floor.width / 2) as f32, 0.02, (floor.depth / 2) as f32);
        const PLATFORM_RADIUS: f32 = 64.0;
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
            // intended 2 m / tile coverage). Small parallax
            // scale enables both POM (visible bump-depth
            // when the player walks across the disc) and
            // height-map self-shadowing along the cast
            // shadow boundary — the shadow line bends
            // along the sand's ripples instead of cutting
            // straight across them. 0.015 is small enough
            // that the parallax march cost stays negligible
            // at our top-down view angle.
            let pbr_flags = f32::from_bits(1u32);
            renderer.set_object_material_params(platform_obj_idx, [1.0, 0.015, pbr_flags, 0.0]);
        } else {
            self.env.ensure_crimson_stone(renderer);
            if let Some(set) = self.env.crimson_stone_set {
                renderer.set_object_shared_material(platform_obj_idx, set);
            }
        }
        // The platform disc is a giant flat receiver — it
        // doesn't cast meaningful shadows on anything (the
        // shadow it would cast goes off the visible scene
        // into the abyss). Excluding it from the shadow
        // passes saves ~10k triangles per dir-shadow + per
        // cube face of the portal point light. The lit-pass
        // heightmap self-shadow + heightmap-displaced shadow
        // lookup already handle the visible micro-shadows.
        renderer.set_object_casts_shadow(platform_obj_idx, false);
        // No glowing rim: the platform now extends well past
        // the fog wall (`fog_end = 55 m`, platform radius =
        // 64 m), so the disc's edge is never visible to the
        // player. A bright crimson ring at the rim would only
        // silhouette through the haze as a hard "edge of the
        // world" line, breaking the desired "infinite desert"
        // feel. Distant dune silhouettes (added below) pick
        // up the framing job instead.

        // Sand-dune ring beyond the platform. The dunes start
        // a couple of metres inside the platform's outer edge
        // (so the seam is hidden under the disc's PBR sand
        // material) and march out to ~180 m where camera-far
        // and fog completely swallow them. Inside the play
        // arena the disc is dead flat — the dunes only ramp
        // up well past the playable zone, then taper back to
        // the disc's Y at the outer fog wall so the ring
        // doesn't read as a solid mountain horizon.
        //
        // Peak heights are intentionally low (~3 m) so this
        // reads as desert dunes rather than the dungeon
        // crate's mountain ring; broader noise frequency
        // (low value) groups peaks into rolling crests
        // instead of jagged spires.
        let dune_params = rift_math::terrain::MountainRingParams {
            inner_radius: PLATFORM_RADIUS - 2.0,
            outer_radius: 180.0,
            // Sit the dune base flush with the platform top
            // so the inner edge meets the disc cleanly; the
            // radial taper inside `mountain_terrain` already
            // pinches heights to zero at the inner edge so
            // there's no visible step.
            base_y: hub_centre.y,
            peak_height: 3.5,
            angular_segments: 192,
            radial_segments: 28,
            // Broad rolling dunes, not narrow spikes.
            noise_frequency: 0.025,
            // Pure fBm — dunes are smooth, not ridged.
            ridged_blend: 0.0,
            seed: 0xD0_5E_5A_4D_5A_4D_5A_4D,
        };
        let dunes = Mesh::mountain_terrain(
            &dune_params,
            hub_centre,
            // Warm sand tint, modulated by the bound PBR
            // basecolor texture below. Slightly desaturated
            // so the desert_rocks pack drives the actual hue.
            Vec3::new(1.0, 0.92, 0.78),
            // One tile of the 2 k sand pack per ~6 m of
            // dune surface — fine enough to avoid obvious
            // repeats at the camera distance, coarse enough
            // that the mip pyramid doesn't thrash on the
            // 192×28 vertex band.
            6.0,
        );
        renderer.add_mesh(&dunes, Mat4::IDENTITY)?;
        let dunes_obj_idx = renderer.objects.len() - 1;
        // Reuse the same sandy_cliff_rocks PBR pack the
        // platform binds — the dunes are an extension of the
        // same desert surface, just elevated.
        if let Some(set) = self.env.desert_rocks_set {
            renderer.set_object_shared_material(dunes_obj_idx, set);
            let pbr_flags = f32::from_bits(1u32);
            // `uv_world_scale` above already produced a
            // tiling UV; pass `1.0` for the per-object
            // multiplier so the bake-in scale is honoured.
            // Tiny parallax scale (0.01) drives height-map
            // self-shadowing so the dune face's grain detail
            // shadow-feathers under the strong directional
            // sun. View-angle is grazing on the far dunes
            // where the offset itself reads as artefacts, so
            // we keep the scale very small — just enough to
            // give the self-shadow march something to work
            // with.
            renderer.set_object_material_params(dunes_obj_idx, [1.0, 0.01, pbr_flags, 0.0]);
        }
        // Dunes ring spans 60–180 m at low elevation; their
        // cast shadow on each other is dwarfed by the
        // heightmap self-shadow already in the lit pass, and
        // they're outside every shadow-casting torch's
        // radius anyway. Excluding them from the shadow
        // draw list saves rasterising ~5k vertices across
        // the directional shadow pass every frame.
        renderer.set_object_casts_shadow(dunes_obj_idx, false);

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

        // Drifting sand haze: a wide-area particle layer
        // spawned once at hub-gen and anchored on the player
        // every frame (see `phases::render_phase`). Two
        // stacked sublayers (slow soft sheets + fast low
        // streaks) sell "you're standing inside a sandstorm"
        // without occluding the sun disc / god rays. Spawned
        // at the hub centre as a placeholder anchor; the
        // per-frame retarget will move it to the player on
        // the very next tick.
        self.hub_haze = Some(renderer.vfx_system.spawn(
            rift_engine::renderer::vfx::presets::sandstorm_haze(),
            hub_centre,
        ));

        // No baked hub point lights. The only light in the
        // hub is the rift portal's own crimson glow, which
        // `portal_system::push_lights` republishes every frame
        // (synced with the shader's breathing/spasm pulse).
        // `TorchSystem::update_lights` clears `point_lights`
        // at the top of every frame, so anything we'd push
        // here would be wiped on the next tick anyway.

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
            let a = (i as f32 / COLLIDER_RING_SEGMENTS as f32) * std::f32::consts::TAU;
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

        // Player stash chest. Placement (position + yaw +
        // collider) is authoritative on the dungeon side via
        // `Floor.hub`'s `props_placement::decorate_hub` call.
        // The chest's world position is needed by the stash
        // UI (range gating), so we pull it back from the
        // placed-prop list rather than recomputing.
        let stash_pos = floor
            .props
            .iter()
            .find(|p| p.id == rift_dungeon::props::PropId::StashChest)
            .map(|p| p.pos);
        // Render every prop the dungeon placed (stash chest +
        // ground scatter).
        self.props.render_floor(renderer, &floor);
        self.stash_chest_pos = stash_pos;

        let spawn = floor.spawn_pos;
        self.spawn_pos = spawn;
        self.spawn_player(world, renderer, spawn, player_state, anim_cache, cosmetics)?;

        let portal_pos = floor.first_room_center() + Vec3::new(0.0, 0.5, 0.0);
        log::info!("Hub generated. Portal at {:?}", portal_pos);
        // Stash the live floor for render-side elevation
        // sampling (terrain pitch, foot IK). The hub is flat
        // so this currently never produces a non-zero pitch,
        // but we still want a populated `dungeon` so consumers
        // don't have to special-case the hub.
        self.dungeon = Some(floor);
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

struct MoodAtmosphere {
    clear_color: [f32; 4],
    fog_color: [f32; 3],
    fog_start: f32,
    fog_end: f32,
}

fn mood_atmosphere(mood: FloorMood) -> MoodAtmosphere {
    match mood {
        FloorMood::Sanctuary => MoodAtmosphere {
            clear_color: [0.30, 0.20, 0.12, 1.0],
            fog_color: [0.78, 0.55, 0.30],
            fog_start: 8.0,
            fog_end: 55.0,
        },
        FloorMood::Crypt => MoodAtmosphere {
            clear_color: [0.004, 0.007, 0.010, 1.0],
            fog_color: [0.030, 0.060, 0.085],
            fog_start: 5.5,
            fog_end: 20.0,
        },
        FloorMood::Armory => MoodAtmosphere {
            clear_color: [0.010, 0.006, 0.004, 1.0],
            fog_color: [0.080, 0.036, 0.016],
            fog_start: 6.5,
            fog_end: 24.0,
        },
        FloorMood::Archive => MoodAtmosphere {
            clear_color: [0.006, 0.005, 0.010, 1.0],
            fog_color: [0.038, 0.032, 0.075],
            fog_start: 6.0,
            fog_end: 23.0,
        },
        FloorMood::Shrine => MoodAtmosphere {
            clear_color: [0.012, 0.009, 0.004, 1.0],
            fog_color: [0.095, 0.060, 0.022],
            fog_start: 7.0,
            fog_end: 26.0,
        },
        FloorMood::Prison => MoodAtmosphere {
            clear_color: [0.004, 0.006, 0.005, 1.0],
            fog_color: [0.030, 0.050, 0.036],
            fog_start: 5.0,
            fog_end: 19.0,
        },
        FloorMood::Infernal => MoodAtmosphere {
            clear_color: [0.014, 0.004, 0.003, 1.0],
            fog_color: [0.090, 0.012, 0.008],
            fog_start: 5.5,
            fog_end: 21.0,
        },
    }
}

fn rift_sky_for_atmosphere(atmosphere: &MoodAtmosphere) -> rift_engine::SkyConfig {
    let mut sky = rift_engine::SkyConfig::rift();
    let fog = Vec3::new(
        atmosphere.fog_color[0],
        atmosphere.fog_color[1],
        atmosphere.fog_color[2],
    );
    let peak = fog.max_element().max(0.001);
    let chroma = fog / peak;
    let mood_zenith = fog * 2.15 + chroma * 0.070;
    let mood_horizon = fog * 1.12 + chroma * 0.020;
    let mood_ground = fog * 0.72 + chroma * 0.008;

    sky.zenith = mix_rgb(
        sky.zenith,
        [mood_zenith.x, mood_zenith.y, mood_zenith.z],
        0.78,
    );
    sky.horizon = mix_rgb(
        sky.horizon,
        [mood_horizon.x, mood_horizon.y, mood_horizon.z],
        0.92,
    );
    sky.ground = mix_rgb(
        sky.ground,
        [mood_ground.x, mood_ground.y, mood_ground.z],
        0.88,
    );
    sky
}

fn mix_rgb(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
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
    net_id: rift_net::NetId,
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
        Vec3::splat(role.scale() * 0.34),
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
    // Ordinary animated enemies are numerous and constantly re-skinned.
    // Letting each one cast point-light cube shadows makes nearby torch
    // slots dirty whenever combat starts, which scales terribly once packs
    // get large. Bosses keep their silhouette shadow; minions are lit by
    // the room but do not force repeated 6-face shadow refreshes.
    renderer.set_object_casts_shadow(obj_index, matches!(role, MonsterRole::Boss));
    if matches!(role, MonsterRole::Wraith) {
        if let Some(obj) = renderer.objects.get_mut(obj_index) {
            obj.tint = [0.72, 1.20, 1.35, 0.48];
        }
    }
    let skinned = Skinned {
        mesh: asset.mesh.clone(),
        scratch: Vec::new(),
        joint_worlds: Vec::new(),
    };
    let initial_clip = asset
        .anim_bindings
        .get(AnimClipKey::Idle)
        .or_else(|| asset.anim_bindings.get(AnimClipKey::Walk))
        .or_else(|| asset.anims.clips.values().next().cloned());
    let animator = initial_clip.map(rift_engine::animation::Animator::new);

    let mut builder = hecs::EntityBuilder::new();
    builder.add(Transform::from_position(position));
    builder.add(Velocity::default());
    builder.add(Health::new(hp_max));
    builder.add(Renderable {
        object_index: obj_index,
    });
    builder.add(NetControlled);
    builder.add(RemoteEnemy { net_id: net_id.0 });
    // Tag as `Enemy` so the HUD pass picks it up for floating health
    // bars + boss arrow. Speed/progress_value are server-authoritative
    // so we leave them at safe defaults; only `kind` matters visually.
    builder.add(Enemy {
        speed: 0.0,
        progress_value: 0.0,
        kind: match role {
            MonsterRole::Brute | MonsterRole::Elite | MonsterRole::Boss => EnemyKind::Brute,
            MonsterRole::Stalker | MonsterRole::Wraith | MonsterRole::Riftling => {
                EnemyKind::Stalker
            }
            MonsterRole::Caster | MonsterRole::Mindbinder => EnemyKind::Caster,
        },
    });
    builder.add(skinned);
    builder.add(asset.anims.clone());
    builder.add(asset.anim_bindings.clone());
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

pub fn spawn_remote_minion_entity(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    monsters: &mut MonsterCache,
    net_id: rift_net::NetId,
    owner_net_id: rift_net::NetId,
    role: MonsterRole,
    position: Vec3,
    hp_max: f32,
) -> anyhow::Result<hecs::Entity> {
    let presentation = rift_game::minions::presentation_for_role(role);
    let hover_height = presentation.hover_height().unwrap_or(0.0);

    let shared_set = monsters
        .slot_mut(role)
        .as_mut()
        .and_then(|a| a.ensure_shared_material(renderer));
    let asset = monsters
        .get(role)
        .ok_or_else(|| anyhow::anyhow!("monster role {role:?} not loaded"))?;
    let scaled = Mat4::from_scale_rotation_translation(
        Vec3::splat(role.scale() * presentation.visual_scale),
        glam::Quat::IDENTITY,
        position + Vec3::Y * hover_height,
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
    renderer.set_object_casts_shadow(obj_index, false);
    if let Some(obj) = renderer.objects.get_mut(obj_index) {
        obj.tint = [0.62, 0.78, 1.45, 0.62];
    }
    let skinned = Skinned {
        mesh: asset.mesh.clone(),
        scratch: Vec::new(),
        joint_worlds: Vec::new(),
    };
    let initial_clip = asset
        .anim_bindings
        .get(AnimClipKey::Idle)
        .or_else(|| asset.anim_bindings.get(AnimClipKey::Walk))
        .or_else(|| asset.anims.clips.values().next().cloned());
    let animator = initial_clip.map(rift_engine::animation::Animator::new);

    let mut builder = hecs::EntityBuilder::new();
    let mut transform = Transform::from_position(position + Vec3::Y * hover_height);
    transform.scale = Vec3::splat(role.scale() * presentation.visual_scale);
    builder.add(transform);
    builder.add(Velocity::default());
    builder.add(Health::new(hp_max));
    builder.add(Duration::new(1.0));
    builder.add(Renderable {
        object_index: obj_index,
    });
    builder.add(NetControlled);
    builder.add(RemoteMinion {
        net_id: net_id.0,
        owner_net_id: owner_net_id.0,
    });
    if hover_height > 0.0 {
        builder.add(FloatingVisual { hover_height });
    }
    builder.add(skinned);
    builder.add(asset.anims.clone());
    builder.add(asset.anim_bindings.clone());
    if let Some(a) = animator {
        builder.add(a);
    }
    builder.add(EnemyAnim {
        last_hp: hp_max,
        attacking: false,
        lock_remaining: 0.0,
    });
    Ok(world.spawn(builder.build()))
}
