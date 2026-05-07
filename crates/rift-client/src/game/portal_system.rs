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

use glam::{Mat4, Vec3};
use rift_engine::ecs::components::{LocalPlayer, Player, Transform};
use rift_engine::{Input, Renderer};

use super::sub_state::{NetState, NetTransitionRequest};

/// Walk-to-interact range. Within this distance the player gets
/// a press-F prompt and the F key triggers an `EnterRift`
/// request. Used for both the hub entry portal and the boss-room
/// exit portal.
pub const INTERACT_RADIUS: f32 = 2.2;

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
pub fn spawn_hub(hub_portal: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3) {
    spawn(hub_portal, renderer, pos, "hub portal");
}

/// Spawn the exit portal mesh + vortex emitter at `pos`.
/// Same body as `spawn_hub` but writes to `*exit_portal` so the
/// two portals can coexist.
pub fn spawn_exit(exit_portal: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3) {
    spawn(exit_portal, renderer, pos, "exit portal");
}

fn spawn(slot: &mut Option<HubPortal>, renderer: &mut Renderer, pos: Vec3, label: &str) {
    use rift_engine::renderer::mesh::Mesh;

    let portal_mesh = Mesh::portal();
    if renderer
        .add_mesh(&portal_mesh, Mat4::from_translation(pos))
        .is_err()
    {
        log::error!("failed to add {label} mesh");
        return;
    }
    let obj_idx = renderer.objects.len() - 1;
    let emitter_id = renderer
        .vfx_system
        .spawn(rift_engine::renderer::vfx::presets::portal_vortex(), pos);
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
    floor_complete: bool,
    in_hub: bool,
    boss_room_center: Vec3,
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
    );
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
) {
    use winit::keyboard::KeyCode;

    let Some(portal) = portal_slot.as_mut() else { return };
    let _ = portal.emitter_idx;
    portal.age += dt;
    if let Some(obj) = renderer.objects.get_mut(portal.obj_idx) {
        obj.model_matrix = Mat4::from_translation(portal.position)
            * Mat4::from_rotation_y(portal.age * 0.6);
    }

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
        if input.key_just_pressed(KeyCode::KeyF) && net.transition.is_none() {
            log::info!("{log_label}: requesting EnterRift");
            net.transition = Some(NetTransitionRequest::EnterRift);
        }
    }
}
