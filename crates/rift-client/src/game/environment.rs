//! Procedurally-generated tiling textures for the dungeon floor and walls.
//!
//! The asset pack we ship doesn't include ground/wall textures, so we
//! synthesize them at runtime: the floor is a moss-streaked cobblestone,
//! and the walls are a weathered ashlar (large rectangular masonry).
//! Both textures tile seamlessly at 1 unit = 1 wall stride, matching the
//! UV layout of `Mesh::dungeon_floor` and `Mesh::wall_colored`.

use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use rift_engine::ash::vk;
use rift_engine::ash::Device;
use rift_engine::gpu_allocator::vulkan::Allocator;
use rift_engine::renderer::asset_decode::{
    decode_linear, decode_mr_atlas, decode_srgb, DecodedPbrPack, DecodedTexture,
};
use rift_engine::renderer::texture::Texture;
use rift_engine::Renderer;

const FLOOR_SIZE: u32 = 256;
const WALL_SIZE: u32 = 256;

pub struct EnvTextures {
    pub floor_set: Option<vk::DescriptorSet>,
    pub wall_set: Option<vk::DescriptorSet>,
    /// Grass tile bound on the hub floor for the outdoor / nature
    /// theme. Optional — only uploaded when first requested via
    /// [`Self::ensure_grass`].
    pub grass_floor_set: Option<vk::DescriptorSet>,
    /// Demon-ground tile bound on the hub platform for the
    /// abyss / hellish theme. Loaded from
    /// `assets/textures/demon_ground_01.jpg` on first use via
    /// [`Self::ensure_demon_ground`].
    pub demon_ground_set: Option<vk::DescriptorSet>,
    /// Procedural crimson cracked-stone tile used for the
    /// abyss-hub platform surface. Generated on first use via
    /// [`Self::ensure_crimson_stone`].
    pub crimson_stone_set: Option<vk::DescriptorSet>,
    /// Authored PBR brick wall material used for dungeon walls.
    /// Loaded lazily by [`Self::ensure_bricks_wall`].
    pub bricks_wall_set: Option<vk::DescriptorSet>,
    /// Authored PBR ground tile material used for dungeon floors.
    /// Loaded lazily by [`Self::ensure_ground_tiles`].
    pub ground_tiles_set: Option<vk::DescriptorSet>,
    /// Authored desert-rocks **basecolor only** texture used
    /// for the hub-platform surface. Loaded lazily by
    /// [`Self::ensure_desert_rocks`]. We deliberately bind
    /// only the colour map here — the giant disc is viewed
    /// from above so PBR specular wanders distractingly across
    /// it as the camera shifts, and parallax/normal-mapping
    /// don't pay off at this scale either. The cel-shading
    /// path produces a calm, painterly look instead.
    /// Authored sand PBR pack used for the hub platform
    /// disc. Lazy-loaded by [`Self::ensure_desert_rocks`].
    /// The sand pack ships only basecolor / normal / roughness
    /// / AO / height (no metallic) — the MR atlas decoder
    /// substitutes a constant zero-metallic channel, which is
    /// what we want for desert sand anyway.
    pub desert_rocks_set: Option<vk::DescriptorSet>,
    /// Authored PBR cliff-rocks material used for the
    /// procedural mountain-ring terrain around the hub.
    /// Loaded lazily by [`Self::ensure_cliff_rocks`]. Unlike
    /// the disc, the mountains *do* benefit from PBR: at
    /// silhouette range the normal/AO maps give the rim
    /// light real bite, and parallax sells the sense of
    /// depth on the closer flanks.
    pub cliff_rocks_set: Option<vk::DescriptorSet>,
    textures: Vec<Texture>,
    /// Background-decode worker for authored material packs.
    /// Lazily spawned the first time `tick_world_preload` is
    /// called. The worker thread decodes each PBR pack's PNGs
    /// to RGBA bytes (the slow part — pure CPU, no Vulkan)
    /// and ships the result back over an `mpsc` channel; the
    /// main thread polls the channel and runs only the GPU
    /// upload step. Using a worker means the main render
    /// thread keeps drawing the character-select screen and
    /// pumping renet packets while the textures decode in
    /// the background.
    decode_worker: Option<DecodeWorker>,
}

impl Default for EnvTextures {
    fn default() -> Self {
        Self {
            floor_set: None,
            wall_set: None,
            grass_floor_set: None,
            demon_ground_set: None,
            crimson_stone_set: None,
            bricks_wall_set: None,
            ground_tiles_set: None,
            desert_rocks_set: None,
            cliff_rocks_set: None,
            textures: Vec::new(),
            decode_worker: None,
        }
    }
}

