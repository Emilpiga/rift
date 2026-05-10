//! Hub entry portal + boss-room exit portal.
//!
//! Both portals are visually identical (a glowing ring + vortex
//! emitter); they differ only in placement (hub vs. boss-room
//! center) and the request fired on F-press (the hub portal
//! starts a rift run; the exit portal advances one floor — the
//! server reads `RequestEnterRift` as "advance" when
//! `floor_index != 0`).
//!
//! Free-standing functions taking explicit borrows of the
//! `GameState` slices they actually touch.

use glam::{Mat4, Quat, Vec3};
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::{Input, Renderer};

use crate::game::sub_state::{NetState, NetTransitionRequest};

/// Walk-to-interact range. Within this distance the player gets
/// a press-F prompt and the F key triggers an `EnterRift`
/// request. Used for both the hub entry portal and the boss-room
/// exit portal.
pub const INTERACT_RADIUS: f32 = 2.2;

/// Compute the Y-axis yaw that points the portal's *visible*
/// face (the side the player should see) at the local player.
///
/// The mesh's geometric +Z is mirrored to a back face with the
/// same colours, so visually either side reads as "the disc",
/// but the front winding is generated for viewers on the +Z
/// side. We want the player to be on the front-facing side, so
/// we rotate so +Z points *away* from the player — that puts
/// the player on the front-face viewing side. (Earlier we had
/// the sign flipped; this matches what playtesting shows.)
fn facing_yaw(world: &hecs::World, portal_pos: Vec3) -> f32 {
    let Some(p) = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()
    else {
        return 0.0;
    };
    let dx = p.x - portal_pos.x;
    let dz = p.z - portal_pos.z;
    if dx.abs() < 1e-4 && dz.abs() < 1e-4 {
        0.0
    } else {
        // Flip 180° vs. naive "+Z toward player" so the
        // correct face is presented.
        (-dx).atan2(-dz)
    }
}

/// Apply the billboard yaw to both the portal mesh's
/// `model_matrix` and the VFX emitter's orientation, so the
/// glowing halo / flame licks rotate together with the disc.
///
/// On top of the Y-axis billboard we also spin the mesh around
/// its *local* Z axis (the disc normal). The frame torus is
/// rotationally symmetric so the spin only shows on the inner
/// disc — its radial gradient + per-vertex hash modulation read
/// as a slowly turning swirl. The VFX emitter's orientation is
/// also given the Z-spin so the orbiting halo sparks share the
/// disc's reference frame.
fn apply_facing(portal: &HubPortal, world: &hecs::World, renderer: &mut Renderer) {
    /// Radians/sec the disc rotates around its own normal. Slow
    /// enough to read as "the other side is alive" without
    /// reading as a fan blade.
    const DISC_SPIN: f32 = 0.6;
    /// Y-offset (in model space) of the disc centre — the
    /// `cy_offset = height / 2` constant baked into
    /// [`rift_engine::renderer::mesh::Mesh::portal_with_palette`].
    /// We pivot the Z-spin around this so the disc rotates
    /// around its own centre instead of the mesh origin (which
    /// sits at the floor).
    const DISC_CENTRE_Y: f32 = 1.05;

    let yaw = facing_yaw(world, portal.position);
    let spin = portal.age * DISC_SPIN;
    let billboard = Quat::from_rotation_y(yaw);
    let local_spin = Quat::from_rotation_z(spin);
    if let Some(obj) = renderer.objects.get_mut(portal.obj_idx) {
        // World <- billboard <- pivot-around-disc-centre <- spin.
        let centre = Vec3::new(0.0, DISC_CENTRE_Y, 0.0);
        obj.model_matrix = Mat4::from_translation(portal.position)
            * Mat4::from_quat(billboard)
            * Mat4::from_translation(centre)
            * Mat4::from_quat(local_spin)
            * Mat4::from_translation(-centre);
    }
    // The VFX emitter is already anchored at the disc centre
    // (see `PORTAL_CENTRE_Y` in the portal preset), so no
    // translation pivot is needed — just hand it the combined
    // rotation.
    renderer
        .vfx_system
        .set_orientation(portal.emitter_idx, billboard * local_spin);
}

