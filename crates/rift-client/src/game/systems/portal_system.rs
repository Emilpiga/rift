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
fn apply_facing(
    portal: &HubPortal,
    world: &hecs::World,
    renderer: &mut Renderer,
) {
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

/// Visual + interaction state for a single portal (hub entry or
/// boss-room exit). The two are structurally identical; we keep
/// them as separate `Option<HubPortal>` fields on `GameState` so
/// they can coexist (boss-room exit + hypothetical hub return).
pub struct HubPortal {
    /// World-space position of the portal centre.
    pub position: Vec3,
    /// Render-object index of the portal mesh in
    /// `renderer.objects`. We mutate `model_matrix` here every
    /// frame to spin it.
    pub obj_idx: usize,
    /// Particle emitter index for the swirling vortex.
    pub emitter_idx: rift_engine::renderer::vfx::EffectId,
    /// Seconds since the portal was spawned. Drives rotation.
    pub age: f32,
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
    );
}

fn spawn(
    slot: &mut Option<HubPortal>,
    renderer: &mut Renderer,
    pos: Vec3,
    label: &str,
    destination: rift_engine::SkyConfig,
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
    // Anchor the VFX at the *centre* of the mesh ring, not at
    // floor level — the Strange-style halo orbits a vertical
    // axis, so the emitter has to sit where the visible ring
    // is. See `PORTAL_CENTRE_Y` for the offset constant.
    let emitter_pos = pos
        + Vec3::Y * rift_engine::renderer::vfx::presets::environment::portal::PORTAL_CENTRE_Y;
    let emitter_id = renderer
        .vfx_system
        .spawn(rift_engine::renderer::vfx::presets::portal_vortex(), emitter_pos);
    *slot = Some(HubPortal {
        position: pos,
        obj_idx,
        emitter_idx: emitter_id,
        age: 0.0,
    });
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
    boss_room_center: Vec3,
    vote_active: bool,
    vote_cooldown: f32,
    dt: f32,
) {
    // Spawn lazily on first qualifying frame.
    if floor_complete && !in_hub && exit_portal.is_none() {
        // Sit slightly above ground so the ring isn't z-fought
        // by the floor decal — mirrors the hub portal's
        // `+ Y 0.5` offset.
        let pos = boss_room_center + Vec3::new(0.0, 0.5, 0.0);
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

/// Per-frame rift-spawn-portal tick. Lazily spawns the portal
/// the first frame we're on a rift floor (i.e. `!in_hub`),
/// spins the mesh thereafter, and converts an F-press inside
/// the interact radius into a `pending_exit_vote_start` flag
/// that the binary forwards as `ClientMsg::RiftExitVoteStart`.
/// While a vote is already active the prompt is suppressed and
/// the F-press is ignored — the player should be using Y/N.
/// Ghosts (risen-but-dead spectators) get no prompt and the F
/// press is ignored: gatekeeping the living team out of an
/// exit by spamming votes would be too easy otherwise.
pub fn tick_rift_spawn(
    portal_slot: &mut Option<HubPortal>,
    world: &hecs::World,
    renderer: &mut Renderer,
    input: &Input,
    net: &mut NetState,
    hud_prompt: &mut Option<&'static str>,
    in_hub: bool,
    spawn_pos: Vec3,
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
        // Sit slightly above ground so the ring isn't z-fought
        // by the floor decal.
        let pos = spawn_pos + Vec3::new(0.0, 0.5, 0.0);
        log::info!("rift spawn portal: spawning at {:?}", pos);
        // Stepping into this portal returns the party to the
        // hub, so the disc bakes the *meadow* palette — bright
        // cyan / warm horizon. Reads as a doorway home, in
        // visual contrast to the crimson hub-entry portal.
        spawn(
            portal_slot,
            renderer,
            pos,
            "rift spawn portal",
            rift_engine::SkyConfig::meadow(),
        );
    }

    let Some(portal) = portal_slot.as_mut() else { return };
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

    let Some(portal) = portal_slot.as_mut() else { return };
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