impl EnvTextures {
    /// Upload (or re-upload) procedural floor and wall textures sized to
    /// the current floor's theme.  Old textures are dropped *only* when
    /// `cleanup_gpu` is called — repeated calls allocate fresh descriptor
    /// sets and grow `self.textures`.  In practice we call `ensure` once
    /// at startup and reuse the same sets for every floor.
    pub fn ensure(&mut self, renderer: &mut Renderer) {
        if self.floor_set.is_none() {
            let pixels = generate_floor(FLOOR_SIZE);
            match renderer.upload_shared_texture_from_rgba(FLOOR_SIZE, FLOOR_SIZE, &pixels) {
                Ok((tex, set)) => {
                    self.textures.push(tex);
                    self.floor_set = Some(set);
                }
                Err(e) => log::warn!("env floor texture upload failed: {}", e),
            }
        }
        if self.wall_set.is_none() {
            let pixels = generate_wall(WALL_SIZE);
            match renderer.upload_shared_texture_from_rgba(WALL_SIZE, WALL_SIZE, &pixels) {
                Ok((tex, set)) => {
                    self.textures.push(tex);
                    self.wall_set = Some(set);
                }
                Err(e) => log::warn!("env wall texture upload failed: {}", e),
            }
        }
    }

    pub fn cleanup_gpu(&mut self, device: &Device, allocator: &Arc<Mutex<Allocator>>) {
        for mut tex in self.textures.drain(..) {
            tex.cleanup(device, allocator);
        }
        self.floor_set = None;
        self.wall_set = None;
        self.grass_floor_set = None;
        self.demon_ground_set = None;
        self.crimson_stone_set = None;
        self.bricks_wall_set = None;
        self.ground_tiles_set = None;
        self.desert_rocks_set = None;
    }

    /// Lazy-initialise the grass tile used by the outdoor hub.
    /// Idempotent: subsequent calls are a no-op once the descriptor
    /// set has been allocated.
    pub fn ensure_grass(&mut self, renderer: &mut Renderer) {
        if self.grass_floor_set.is_some() {
            return;
        }
        let pixels = generate_grass(FLOOR_SIZE);
        match renderer.upload_shared_texture_from_rgba(FLOOR_SIZE, FLOOR_SIZE, &pixels) {
            Ok((tex, set)) => {
                self.textures.push(tex);
                self.grass_floor_set = Some(set);
            }
            Err(e) => log::warn!("env grass texture upload failed: {}", e),
        }
    }

    /// Lazy-initialise the `demon_ground_01` tile used as the
    /// hub platform surface. Decoded once from the JPG asset
    /// and reused across hub regenerations. No-op once the
    /// descriptor set has been allocated.
    pub fn ensure_demon_ground(&mut self, renderer: &mut Renderer) {
        if self.demon_ground_set.is_some() {
            return;
        }
        match renderer.upload_shared_texture_from_file(
            "assets/textures/demon_ground_01.jpg",
        ) {
            Ok((tex, set)) => {
                self.textures.push(tex);
                self.demon_ground_set = Some(set);
            }
            Err(e) => log::warn!("env demon_ground texture upload failed: {}", e),
        }
    }

    /// Lazy-initialise the procedural crimson cracked-stone
    /// tile used for the abyss-hub floating platform. Generated
    /// once, cached for the rest of the session. The tile is a
    /// dark oxblood plate veined with thin glowing crimson
    /// cracks and weathered with fbm grime so each platform
    /// region reads as natural rather than uniform.
    pub fn ensure_crimson_stone(&mut self, renderer: &mut Renderer) {
        if self.crimson_stone_set.is_some() {
            return;
        }
        // Generated at higher resolution than the dungeon
        // floor tile because the hub platform stretches the
        // texture across ~14 m per repeat (vs. 1 m for dungeon
        // floors), so each texel covers more world space and
        // we need more detail to keep features crisp.
        const CRIMSON_SIZE: u32 = 512;
        let pixels = generate_crimson_stone(CRIMSON_SIZE);
        match renderer.upload_shared_texture_from_rgba(CRIMSON_SIZE, CRIMSON_SIZE, &pixels) {
            Ok((tex, set)) => {
                self.textures.push(tex);
                self.crimson_stone_set = Some(set);
            }
            Err(e) => log::warn!("env crimson_stone texture upload failed: {}", e),
        }
    }