/// Which destination a portal leads to. Drives both the disc's
/// baked sky palette (set at spawn time via `SkyConfig`) and
/// the per-frame point-light colour pushed by `push_lights`.
/// Keeping the two in lock-step is essential — the player
/// reads the *same chromatic signature* from both the disc and
/// the surrounding ground spill, so the at-a-glance "is this
/// the way deeper or the way home?" decision works at any
/// camera angle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalKind {
    /// Hub → rift entry. Crimson rift palette, intended to
    /// read as "you are about to enter danger". Same chroma
    /// as `Descend` — they share the visual language of
    /// "deeper into the rift".
    HubEntry,
    /// Rift boss room → next floor. Crimson rift palette,
    /// matching `HubEntry` so the player learns "red portal
    /// = deeper".
    Descend,
    /// Rift boss room → hub. Cool cyan/teal palette, the
    /// chromatic *opposite* of the descend portal so the two
    /// portals standing side-by-side in the boss-room corridor
    /// can never be confused at a glance.
    Extract,
}

/// Visual + interaction state for a single portal (hub entry,
/// descend, or extract). The three are structurally identical
/// apart from `kind`, which drives both the spawn-time disc
/// palette and the per-frame light colour.
pub struct HubPortal {
    /// World-space position of the portal centre.
    pub position: Vec3,
    /// Render-object index of the portal mesh in
    /// `renderer.objects`. We mutate `model_matrix` here every
    /// frame to spin it.
    pub obj_idx: usize,
    /// Particle emitter index for the swirling vortex.
    pub emitter_idx: rift_engine::renderer::vfx::EffectId,
    /// Looping portal-hum audio emitter. `None` when the
    /// audio system is unavailable or the asset is missing.
    /// Volume is driven each frame by [`tick_audio`] from
    /// the same breathing/spasm pulse that modulates the
    /// portal light, so the hum throbs in lockstep with
    /// the visible disc.
    pub audio: Option<rift_audio::EmitterId>,
    /// Seconds since the portal was spawned. Drives rotation.
    pub age: f32,
    /// Destination class. Selects the per-frame light colour
    /// in `push_lights` so the disc and the ground halo always
    /// share the same chroma.
    pub kind: PortalKind,
}

