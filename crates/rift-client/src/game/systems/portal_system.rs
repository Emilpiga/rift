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

use crate::game::sub_state::{NetState, NetTransitionRequest};

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
        // Still spin the mesh below.
        if let Some(portal) = exit_portal.as_mut() {
            portal.age += dt;
            if let Some(obj) = renderer.objects.get_mut(portal.obj_idx) {
                obj.model_matrix = Mat4::from_translation(portal.position)
                    * Mat4::from_rotation_y(portal.age * 0.6);
            }
        }
        return;
    }
    if vote_cooldown > 0.0 {
        if let Some(portal) = exit_portal.as_mut() {
            portal.age += dt;
            if let Some(obj) = renderer.objects.get_mut(portal.obj_idx) {
                obj.model_matrix = Mat4::from_translation(portal.position)
                    * Mat4::from_rotation_y(portal.age * 0.6);
            }
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
        spawn(portal_slot, renderer, pos, "rift spawn portal");
    }

    let Some(portal) = portal_slot.as_mut() else { return };
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