    /// Lazy-initialise the authored PBR brick wall material from
    /// `assets/textures/bricks_wall/`. Loads basecolor + OpenGL-
    /// convention normal map + AO + height + (metallic, roughness)
    /// into a single per-object descriptor set so the PBR shader
    /// path can read every channel at once. The metallic and
    /// roughness PNGs ship as separate single-channel files; the
    /// renderer packs them on the CPU into a single MR atlas
    /// before upload.
    pub fn ensure_bricks_wall(&mut self, renderer: &mut Renderer) {
        if self.bricks_wall_set.is_some() {
            return;
        }
        use std::path::Path;
        let result = renderer.upload_shared_pbr_material_split_mr(
            Path::new("assets/textures/bricks_wall/bricks_wall_07_baseColor_2k.png"),
            Some(Path::new("assets/textures/bricks_wall/bricks_wall_07_normal_gl_2k.png")),
            Some(Path::new("assets/textures/bricks_wall/bricks_wall_07_metallic_2k.png")),
            Some(Path::new("assets/textures/bricks_wall/bricks_wall_07_roughness_2k.png")),
            Some(Path::new("assets/textures/bricks_wall/bricks_wall_07_ambientOcclusion_2k.png")),
            Some(Path::new("assets/textures/bricks_wall/bricks_wall_07_height_2k.png")),
        );
        match result {
            Ok((mut texs, set)) => {
                self.textures.append(&mut texs);
                self.bricks_wall_set = Some(set);
            }
            Err(e) => log::warn!("env bricks_wall PBR upload failed: {}", e),
        }
    }

    /// Lazy-initialise the authored PBR ground tile material from
    /// `assets/textures/ground_tiles/`. See
    /// [`Self::ensure_bricks_wall`] for channel handling notes.
    pub fn ensure_ground_tiles(&mut self, renderer: &mut Renderer) {
        if self.ground_tiles_set.is_some() {
            return;
        }
        use std::path::Path;
        let result = renderer.upload_shared_pbr_material_split_mr(
            Path::new("assets/textures/ground_tiles/ground_tiles_25_basecolor_2k.png"),
            Some(Path::new("assets/textures/ground_tiles/ground_tiles_25_normal_gl_2k.png")),
            Some(Path::new("assets/textures/ground_tiles/ground_tiles_25_metallic_2k.png")),
            Some(Path::new("assets/textures/ground_tiles/ground_tiles_25_roughness_2k.png")),
            Some(Path::new("assets/textures/ground_tiles/ground_tiles_25_ambientocclusion_2k.png")),
            Some(Path::new("assets/textures/ground_tiles/ground_tiles_25_height_2k.png")),
        );
        match result {
            Ok((mut texs, set)) => {
                self.textures.append(&mut texs);
                self.ground_tiles_set = Some(set);
            }
            Err(e) => log::warn!("env ground_tiles PBR upload failed: {}", e),
        }
    }

    /// Lazy-initialise the desert-rocks **basecolor only**
    /// texture used for the hub-platform surface. We
    /// deliberately skip the PBR channels here: the disc is
    /// viewed from far overhead, so PBR specular highlights
    /// wander distractingly across it as the camera moves,
    /// and normal/height detail is invisible at that distance.
    /// Cel-shading on the basecolor gives a calm painterly
    /// finish at a fraction of the per-fragment cost.
    /// Lazy-initialise the authored sand PBR pack used to
    /// surface the hub platform disc. The pack ships no
    /// metallic map (sand is fully dielectric), so we pass
    /// `None` for that channel and let the MR atlas decoder
    /// fill in a zero metallic value.
    pub fn ensure_desert_rocks(&mut self, renderer: &mut Renderer) {
        if self.desert_rocks_set.is_some() {
            return;
        }
        use std::path::Path;
        let result = renderer.upload_shared_pbr_material_split_mr(
            Path::new("assets/textures/sand/sand_04_color_2k.png"),
            Some(Path::new("assets/textures/sand/sand_04_normal_gl_2k.png")),
            None,
            Some(Path::new("assets/textures/sand/sand_04_roughness_2k.png")),
            Some(Path::new("assets/textures/sand/sand_04_ambient_occlusion_2k.png")),
            Some(Path::new("assets/textures/sand/sand_04_height_2k.png")),
        );
        match result {
            Ok((mut texs, set)) => {
                log::info!("env: bound sand PBR pack (hub platform)");
                self.textures.append(&mut texs);
                self.desert_rocks_set = Some(set);
            }
            Err(e) => log::warn!("env sand PBR upload failed: {}", e),
        }
    }