/// Push a hot-crimson point light at every active portal so the
/// rift's emissive disc actually paints the surrounding sand,
/// chest, and player. Called every frame *after* the torch
/// system has rebuilt `point_lights`, so the portal lights
/// always survive that vec's per-frame clear.
///
/// Light parameters:
///
///   * **Position**: portal centre (`portal.position + Y * PORTAL_CENTRE_Y`).
///     Lifted to disc-centre height so the falloff illuminates
///     the floor *around* the portal pillar rather than only
///     directly underneath it.
///   * **Color**: deep crimson, HDR-boosted hard so it
///     actually paints over the sandstorm hub's warm
///     ambient fill. Matches the rift's emissive palette so
///     light spilling onto the chest / player reads as "lit
///     by the rift" rather than a decorative sconce.
///   * **Radius**: 16 m. Reaches well past the portal pillar
///     base so the surrounding sand picks up a visible halo,
///     not just the immediate dais.
///   * **Intensity**: synced with the shader's breathing pulse
///     and intermittent destabilisation spasm so the light
///     visibly throbs in lock-step with the visible disc. The
///     rate constants (`0.85` breathing, `0.14` spasm phase,
///     `0.37` tremor phase) mirror the same values inside
///     `assets/shaders/triangle.frag::shadeRift`.
///
/// The 8-light renderer cap means this function is the only
/// place that should push portal lights — having two portals
/// active (e.g. exit + hub-return) costs 2 of the 8 slots.
pub fn push_lights(renderer: &mut Renderer, portals: &[Option<&HubPortal>], elapsed: f32) {
    use rift_engine::renderer::vfx::presets::environment::portal::PORTAL_CENTRE_Y;

    let t = elapsed;
    // Mirror the shader's pulse maths so light + visuals throb together.
    let breathe = 0.88 + 0.12 * (t * 0.85).sin();
    let spasm_phase = (t * 0.14).fract();
    let spasm = ((spasm_phase / 0.08).clamp(0.0, 1.0))
        .min(1.0 - ((spasm_phase - 0.18) / 0.10).clamp(0.0, 1.0))
        .max(0.0);
    let tremor_phase = (t * 0.37 + 0.21).fract();
    let tremor = ((tremor_phase / 0.05).clamp(0.0, 1.0))
        .min(1.0 - ((tremor_phase - 0.10) / 0.06).clamp(0.0, 1.0))
        .max(0.0);
    let pulse = breathe + spasm * 0.40 + tremor * 0.15;

    for portal in portals.iter().copied().flatten() {
        let pos = portal.position + Vec3::Y * PORTAL_CENTRE_Y;
        // Per-kind chroma. The disc bakes the destination
        // sky's palette at spawn time; the per-frame light
        // colour has to match or the player gets a chromatic
        // mismatch ("the disc is cyan but the floor under it
        // is red") that breaks the at-a-glance "deeper vs
        // home" read.
        //
        // All values are HDR-boosted hard. In the rift,
        // shadow-casting torches each push ~10 W of warm
        // orange across a 7–10 m radius and the renderer's
        // 8-light cap means the *first* lights pushed are
        // the shadow-casters — i.e. the torches usually win
        // the slot lottery before the portal light is
        // pushed (`push_lights` runs after the torch
        // system). Pushing the portal at a chromatic value
        // that bloom-blooms past `1.0` on every channel
        // ensures the portal still reads as the dominant
        // local light source even when it's competing with
        // four torch sconces around the boss room.
        let (color, radius) = match portal.kind {
            PortalKind::HubEntry | PortalKind::Descend => (
                // Crimson — same chroma family as torches,
                // but pushed harder on red and noticeably
                // hotter on the highlight so it dominates
                // any torch overlap. The faint green / blue
                // headroom keeps bloom from desaturating
                // into pink mush.
                Vec3::new(5.50, 0.65, 0.22),
                16.0,
            ),
            PortalKind::Extract => (
                // Cool cyan-teal — chromatic complement of
                // the descend portal so the two side-by-
                // side in the boss-room corridor are
                // unmistakable. HDR-boosted on G/B; the
                // tiny R component prevents an unnatural
                // pure-cyan cast under the player and reads
                // as "this is a way out, not the rift".
                Vec3::new(0.35, 3.20, 4.80),
                16.0,
            ),
        };
        // Insert at the *front* of `point_lights` so the
        // portal occupies one of the shadow-caster slots
        // (`point_lights[0..N_SHADOW]`) rather than landing
        // in the unshadowed additive tail. Without this the
        // portal's 16 m radius leaks through every wall in
        // its bubble — most visibly as a brief flash on
        // backside walls during the `spasm` pulse spike,
        // which is the "spills through walls for one frame
        // as I approach" symptom.
        //
        // The previous worry about spending a 6-face cube
        // render every frame on a static-vs-static occlusion
        // pair was unfounded: the renderer's per-slot dirty
        // check (`PointShadowSlotState`) hashes the light's
        // pose + every caster within radius. Both portal
        // and walls are static, so after the first frame in
        // a given room the hash matches the cached value and
        // the 6 face renders are skipped entirely. Even when
        // a monster wanders into the radius the staggered
        // every-other-frame refresh keeps the cost bounded.
        //
        // Cost: bumps the *farthest* of the 8 shadow-casting
        // torches (already heavily rank-faded — see
        // `torches::update_lights`'s RANK_FULL=12 / RANK_CAP=14
        // smoothstep) out of the shadow set into the
        // additive tail. That torch was already at near-zero
        // intensity by the time it reached slot 7, so losing
        // its self-shadow is invisible. The portal's
        // wall-occlusion read, meanwhile, is the dominant
        // visual feature of the boss-room corridor.
        renderer.point_lights.insert(
            0,
            rift_engine::PointLight {
                position: pos,
                color,
                radius,
                // Pulse-modulated intensity. Headroom raised so
                // the breathing/spasm peaks read as a visible
                // throb on the ground, not just the ring itself.
                // Bumped from 8 → 11 in the rift specifically so
                // the portal doesn't get visually drowned by
                // adjacent torches' warm orange spill.
                intensity: 11.0 * pulse,
            },
        );
    }
}

/// Spawn the hub entry portal mesh + vortex emitter at `pos`.
/// Records the render slots in `*hub_portal` so we can spin the
/// mesh and check the interaction radius each frame.
///
/// The portal disc bakes in the *destination* biome's sky
/// palette — a hub portal opens onto the rift, so we feed in
/// [`SkyConfig::rift`] (crimson zenith, near-black horizon).
pub fn spawn_hub(hub_portal: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3) {
    spawn(
        hub_portal,
        renderer,
        pos,
        "hub portal",
        rift_engine::SkyConfig::rift(),
        PortalKind::HubEntry,
    );
}

/// Spawn the exit portal mesh + vortex emitter at `pos`.
/// Same body as `spawn_hub` but writes to `*exit_portal` so the
/// two portals can coexist.
///
/// The exit portal opens onto the *next* rift floor, but for
/// now the visual destination is "more rift" — same crimson
/// gloom palette as `spawn_hub`. (When per-floor biomes ship
/// the caller can pick a different `SkyConfig` here.)
pub fn spawn_exit(exit_portal: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3) {
    spawn(
        exit_portal,
        renderer,
        pos,
        "exit portal",
        rift_engine::SkyConfig::rift(),
        PortalKind::Descend,
    );
}

/// Spawn a portal whose disc shows the *hub* biome — used by the
/// boss-room success portal that ferries you back to the safe
/// zone, as opposed to the deeper-into-the-rift continuation.
/// Currently unused but kept as a small public hook so the
/// callsite that wants "portal home" reads as such.
pub fn spawn_return_to_hub(slot: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3) {
    spawn(
        slot,
        renderer,
        pos,
        "hub return portal",
        rift_engine::SkyConfig::meadow(),
        PortalKind::Extract,
    );
}

fn spawn(
    slot: &mut Option<HubPortal>,
    renderer: &mut Renderer,
    pos: Vec3,
    label: &str,
    destination: rift_engine::SkyConfig,
    kind: PortalKind,
) {
    use rift_engine::renderer::mesh::Mesh;

    let portal_mesh = Mesh::portal_with_palette(
        Vec3::from(destination.zenith),
        Vec3::from(destination.horizon),
        Vec3::from(destination.ground),
    );
    if renderer
        .add_mesh(&portal_mesh, Mat4::from_translation(pos))
        .is_err()
    {
        log::error!("failed to add {label} mesh");
        return;
    }
    let obj_idx = renderer.objects.len() - 1;
    // Flag the portal object as a "rift" surface (bit 1 of
    // material_params.z). The forward shader's `shadeRift`
    // branch synthesises the entire dimensional-tear look
    // procedurally from polar UVs + time, so vertex colors
    // and lighting are bypassed for portal pixels. See
    // `assets/shaders/triangle.frag` and
    // `Mesh::portal_with_palette` for the full contract.
    //
    // Bit 2 (extract portal) tells `shadeRift` to swap its
    // crimson palette for a cool cyan/teal so the descend
    // and extract portals standing side-by-side in the
    // boss-room corridor read as opposite signals at a
    // glance: red = deeper, cyan = home.
    let mut flag_bits: u32 = 2; // bit 1: rift shading
    if matches!(kind, PortalKind::Extract) {
        flag_bits |= 4; // bit 2: extract recolour
    }
    let rift_flags = f32::from_bits(flag_bits);
    renderer.set_object_material_params(obj_idx, [1.0, 0.0, rift_flags, 0.0]);
    // Opt the disc out of every shadow pass. The portal
    // light (pushed each frame by `push_lights`) sits at
    // disc centre, so leaving the disc as a shadow caster
    // makes it occlude its own light from below — the floor
    // directly under the portal would render dark instead of
    // glowing. The disc also sits inside the cube atlas
    // slot's near-plane on most faces, so its self-shadow
    // would be a near-uniform black wherever the disc's
    // back-face faces the cube face origin.
    renderer.set_object_casts_shadow(obj_idx, false);
    // Anchor the VFX at the *centre* of the mesh ring, not at
    // floor level — the Strange-style halo orbits a vertical
    // axis, so the emitter has to sit where the visible ring
    // is. See `PORTAL_CENTRE_Y` for the offset constant.
    let emitter_pos =
        pos + Vec3::Y * rift_engine::renderer::vfx::presets::environment::portal::PORTAL_CENTRE_Y;
    let emitter_id = renderer.vfx_system.spawn(
        rift_engine::renderer::vfx::presets::portal_vortex(),
        emitter_pos,
    );
    *slot = Some(HubPortal {
        position: pos,
        obj_idx,
        emitter_idx: emitter_id,
        audio: None,
        age: 0.0,
        kind,
    });
}