    /// Lazy-initialise the authored sandy cliff-rocks PBR
    /// material used for the procedural mountain ring around
    /// the hub. See [`Self::ensure_bricks_wall`] for channel
    /// handling notes — the metallic + roughness PNGs are
    /// packed on the CPU into a single MR atlas before
    /// upload.
    pub fn ensure_cliff_rocks(&mut self, renderer: &mut Renderer) {
        if self.cliff_rocks_set.is_some() {
            return;
        }
        use std::path::Path;
        let result = renderer.upload_shared_pbr_material_split_mr(
            Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_color_2k.png"),
            Some(Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_normal_gl_2k.png")),
            Some(Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_metallic_2k.png")),
            Some(Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_roughness_2k.png")),
            Some(Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_ambient_occlusion_2k.png")),
            Some(Path::new("assets/textures/sandy_cliff_rocks/cliff_rocks_01_height_2k.png")),
        );
        match result {
            Ok((mut texs, set)) => {
                self.textures.append(&mut texs);
                self.cliff_rocks_set = Some(set);
            }
            Err(e) => log::warn!("env sandy_cliff_rocks PBR upload failed: {}", e),
        }
    }

    /// Drive the background pre-warm of authored PBR packs.
    /// On the first call this spawns a worker thread that
    /// decodes every world material's PNGs to RGBA bytes; on
    /// each subsequent call we drain whatever the worker has
    /// finished and run the (fast) GPU upload step on the
    /// main thread. Returns `true` while any pack is still
    /// outstanding so callers know to keep ticking.
    ///
    /// The decode is the expensive half (~0.5–1 s per 2 k
    /// PNG, pure CPU); the upload is a few hundred
    /// milliseconds total even for the full set. Splitting
    /// them across a worker means the main thread keeps
    /// drawing the character-select UI and pumping renet
    /// packets at full rate, so renetcode's 5 s timeout
    /// never gets near triggering.
    pub fn tick_world_preload(&mut self, renderer: &mut Renderer) -> bool {
        // Spawn the worker on first poll. Subsequent calls
        // just drain its outbox.
        if self.decode_worker.is_none() {
            self.decode_worker = Some(DecodeWorker::spawn());
        }
        let worker = self.decode_worker.as_mut().expect("just spawned");

        // Drain ready jobs. We `try_recv` in a loop so a fast
        // worker that finished multiple packs between frames
        // can drop them all into descriptor sets in one tick;
        // each upload is on the order of tens of ms so even
        // four-in-a-row is well under the netcode budget.
        while let Ok(done) = worker.outbox.try_recv() {
            match done {
                DecodeOutput::DesertRocks(pack) => {
                    if self.desert_rocks_set.is_none() {
                        match renderer.upload_shared_pbr_material_decoded(pack) {
                            Ok((mut texs, set)) => {
                                self.textures.append(&mut texs);
                                self.desert_rocks_set = Some(set);
                                log::info!("env: bound sand PBR pack (worker)");
                            }
                            Err(e) => log::warn!(
                                "env sand PBR decoded upload failed: {}",
                                e
                            ),
                        }
                    }
                }
                DecodeOutput::Pbr(name, pack) => match renderer.upload_shared_pbr_material_decoded(pack)
                {
                    Ok((mut texs, set)) => {
                        self.textures.append(&mut texs);
                        match name.as_str() {
                            "cliff_rocks" => {
                                if self.cliff_rocks_set.is_none() {
                                    self.cliff_rocks_set = Some(set);
                                }
                            }
                            "ground_tiles" => {
                                if self.ground_tiles_set.is_none() {
                                    self.ground_tiles_set = Some(set);
                                }
                            }
                            "bricks_wall" => {
                                if self.bricks_wall_set.is_none() {
                                    self.bricks_wall_set = Some(set);
                                }
                            }
                            other => log::warn!(
                                "env: unknown PBR pack name from worker: {other}"
                            ),
                        }
                        log::info!("env: bound PBR pack `{name}` (worker)");
                    }
                    Err(e) => log::warn!(
                        "env PBR pack `{name}` decoded upload failed: {}",
                        e
                    ),
                },
                DecodeOutput::Failed(name, err) => {
                    log::warn!("env: worker failed to decode `{name}`: {err}");
                }
            }
            worker.in_flight = worker.in_flight.saturating_sub(1);
        }

        worker.in_flight > 0
    }
}

// ---------------------------------------------------------------------
// Background-decode worker
// ---------------------------------------------------------------------

/// Output of one decode job. Either a fully-decoded texture /
/// pack ready to upload on the main thread, or a failure
/// message so the main thread can log and move on.
enum DecodeOutput {
    DesertRocks(DecodedPbrPack),
    Pbr(String, DecodedPbrPack),
    Failed(String, String),
}

/// Persistent decode worker. Spawned on first
/// [`EnvTextures::tick_world_preload`] call. The thread runs
/// through a fixed list of jobs once and then exits. The
/// receiver side stays alive for the lifetime of
/// [`EnvTextures`] so a re-poll after the worker thread has
/// joined still drains any pending messages from the channel
/// buffer cleanly.
struct DecodeWorker {
    outbox: mpsc::Receiver<DecodeOutput>,
    /// Number of jobs the worker is still expected to
    /// produce. Decremented as messages are consumed; once
    /// it hits zero `tick_world_preload` returns `false`
    /// and the caller can stop polling.
    in_flight: usize,
}

impl DecodeWorker {
    fn spawn() -> Self {
        // Manifest of background jobs. Ordered largest-
        // perceptual-impact-first so a player who clicks
        // Play very quickly sees the most visible surfaces
        // (hub disc, mountains) bind first.
        type Job = Box<dyn FnOnce() -> DecodeOutput + Send + 'static>;
        let jobs: Vec<Job> = vec![
            Box::new(|| match decode_pbr_pack("sand", &SAND_PATHS) {
                DecodeOutput::Pbr(_, pack) => DecodeOutput::DesertRocks(pack),
                other => other,
            }),
            Box::new(|| decode_pbr_pack("cliff_rocks", &CLIFF_ROCKS_PATHS)),
            Box::new(|| decode_pbr_pack("ground_tiles", &GROUND_TILES_PATHS)),
            Box::new(|| decode_pbr_pack("bricks_wall", &BRICKS_WALL_PATHS)),
        ];
        let (tx, rx) = mpsc::channel();
        let in_flight = jobs.len();
        std::thread::Builder::new()
            .name("env-decode".into())
            .spawn(move || {
                for job in jobs {
                    let result = job();
                    if tx.send(result).is_err() {
                        // Receiver dropped — main thread
                        // probably exiting. Stop quietly.
                        return;
                    }
                }
            })
            .expect("spawn env-decode worker");
        Self {
            outbox: rx,
            in_flight,
        }
    }
}

/// Spec describing the file paths for one authored PBR pack.
struct PbrPackPaths {
    basecolor: &'static str,
    normal: Option<&'static str>,
    metallic: Option<&'static str>,
    roughness: Option<&'static str>,
    ao: Option<&'static str>,
    height: Option<&'static str>,
}

const SAND_PATHS: PbrPackPaths = PbrPackPaths {
    basecolor: "assets/textures/sand/sand_04_color_2k.png",
    normal: Some("assets/textures/sand/sand_04_normal_gl_2k.png"),
    // Sand is dielectric — no metallic map is shipped, so
    // the MR atlas builder substitutes zero metallic.
    metallic: None,
    roughness: Some("assets/textures/sand/sand_04_roughness_2k.png"),
    ao: Some("assets/textures/sand/sand_04_ambient_occlusion_2k.png"),
    height: Some("assets/textures/sand/sand_04_height_2k.png"),
};

const CLIFF_ROCKS_PATHS: PbrPackPaths = PbrPackPaths {
    basecolor: "assets/textures/sandy_cliff_rocks/cliff_rocks_01_color_2k.png",
    normal: Some("assets/textures/sandy_cliff_rocks/cliff_rocks_01_normal_gl_2k.png"),
    metallic: Some("assets/textures/sandy_cliff_rocks/cliff_rocks_01_metallic_2k.png"),
    roughness: Some("assets/textures/sandy_cliff_rocks/cliff_rocks_01_roughness_2k.png"),
    ao: Some("assets/textures/sandy_cliff_rocks/cliff_rocks_01_ambient_occlusion_2k.png"),
    height: Some("assets/textures/sandy_cliff_rocks/cliff_rocks_01_height_2k.png"),
};

const GROUND_TILES_PATHS: PbrPackPaths = PbrPackPaths {
    basecolor: "assets/textures/ground_tiles/ground_tiles_25_basecolor_2k.png",
    normal: Some("assets/textures/ground_tiles/ground_tiles_25_normal_gl_2k.png"),
    metallic: Some("assets/textures/ground_tiles/ground_tiles_25_metallic_2k.png"),
    roughness: Some("assets/textures/ground_tiles/ground_tiles_25_roughness_2k.png"),
    ao: Some("assets/textures/ground_tiles/ground_tiles_25_ambientocclusion_2k.png"),
    height: Some("assets/textures/ground_tiles/ground_tiles_25_height_2k.png"),
};

const BRICKS_WALL_PATHS: PbrPackPaths = PbrPackPaths {
    basecolor: "assets/textures/bricks_wall/bricks_wall_07_baseColor_2k.png",
    normal: Some("assets/textures/bricks_wall/bricks_wall_07_normal_gl_2k.png"),
    metallic: Some("assets/textures/bricks_wall/bricks_wall_07_metallic_2k.png"),
    roughness: Some("assets/textures/bricks_wall/bricks_wall_07_roughness_2k.png"),
    ao: Some("assets/textures/bricks_wall/bricks_wall_07_ambientOcclusion_2k.png"),
    height: Some("assets/textures/bricks_wall/bricks_wall_07_height_2k.png"),
};

fn decode_pbr_pack(name: &'static str, paths: &PbrPackPaths) -> DecodeOutput {
    let pb = std::path::Path::new;
    let basecolor = match decode_srgb(pb(paths.basecolor)) {
        Ok(d) => d,
        Err(e) => return DecodeOutput::Failed(name.into(), e.to_string()),
    };
    let opt_linear = |p: Option<&'static str>| -> std::result::Result<Option<DecodedTexture>, String> {
        match p {
            None => Ok(None),
            Some(s) => decode_linear(pb(s))
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    };
    let normal = match opt_linear(paths.normal) {
        Ok(v) => v,
        Err(e) => return DecodeOutput::Failed(name.into(), e),
    };
    let mr = match decode_mr_atlas(paths.metallic.map(pb), paths.roughness.map(pb)) {
        Ok(v) => v,
        Err(e) => return DecodeOutput::Failed(name.into(), e.to_string()),
    };
    let ao = match opt_linear(paths.ao) {
        Ok(v) => v,
        Err(e) => return DecodeOutput::Failed(name.into(), e),
    };
    let height = match opt_linear(paths.height) {
        Ok(v) => v,
        Err(e) => return DecodeOutput::Failed(name.into(), e),
    };
    DecodeOutput::Pbr(
        name.into(),
        DecodedPbrPack {
            name: name.into(),
            basecolor,
            normal,
            mr,
            ao,
            height,
        },
    )
}

// ---------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------

fn hash2(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x27D4_EB2D)
        ^ (y as u32).wrapping_mul(0x9E37_79B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

fn rand01(seed: u32) -> f32 {
    (seed & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

/// Smooth value-noise sample at (u, v) in [0,1]^2 using `cells` cells
/// per axis. Wraps so the result tiles seamlessly.
fn vnoise(u: f32, v: f32, cells: u32, seed: u32) -> f32 {
    let su = u * cells as f32;
    let sv = v * cells as f32;
    let x0 = su.floor() as i32;
    let y0 = sv.floor() as i32;
    let fx = su - x0 as f32;
    let fy = sv - y0 as f32;
    let sx = rift_math::smoothstep(fx);
    let sy = rift_math::smoothstep(fy);
    let h = |ix: i32, iy: i32| -> f32 {
        let wx = ix.rem_euclid(cells as i32);
        let wy = iy.rem_euclid(cells as i32);
        rand01(hash2(wx, wy).wrapping_add(seed))
    };
    let a = h(x0, y0);
    let b = h(x0 + 1, y0);
    let c = h(x0, y0 + 1);
    let d = h(x0 + 1, y0 + 1);
    let ab = a + (b - a) * sx;
    let cd = c + (d - c) * sx;
    ab + (cd - ab) * sy
}

fn fbm(u: f32, v: f32, base_cells: u32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    let mut cells = base_cells;
    for o in 0..octaves {
        sum += vnoise(u, v, cells, seed.wrapping_add(o * 131)) * amp;
        total += amp;
        amp *= 0.5;
        cells = (cells * 2).max(1);
    }
    sum / total
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn pack(c: [f32; 3]) -> [u8; 4] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

/// Cobblestone floor: irregular polygonal cells, dark grout between them,
/// faint moss in the cracks. Tiles seamlessly.
fn generate_floor(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];

    // Worley-style cells: pick N jittered points, color each pixel by
    // distance to the nearest point and to the second-nearest (for grout).
    let cell_count: i32 = 6; // cells per axis
    let inv = 1.0 / size as f32;

    // Pre-compute jittered cell points (in [0,1] tiling space).
    let cells = cell_count;
    let seed_pts = 0xA1B2_C3D4u32;

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;
            let cu = (u * cells as f32).floor() as i32;
            let cv = (v * cells as f32).floor() as i32;
            let mut d1 = f32::INFINITY;
            let mut d2 = f32::INFINITY;
            let mut nearest: (i32, i32) = (0, 0);
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let cx = cu + ox;
                    let cy = cv + oy;
                    // Hash uses wrapped cell (so the same point appears on
                    // both sides of the seam), but the world-space position
                    // uses the un-wrapped cell so distance is correct.
                    let hx = cx.rem_euclid(cells);
                    let hy = cy.rem_euclid(cells);
                    let h = hash2(hx, hy).wrapping_add(seed_pts);
                    let jx = rand01(h);
                    let jy = rand01(h.wrapping_mul(0x9E37));
                    let px = (cx as f32 + 0.15 + 0.7 * jx) / cells as f32;
                    let py = (cy as f32 + 0.15 + 0.7 * jy) / cells as f32;
                    let ddx = px - u;
                    let ddy = py - v;
                    let d = ddx * ddx + ddy * ddy;
                    if d < d1 {
                        d2 = d1;
                        d1 = d;
                        nearest = (hx, hy);
                    } else if d < d2 {
                        d2 = d;
                    }
                }
            }
            let edge = (d2.sqrt() - d1.sqrt()).max(0.0);

            // Per-cell stone tint variation.
            let cell_h = hash2(nearest.0, nearest.1);
            let stone_tone = rand01(cell_h);
            let warm = lerp3([0.34, 0.30, 0.27], [0.46, 0.42, 0.38], stone_tone);

            // Inner stone fbm noise for surface detail.
            let detail = fbm(u, v, 8, 4, 0x55AA_3322);
            let stone = lerp3(
                [warm[0] * 0.85, warm[1] * 0.85, warm[2] * 0.85],
                [warm[0] * 1.10, warm[1] * 1.08, warm[2] * 1.06],
                detail,
            );

            // Grout: dark when edge distance is small.
            let grout_t = (1.0 - (edge * 18.0).min(1.0)).powf(2.0);
            let grout_color = [0.10, 0.09, 0.08];
            let mut color = lerp3(stone, grout_color, grout_t);

            // Subtle moss in the grout cracks.
            let moss_n = fbm(u * 1.3, v * 1.3, 5, 3, 0x77BB_99CC);
            let moss_t = grout_t * (moss_n - 0.45).max(0.0) * 1.6;
            color = lerp3(color, [0.10, 0.18, 0.08], moss_t.clamp(0.0, 0.7));

            // Dust speckles.
            let speck = fbm(u, v, 64, 2, 0x4242_4242);
            if speck > 0.86 {
                color = lerp3(color, [0.55, 0.50, 0.42], 0.4);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}

/// Ashlar (rectangular block) wall: courses of large stones with mortar
/// between them, alternating offsets per row, plus surface noise.  Tiles
/// vertically every `BRICK_ROWS` courses and horizontally every block
/// width, so the wall mesh's UV mapping (u in [0,1], v in [0, h]) repeats
/// cleanly.
fn generate_wall(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    const ROWS: u32 = 4;          // courses per UV repeat
    const COLS_EVEN: u32 = 2;     // blocks across at v=0
    const COLS_ODD: u32 = 2;      // staggered rows have same density, half-offset

    let inv = 1.0 / size as f32;
    let mortar = [0.07, 0.06, 0.055];

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;
            let row_f = v * ROWS as f32;
            let row = row_f.floor() as i32;
            let row_frac = row_f - row as f32;
            let cols = if row % 2 == 0 { COLS_EVEN } else { COLS_ODD };
            let offset = if row % 2 == 0 { 0.0 } else { 0.5 };
            let col_f = u * cols as f32 + offset;
            let col = col_f.floor() as i32;
            let col_frac = col_f - col as f32;

            // Distance to nearest mortar line in normalized brick space.
            let dx = (col_frac - 0.5).abs() * 2.0;     // 0 center, 1 edge
            let dy = (row_frac - 0.5).abs() * 2.0;
            let edge = 1.0 - dx.max(dy);              // 0 = on edge, ~1 = center
            let mortar_t = (1.0 - (edge * 14.0).min(1.0)).powf(2.0);

            // Per-block stone tint.
            let cell_h = hash2(col.rem_euclid(cols as i32), row.rem_euclid(ROWS as i32));
            let stone_tone = rand01(cell_h);
            let warm = lerp3([0.30, 0.27, 0.24], [0.45, 0.40, 0.36], stone_tone);

            // Surface detail inside the block.
            let detail = fbm(u * 4.0, v * 4.0, 6, 4, 0x33AA_55BB);
            let stone = lerp3(
                [warm[0] * 0.82, warm[1] * 0.82, warm[2] * 0.82],
                [warm[0] * 1.10, warm[1] * 1.08, warm[2] * 1.06],
                detail,
            );

            // Subtle horizontal streaking inside each block (water staining).
            let streak = fbm(u * 8.0, v * 1.5, 3, 3, 0x9911_2233);
            let stone = lerp3(stone, [stone[0] * 0.85, stone[1] * 0.84, stone[2] * 0.82], (streak - 0.5).max(0.0) * 0.6);

            let mut color = lerp3(stone, mortar, mortar_t);

            // Cracks: thin dark streaks scattered over the surface.
            let crack = fbm(u * 2.0, v * 2.0, 5, 4, 0xBEEF_F00D);
            if (crack - 0.55).abs() < 0.025 {
                color = lerp3(color, [0.05, 0.04, 0.04], 0.65);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}

/// Lush meadow grass: warm green base modulated by clumpy fbm with
/// occasional dirt patches and tiny yellow flower specks. Tiles
/// seamlessly at the same scale as `generate_floor`.
fn generate_grass(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let inv = 1.0 / size as f32;

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;

            let clump = fbm(u, v, 4, 4, 0x6A11_CE03);
            let blade = fbm(u, v, 32, 3, 0x12AB_CD34);

            let cool = [0.20, 0.46, 0.18];
            let warm = [0.42, 0.62, 0.22];
            let mut color = lerp3(cool, warm, clump);

            color = lerp3(
                [color[0] * 0.78, color[1] * 0.82, color[2] * 0.74],
                [color[0] * 1.18, color[1] * 1.14, color[2] * 1.10],
                blade,
            );

            let dirt_t = (0.18 - clump).max(0.0) * 4.0;
            color = lerp3(color, [0.34, 0.26, 0.18], dirt_t.clamp(0.0, 0.55));

            let speck = fbm(u, v, 96, 2, 0xF10A_77E5);
            if speck > 0.88 && clump > 0.45 {
                color = lerp3(color, [0.95, 0.86, 0.30], 0.55);
            }
            let speck2 = fbm(u, v, 96, 2, 0x21B5_DDAA);
            if speck2 > 0.90 && clump > 0.40 {
                color = lerp3(color, [0.92, 0.92, 0.86], 0.55);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}

/// Crimson cracked stone: dark oxblood plate veined with thin
/// glowing fissures and weathered with low-frequency grime so
/// the tile reads as natural rather than uniform. Used as the
/// abyss-hub floating platform surface. Tiles seamlessly at
/// the same scale as the other floor textures.
fn generate_crimson_stone(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let inv = 1.0 / size as f32;

    for y in 0..size {
        for x in 0..size {
            let u = x as f32 * inv;
            let v = y as f32 * inv;

            // Two-layer base: a slow patchy stain that biases
            // the plate between dim oxblood and near-black,
            // plus a finer crunch for surface microvariation.
            let stain = fbm(u, v, 3, 4, 0xC110_55E1);
            let crunch = fbm(u, v, 24, 4, 0xD1A8_011D);

            let dim = [0.060, 0.020, 0.024];
            let mid = [0.140, 0.045, 0.050];
            let mut color = lerp3(dim, mid, stain);

            // Microvariation: brighten / darken by +/-15 % based
            // on the fine fbm so the plate has visible grain.
            let micro = (crunch - 0.5) * 0.30;
            color = [
                (color[0] * (1.0 + micro)).max(0.0),
                (color[1] * (1.0 + micro)).max(0.0),
                (color[2] * (1.0 + micro)).max(0.0),
            ];

            // Soot patches: a separate low-freq fbm pulls some
            // areas toward near-black so the surface looks
            // burnt unevenly rather than tinted.
            let soot = fbm(u, v, 5, 3, 0x5007_F00D);
            let soot_t = ((0.42 - soot).max(0.0) * 2.5).clamp(0.0, 0.65);
            color = lerp3(color, [0.020, 0.010, 0.012], soot_t);

            // Cracks: two crossing thin-line networks of
            // different scales. Each line is detected by the
            // thin-isovalue trick: |fbm - threshold| < epsilon
            // => on the crack.
            let crack_a = fbm(u, v, 3, 5, 0xCA1A_DA17);
            let crack_b = fbm(u, v, 7, 5, 0x9F00_BABE);

            // Wide structural fissures with a glowing oxblood
            // core so they read as splits with residual heat.
            if (crack_a - 0.50).abs() < 0.012 {
                let d = (crack_a - 0.50).abs() / 0.012;
                let edge = 1.0 - d;
                let hot = [0.55, 0.10, 0.06];
                let groove = [0.020, 0.005, 0.005];
                let line_color = lerp3(groove, hot, edge.powf(2.0));
                color = lerp3(color, line_color, edge.clamp(0.0, 1.0) * 0.85);
            }
            // Finer secondary cracks: narrower, dimmer.
            if (crack_b - 0.55).abs() < 0.008 {
                let d = (crack_b - 0.55).abs() / 0.008;
                let edge = 1.0 - d;
                color = lerp3(color, [0.030, 0.008, 0.010], edge * 0.65);
            }

            // Sparse molten flecks: rare bright points where
            // the cracks intersect a hot pocket. Drives bloom
            // into the floor so the platform glints under
            // lightning flashes.
            let flecks = fbm(u, v, 64, 2, 0xF1A1_B0B0);
            if flecks > 0.92 && stain > 0.45 {
                color = lerp3(color, [0.95, 0.32, 0.12], 0.55);
            }

            let i = ((y * size + x) * 4) as usize;
            let p = pack(color);
            out[i] = p[0];
            out[i + 1] = p[1];
            out[i + 2] = p[2];
            out[i + 3] = p[3];
        }
    }
    out
}