/// Spawn a looping `ambient/portal_hum.wav` emitter for
/// `portal` and stash its id on the struct. Idempotent: if the
/// portal already has an emitter, the previous one is freed
/// first so re-attaching can't leak. No-op when the asset is
/// missing or the audio system rejects the spawn.
///
/// Falloff is wider than torches (2 m full → 24 m silent) so
/// the hum is audible from across the hub platform but still
/// localises to the portal pillar when the player gets close.
pub fn attach_audio(portal: &mut HubPortal, audio: &mut rift_audio::AudioSystem) {
    if let Some(prev) = portal.audio.take() {
        audio.despawn_emitter(prev);
    }
    let spec = rift_audio::SoundSpec {
        path: "ambient/portal_hum.wav".into(),
        // Held at moderate volume so per-frame pulse
        // modulation in `tick_audio` can dip below it
        // without sounding like the hum cuts out.
        volume: 0.55,
        min_distance: 6.0,
        max_distance: 55.0,
        looping: true,
    };
    use rift_engine::renderer::vfx::presets::environment::portal::PORTAL_CENTRE_Y;
    let emitter_pos = portal.position + Vec3::Y * PORTAL_CENTRE_Y;
    portal.audio = audio.spawn_emitter(&spec, emitter_pos);
}

/// Despawn this portal's looping audio emitter (if any). Call
/// from the floor-teardown path BEFORE the portal struct is
/// dropped so the emitter slot recycles cleanly.
pub fn detach_audio(portal: &mut HubPortal, audio: &mut rift_audio::AudioSystem) {
    if let Some(em) = portal.audio.take() {
        audio.despawn_emitter(em);
    }
}

/// Per-frame: drive each portal's hum volume from the same
/// breathing/spasm pulse that modulates the portal light
/// (see [`push_lights`]). Phase-locked by construction — both
/// derivations read the same `elapsed` and use identical
/// constants — so the audio swell tracks the visible disc
/// throb exactly.
///
/// Also lazily attaches the looping hum to any portal that
/// doesn't have one yet — this is the single integration
/// point for portal audio so the lazy-spawn paths
/// (`tick_exit`, `tick_rift_spawn`) don't have to thread
/// `&mut AudioSystem` through their already-busy signatures.
pub fn tick_audio(
    portals: &mut [Option<&mut HubPortal>],
    audio: &mut rift_audio::AudioSystem,
    elapsed: f32,
) {
    let t = elapsed;
    // Mirrors the maths in `push_lights` so the hum and the
    // light pulse together.
    let breathe = 0.88 + 0.12 * (t * 0.85).sin();
    let spasm_phase = (t * 0.14).fract();
    let spasm = ((spasm_phase / 0.08).clamp(0.0, 1.0))
        .min(1.0 - ((spasm_phase - 0.18) / 0.10).clamp(0.0, 1.0))
        .max(0.0);
    let tremor_phase = (t * 0.37 + 0.21).fract();
    let tremor = ((tremor_phase / 0.05).clamp(0.0, 1.0))
        .min(1.0 - ((tremor_phase - 0.10) / 0.06).clamp(0.0, 1.0))
        .max(0.0);
    // Visual pulse goes ~1.0..1.55. Map to a tighter audio
    // band (~0.85..1.20) so the hum throbs audibly without
    // the spasm spikes shouting over the rest of the
    // soundscape.
    let pulse_v = breathe + spasm * 0.40 + tremor * 0.15;
    let volume = 0.85 + (pulse_v - 1.0).clamp(0.0, 0.6) * 0.6;

    for portal in portals.iter_mut().flatten() {
        if portal.audio.is_none() {
            attach_audio(portal, audio);
        }
        let Some(em) = portal.audio else { continue };
        // Keep the source pinned at the disc centre — portals
        // don't move, but `set_emitter_position` is cheap and
        // ensures the spatial track stays put even if a
        // future system mutates `portal.position`.
        use rift_engine::renderer::vfx::presets::environment::portal::PORTAL_CENTRE_Y;
        audio.set_emitter_position(em, portal.position + Vec3::Y * PORTAL_CENTRE_Y);
        audio.set_emitter_volume(em, volume);
    }
}

/// Per-frame hub-portal tick: spin the mesh and let the local
/// player walk up + press F to enqueue an `EnterRift` request.
pub fn tick_hub(
    hub_portal: &mut Option<HubPortal>,
    world: &hecs::World,
    renderer: &mut Renderer,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    dt: f32,
) {
    tick(
        hub_portal,
        world,
        renderer,
        input,
        net,
        hud_prompt,
        dt,
        "PRESS [F] TO ENTER THE RIFT",
        "hub portal",
        true,
    );
}

/// Per-frame exit-portal tick. Lazily spawns the portal the
/// first frame `floor_complete` is true and we're on a rift
/// floor (not the hub), spins the mesh thereafter, and converts
/// an F-press inside the interact radius into an `EnterRift`
/// request — the server's `RequestEnterRift` handler reads that
/// as "advance one floor" because `floor_index != 0`.
///
/// `anchor` is the world position pre-baked by the dungeon
/// generator's portal-room — typically one of the two
/// `Floor::portal_anchors` slots. Spawning at a pre-validated
/// floor tile guarantees the portal can't end up inside a
/// wall or clipping into a neighbouring room, both of which
/// the previous offset-from-boss-centre math could produce on
/// awkward BSP layouts.
pub fn tick_exit(
    exit_portal: &mut Option<HubPortal>,
    world: &hecs::World,
    renderer: &mut Renderer,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    descend_prompt: &mut bool,
    floor_complete: bool,
    in_hub: bool,
    anchor: Vec3,
    vote_active: bool,
    vote_cooldown: f32,
    dt: f32,
) {
    // Spawn lazily on first qualifying frame.
    if floor_complete && !in_hub && exit_portal.is_none() {
        // The +Y 0.5 keeps the ring from z-fighting the
        // floor decal; the anchor itself sits inside the
        // portal room so we don't add any lateral offset.
        let pos = anchor + Vec3::new(0.0, 0.5, 0.0);
        log::info!("exit portal: spawning at {:?}", pos);
        spawn_exit(exit_portal, renderer, pos);
    }
    // Track in-range for the difficulty tooltip even when the
    // F-press / cooldown banner paths short-circuit below.
    if let Some(portal) = exit_portal.as_ref() {
        let in_range = world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.position)
            .next()
            .map(|p| p.distance(portal.position) <= INTERACT_RADIUS)
            .unwrap_or(false);
        if in_range {
            *descend_prompt = true;
        }
    }
    // While a descend / exit vote is open the HUD vote panel
    // owns the prompt slot and the F-press is reserved for
    // Y/N — so suppress both here. Likewise during cooldown,
    // surface the cooldown banner instead of the F-press
    // prompt so the player understands why F doesn't work.
    if vote_active {
        // Still re-orient the disc toward the player below.
        if let Some(portal) = exit_portal.as_mut() {
            portal.age += dt;
        }
        if let Some(portal) = exit_portal.as_ref() {
            apply_facing(portal, world, renderer);
        }
        return;
    }
    if vote_cooldown > 0.0 {
        if let Some(portal) = exit_portal.as_mut() {
            portal.age += dt;
        }
        if let Some(portal) = exit_portal.as_ref() {
            apply_facing(portal, world, renderer);
            let player_in_range = world
                .query::<(&Transform, &Player, &LocalPlayer)>()
                .iter()
                .map(|(_, (t, _, _))| t.position)
                .next()
                .map(|p| p.distance(portal.position) <= INTERACT_RADIUS)
                .unwrap_or(false);
            if player_in_range {
                *hud_prompt = Some("VOTE COOLDOWN ACTIVE");
            }
        }
        return;
    }
    tick(
        exit_portal,
        world,
        renderer,
        input,
        net,
        hud_prompt,
        dt,
        "PRESS [F] TO DESCEND",
        "exit portal",
        false,
    );
}

/// Per-frame rift-exit-portal tick. Lazily spawns the portal
/// the first frame the floor's boss is dead and we're not in
/// the hub — i.e. the same gate the descend portal uses, so
/// the two appear together in the boss room. Spinning the
/// mesh thereafter, and converts an F-press inside the
/// interact radius into a `pending_exit_vote_start` flag that
/// the binary forwards as `ClientMsg::RiftExitVoteStart`.
/// While a vote is already active the prompt is suppressed
/// and the F-press is ignored — the player should be using
/// Y/N. Ghosts (risen-but-dead spectators) get no prompt and
/// the F press is ignored: gatekeeping the living team out of
/// an exit by spamming votes would be too easy otherwise.
///
/// `anchor` is the world position pre-baked by the dungeon
/// generator's portal-room — the second of the two
/// `Floor::portal_anchors` slots, sibling to the descend
/// portal's anchor. Both anchors live inside the dedicated
/// portal room, a corridor away from the boss room, so the
/// player physically walks from the boss kill into a separate
/// chamber to choose descend vs return-to-hub.
pub fn tick_rift_spawn(
    portal_slot: &mut Option<HubPortal>,
    world: &hecs::World,
    renderer: &mut Renderer,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    floor_complete: bool,
    in_hub: bool,
    anchor: Vec3,
    vote_active: bool,
    cooldown_remaining: f32,
    is_ghost: bool,
    dt: f32,
) {
    use winit::keyboard::KeyCode;

    if in_hub {
        // Despawn isn't worth the cleanup churn — we just stop
        // ticking. The renderer slot will be cleared when the
        // next floor regen wipes objects wholesale.
        return;
    }
    if portal_slot.is_none() {
        // Gate the exit portal on boss death so the player
        // can't bail out of a floor they haven't cleared.
        // Once the boss is down, this portal is the
        // "leave with loot" half of the boss-room choice;
        // the descend portal sits next to it inside the
        // portal room.
        if !floor_complete {
            return;
        }
        let pos = anchor + Vec3::new(0.0, 0.5, 0.0);
        log::info!("rift exit portal: spawning at {:?}", pos);
        // Stepping into this portal returns the party to the
        // hub, so the disc bakes the *meadow* palette — bright
        // cyan / warm horizon. Reads as a doorway home, in
        // visual contrast to the crimson hub-entry portal.
        spawn(
            portal_slot,
            renderer,
            pos,
            "rift exit portal",
            rift_engine::SkyConfig::meadow(),
            PortalKind::Extract,
        );
    }

    let Some(portal) = portal_slot.as_mut() else {
        return;
    };
    portal.age += dt;
    apply_facing(portal, world, renderer);

    let Some(player_pos) = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()
    else {
        return;
    };
    if player_pos.distance(portal.position) > INTERACT_RADIUS {
        return;
    }
    if is_ghost {
        // Ghosts don't get a prompt and can't open a vote. We
        // still tick the spin animation above so the portal
        // visual stays alive in their spectator view.
        return;
    }
    if vote_active {
        // HUD vote panel owns the prompt while a vote is in
        // flight — keep this slot quiet so the two prompts
        // don't fight for the same on-screen line.
        return;
    }
    if cooldown_remaining > 0.0 {
        // Surface a temporary "wait N s" prompt instead of the
        // F-press one so the player understands why F doesn't
        // work. The HUD prompt is `&'static str` today; we
        // round + use a small set of pre-baked literals to
        // avoid plumbing dynamic strings through the prompt
        // system just for this.
        *hud_prompt = Some("VOTE COOLDOWN ACTIVE");
        return;
    }
    *hud_prompt = Some("PRESS [F] TO LEAVE THE RIFT");
    if input.key_just_pressed(KeyCode::KeyF) && !net.pending_exit_vote_start {
        log::info!("rift spawn portal: requesting RiftExitVoteStart");
        net.pending_exit_vote_start = true;
    }
}

fn tick(
    portal_slot: &mut Option<HubPortal>,
    world: &hecs::World,
    renderer: &mut Renderer,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    dt: f32,
    prompt_text: &'static str,
    log_label: &str,
    is_hub_portal: bool,
) {
    use winit::keyboard::KeyCode;

    let Some(portal) = portal_slot.as_mut() else {
        return;
    };
    let _ = portal.emitter_idx;
    portal.age += dt;
    apply_facing(portal, world, renderer);

    let Some(player_pos) = world
        .query::<(&Transform, &Player, &LocalPlayer)>()
        .iter()
        .map(|(_, (t, _, _))| t.position)
        .next()
    else {
        return;
    };
    if player_pos.distance(portal.position) <= INTERACT_RADIUS {
        *hud_prompt = Some(prompt_text);
        if input.key_just_pressed(KeyCode::KeyF) {
            if is_hub_portal {
                // Hub portal: defer to the portal modal so the
                // player picks Solo / Party / Matchmade and a
                // start floor before the proposal goes out.
                if !net.pending_open_portal_modal {
                    log::info!("{log_label}: opening portal modal");
                    net.pending_open_portal_modal = true;
                }
            } else if net.transition.is_none() {
                // Exit portal: legacy direct-descend path. The
                // server treats `RequestEnterRift` from inside
                // a rift as "open the descend vote".
                log::info!("{log_label}: requesting EnterRift");
                net.transition = Some(NetTransitionRequest::EnterRift);
            }
        }
    }
}
