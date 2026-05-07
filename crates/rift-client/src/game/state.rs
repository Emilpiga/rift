use glam::Vec3;
use rift_game::abilities::{Ability, TargetingMode};
use rift_engine::ecs::components::{Health, LocalPlayer, Player, Transform};
use rift_engine::ui::CombatTextSystem;
use rift_engine::renderer::decals::DecalSystem;
use rift_engine::ecs::systems::{
    camera_follow_system, cast_advance_system, collision_system, despawn_system,
    enemy_anim_system, locomotion_anim_system, movement_system, player_action_post_system,
    player_action_pre_system, player_input_system, render_sync_system, skinning_system,
    PlayerActionConfig,
};
use rift_engine::{Input, LoadStatus, Renderer};

use rift_game::character;
use rift_game::monsters;
use super::character_select;
use super::character_spawn;
use super::floor::FloorManager;
use super::hud;
use super::loot_system;
use super::monster_assets::load_role;
use super::mp_inventory_ui;
use super::player_state::PlayerState;
use super::portal_system::{self, HubPortal};
use super::rift_state::RiftState;
use super::stash_system;
use super::spellbook;
pub use super::sub_state::*;

/// Top-level game state — the single struct that orchestrates all
/// rendering / input / UI. Authoritative gameplay (enemies, hits,
/// loot, transitions) lives in `rift-server`.
///
/// Multiplayer / loot / channel / loading concerns are split into
/// the sub-structs below to keep this header readable. Internal
/// methods reach through `self.net.*`, `self.loot.*`, etc.; the
/// client binary (`main.rs`) does the same so the contract is
/// uniform across the crate boundary.
pub struct GameState {
    pub world: hecs::World,
    pub rift: RiftState,
    pub player_state: PlayerState,
    pub floor_mgr: FloorManager,
    /// New multiplayer inventory panel — operates on
    /// [`LootClientState::mp_inventory`] (the server-mirrored bag)
    /// instead of the legacy engine `Inventory`. Owns the Tab
    /// toggle now.
    pub mp_inventory_ui: mp_inventory_ui::MpInventoryUI,
    pub combat_text: CombatTextSystem,
    /// Cross-frame immediate-mode UI state — owns focus, hover,
    /// drag, and the modal stack. Borrowed by `Ui::begin` once
    /// per frame; widgets in [`rift_engine::ui::im`] thread it
    /// transparently. Landing 1 is scaffolding-only; subsequent
    /// landings migrate the bespoke panels onto it.
    pub ui_state: rift_engine::ui::im::UiState,
    pub decals: DecalSystem,
    needs_new_floor: bool,
    /// Cached wall colliders for physics (rebuilt on floor change).
    wall_colliders: Vec<(Vec3, rift_engine::ecs::components::Collider)>,
    /// Cached wall AABBs for raycasting (rebuilt on floor change).
    pub wall_aabbs: Vec<rift_engine::physics::Aabb>,
    /// Active placed-ability targeting (if any). Pure visual / input
    /// state — the actual cast is sent to the server.
    targeting: Option<PlacedTargeting>,
    /// Eases from 1 -> 0 over ~0.5 s after the player takes damage.
    damage_flash: f32,
    /// Eases from 1 -> 0 over ~2.5 s after a level-up. Drives a
    /// HUD banner overlay.
    pub level_up_flash: f32,
    /// Black-screen alpha used for hub ↔ rift transitions and
    /// for the post-death respawn fade. Pinned to 1.0 by
    /// [`Self::apply_net_transition`] (and locally when a death
    /// kicks in) and decayed back to 0 over ~0.6 s each frame so
    /// the world fades in cleanly after the regeneration stall.
    transition_fade: f32,
    /// Local player's HP last frame, used to detect damage events
    /// (for the hit-react one-shot) and the alive→dead edge that
    /// triggers the death animation. `None` until the first
    /// frame the local player exists in the world.
    prev_player_hp: Option<f32>,
    /// Edge-detector mirror of `local_ghost_cached`, used to
    /// fire `trigger_player_rise` exactly once on the down-pose
    /// → ghost transition. Cleared on regen / respawn.
    prev_local_ghost: bool,
    /// `Some(text)` if the local player is standing in an
    /// interaction range this frame and the HUD should show a
    /// press-F prompt. Set during `tick_*_portal` and the stash
    /// chest tick, consumed and cleared during the HUD pass.
    hud_prompt: Option<&'static str>,
    /// True while the player is in the safe hub zone.
    in_hub: bool,
    /// Glowing entry portal placed in the hub.
    hub_portal: Option<HubPortal>,
    /// Glowing exit portal that appears in the boss room after
    /// the floor's boss dies. Same chrome as `hub_portal` but
    /// triggers `NetTransitionRequest::EnterRift` (which the
    /// server interprets as "advance one floor" once we're not
    /// in the hub).
    exit_portal: Option<HubPortal>,
    /// Always-present portal at the rift floor's spawn point.
    /// Pressing F here opens the rift exit vote (or, solo,
    /// instantly transitions to the hub). Spawned lazily when a
    /// rift floor is generated; cleared on hub return.
    rift_spawn_portal: Option<HubPortal>,
    /// Latest authoritative rift exit vote snapshot from the
    /// server. `None` means we've never received one (typical
    /// at session start before any vote happens). When `active`
    /// is true the HUD vote panel renders the countdown +
    /// voter roll.
    pub exit_vote: Option<rift_net::messages::VoteState>,

    /// Per-frame staged init progress (icons, monsters).
    pub loading: LoadingState,
    /// Outbound / inbound traffic the binary forwards to / receives
    /// from the server. Drained every frame.
    pub net: NetState,
    /// Locally-tracked channel state (active hold, beam visuals).
    pub channel: ChannelState,
    /// Server-mirrored loot visuals, pickup queue, and inventory.
    pub loot: LootClientState,

    /// Top-level state (character-select vs playing).
    app_state: AppState,
    /// Owns the character-select screen UI + preview avatar.
    character_select: character_select::CharacterSelect,
    /// Spellbook overlay state (open/closed + selected ability).
    /// Toggled with `B`; mutates the loadout via
    /// `request_set_loadout_slot` and waits for the server to
    /// echo the new bar back through `ServerMsg::Loadout`.
    pub spellbook: spellbook::SpellbookUi,
    /// Shared cache of bound player-skeleton animation sets, keyed by
    /// gender. Populated lazily on first spawn (local or remote).
    pub anim_cache: character_spawn::AnimLibraryCache,
}

/// Hub entry portal. Visual + interaction state for the glowing ring
/// the player walks into to start a rift run. Lives in
/// `portal_system::HubPortal` now; this re-export keeps the
/// existing `Option<HubPortal>` field declarations below wired
/// up without extra path noise.
// (HubPortal type imported above from portal_system.)

/// Active placed-ability targeting state (player is choosing where to place an AoE).
struct PlacedTargeting {
    /// Which ability slot triggered this.
    slot_index: usize,
    /// The ability being placed (cloned).
    ability: Ability,
    /// Radius of the AoE indicator circle.
    radius: f32,
    /// Render object index for the ground indicator mesh.
    indicator_obj: Option<usize>,
}

/// Top-level app state.
#[derive(Clone, Debug, PartialEq, Eq)]
enum AppState {
    /// Showing the roster / create / delete screen.
    CharacterSelect,
    /// User picked Play. Run heavy world setup one chunk per frame.
    EnteringWorld(EnterPhase),
    /// Player is in-game (hub or rift).
    Playing,
}

/// One step of the character-select → in-game transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EnterPhase {
    PrepareScene,
    PreloadHub,
    GenerateHub,
    AttachOutfits,
    LoadOutfits,
    RebuildWalls,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            rift: RiftState::new(1),
            player_state: PlayerState::new(),
            floor_mgr: FloorManager::new(),
            mp_inventory_ui: mp_inventory_ui::MpInventoryUI::new(),
            combat_text: CombatTextSystem::new(),
            ui_state: rift_engine::ui::im::UiState::new(),
            decals: DecalSystem::new(),
            needs_new_floor: false,
            wall_colliders: Vec::new(),
            wall_aabbs: Vec::new(),
            targeting: None,
            damage_flash: 0.0,
            level_up_flash: 0.0,
            transition_fade: 0.0,
            prev_player_hp: None,
            prev_local_ghost: false,
            hud_prompt: None,
            in_hub: true,
            hub_portal: None,
            exit_portal: None,
            rift_spawn_portal: None,
            exit_vote: None,
            loading: LoadingState::default(),
            net: NetState::default(),
            channel: ChannelState::default(),
            loot: LootClientState::default(),
            app_state: AppState::CharacterSelect,
            character_select: character_select::CharacterSelect::new(),
            anim_cache: character_spawn::AnimLibraryCache::new(),
            spellbook: spellbook::SpellbookUi::new(),
        }
    }

    /// Drive one stage of staged initialization.
    pub fn load_step(&mut self, renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        let monster_total = monsters::ALL_ROLES.len();
        let icon_total = renderer.total_icons();
        // Combined progress denominator: every icon counts as
        // one step, every monster role as one step. Avoids a
        // divide-by-zero when there are no icons at all.
        let total_steps = (icon_total + monster_total).max(1) as f32;

        let label = match self.loading.phase {
            LoadPhase::Icons => {
                // Decode + upload a generous batch per call. All
                // icons in a single step share one staging buffer
                // and one command-buffer submit, and the decode
                // pass runs in parallel across CPU cores via
                // rayon, so a large budget mostly costs us a
                // single multi-core stall — the loading screen
                // still pumps frames between batches.
                let (loaded, total) = renderer.step_load_icons(128)?;
                if loaded >= total {
                    self.loading.phase = LoadPhase::Monsters;
                }
                format!("Loading icons ({loaded}/{total})")
            }
            LoadPhase::Monsters => {
                let role = monsters::ALL_ROLES[self.loading.monster_index];
                let asset = load_role(role);
                *self.floor_mgr.monsters.slot_mut(role) = asset;
                self.loading.monster_index += 1;
                if self.loading.monster_index >= monsters::ALL_ROLES.len() {
                    self.loading.phase = LoadPhase::Done;
                }
                format!("Loading monster: {:?}", role)
            }
            LoadPhase::Done => return Ok(LoadStatus::Done),
        };

        let done_after = match self.loading.phase {
            LoadPhase::Icons => renderer.loaded_icons() as f32,
            LoadPhase::Monsters => (icon_total + self.loading.monster_index) as f32,
            LoadPhase::Done => total_steps,
        };
        let progress = (done_after / total_steps).min(1.0);

        if matches!(self.loading.phase, LoadPhase::Done) {
            Ok(LoadStatus::Done)
        } else {
            Ok(LoadStatus::Loading { progress, label })
        }
    }


    fn rebuild_wall_caches(&mut self) {
        use rift_engine::ecs::components::{Collider, Static};
        use rift_engine::physics::Aabb;

        self.wall_colliders = self.world
            .query::<(&Transform, &Collider, &Static)>()
            .iter()
            .map(|(_, (t, c, _))| (t.position, *c))
            .collect();

        self.wall_aabbs = self.wall_colliders
            .iter()
            .map(|(pos, col)| Aabb::from_center(*pos, col.half_extents))
            .collect();
    }

    fn reset_for_regeneration(&mut self, renderer: &mut Renderer) {
        self.prev_player_hp = None;
        self.prev_local_ghost = false;
        self.hud_prompt = None;
        self.damage_flash = 0.0;
        self.level_up_flash = 0.0;
        self.targeting = None;
        self.hub_portal = None;
        self.exit_portal = None;
        self.rift_spawn_portal = None;
        // Exit-vote state is not cleared on regen: the server
        // re-broadcasts the authoritative `RiftExitVote` whenever
        // we land on a fresh floor (cooldown wipe → dirty flag
        // → broadcast), and the floor transition itself cancels
        // any in-flight vote on the server side.
        self.decals.clear();
        self.combat_text.clear();
        // Wipe every live particle / ribbon emitter so loot
        // beams, frost trails, channel ribbons, and any other
        // long-lived effect from the previous floor don't leak
        // visuals into the new one.
        renderer.vfx_system.clear_all();
        // The pillar emitters owned by `LootDropVisual`s are
        // already invalidated by the wipe above; just drop the
        // bookkeeping Vec.
        self.loot.drops.clear();
        self.loot.pending_pickups.clear();
        self.loot.claimed_ids.clear();
        // Stash UI must close on transition: the chest only
        // exists in the hub, and a stale "stash open" flag
        // would cause bag clicks to deposit into nothing.
        self.loot.stash_session = false;
        self.mp_inventory_ui.open = false;
        self.loot.stash_items.clear();
    }

    /// Number of occupied bag slots in our local inventory mirror.
    pub fn local_inventory_filled(&self) -> usize {
        loot_system::local_inventory_filled(&self.loot)
    }

    /// Surface an "Inventory full" warning above the local
    /// player. Called by the binary when the server replies with
    /// `PickupRejected::InventoryFull`.
    pub fn warn_inventory_full(&mut self) {
        loot_system::warn_inventory_full(&self.world, &mut self.combat_text);
    }

    /// Closest loot drop inside `loot_system::PICKUP_RADIUS` of
    /// the local player. Used by the HUD pass to render a
    /// "Press F: <item>" tooltip.
    fn nearest_lootable_drop(&self) -> Option<(rift_net::NetId, f32)> {
        loot_system::nearest_drop(&self.world, &self.loot)
    }

    /// Tear down the visual for a loot drop that was claimed.
    /// Shim for [`loot_system::resolve_claim`].
    pub fn resolve_loot_claim(
        &mut self,
        renderer: &mut Renderer,
        loot: rift_net::NetId,
        add_to_local: bool,
    ) {
        loot_system::resolve_claim(&mut self.loot, renderer, loot, add_to_local);
    }

    pub fn shutdown(&mut self, renderer: &mut Renderer) {
        unsafe { renderer.ash_device().device_wait_idle().ok(); }
        let device = renderer.ash_device().clone();
        let allocator = renderer.allocator_arc();
        self.floor_mgr.monsters.cleanup_gpu(&device, &allocator);
        self.floor_mgr.props.cleanup_gpu(&device, &allocator);
        self.floor_mgr.env.cleanup_gpu(&device, &allocator);
    }

    /// Apply a server-driven floor transition.
    pub fn apply_net_transition(&mut self, renderer: &mut Renderer, index: u32) {
        self.reset_for_regeneration(renderer);
        // Pin the fade to fully-black for one frame so the world
        // regenerates behind a curtain. The decay in `update`
        // will fade it back out over ~0.6 s.
        self.transition_fade = 1.0;
        if index == 0 {
            self.in_hub = true;
            self.rift = RiftState::new(1);
            match self.floor_mgr.generate_hub(
                &mut self.world,
                renderer,
                &self.player_state,
                &mut self.anim_cache,
            ) {
                Ok(portal_pos) => portal_system::spawn_hub(&mut self.hub_portal, renderer, portal_pos),
                Err(e) => log::error!("Hub regeneration failed: {}", e),
            }
        } else {
            self.in_hub = false;
            self.rift = RiftState::new(index);
            if let Err(e) = self.floor_mgr.generate(
                &mut self.world,
                renderer,
                &self.rift,
                &self.player_state,
                &mut self.anim_cache,
                self.net.floor_seed,
            ) {
                log::error!("Net floor regeneration failed: {}", e);
            }
        }
        self.rebuild_wall_caches();
    }

    pub fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        match self.app_state.clone() {
            AppState::CharacterSelect => {
                self.update_character_select(renderer, input, dt);
                return;
            }
            AppState::EnteringWorld(phase) => {
                self.tick_entering_world(renderer, phase);
                return;
            }
            AppState::Playing => {}
        }

        // Gameplay → combat → render → UI. Each phase owns a
        // coherent slice of the per-frame work; the `update`
        // header reads as a high-level outline so a future
        // reader doesn't have to scan ~250 lines of mixed
        // input/sim/render/UI code to find anything.
        self.update_gameplay(renderer, input, dt);
        self.update_combat(renderer, input, dt);
        self.update_render(renderer, input, dt);
        self.update_ui(renderer, input);

        // Mark needs_new_floor as consumed (kept for future use,
        // but no SP path sets it any more).
        if self.needs_new_floor {
            self.needs_new_floor = false;
        }
    }

    /// Phase 1 — gameplay input + interaction ticks + ECS sim.
    /// Reads input, advances the per-frame interaction systems
    /// (portals, stash chest, ground loot), then runs the
    /// movement / collision pipeline.
    fn update_gameplay(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        self.rift.timer += if self.in_hub { 0.0 } else { dt };

        // Hub portal: spin the mesh and watch for the local player
        // walking up + pressing F to start a rift run.
        portal_system::tick_hub(
            &mut self.hub_portal,
            &self.world,
            renderer,
            input,
            &mut self.net,
            &mut self.hud_prompt,
            dt,
        );

        // Exit portal: appears in the boss room after the boss
        // dies; F-press advances to the next rift floor.
        portal_system::tick_exit(
            &mut self.exit_portal,
            &self.world,
            renderer,
            input,
            &mut self.net,
            &mut self.hud_prompt,
            self.rift.floor_complete,
            self.in_hub,
            self.floor_mgr.boss_room_center,
            dt,
        );

        // Rift spawn portal: always-present near the spawn point
        // of every rift floor. F-press opens (or, solo, instantly
        // resolves) the exit vote that returns the party to the
        // hub with their current loot.
        let (vote_active, vote_cd) = self
            .exit_vote
            .as_ref()
            .map(|v| (v.active, v.cooldown_remaining))
            .unwrap_or((false, 0.0));
        let is_ghost = self.net.local_ghost_cached;
        portal_system::tick_rift_spawn(
            &mut self.rift_spawn_portal,
            &self.world,
            renderer,
            input,
            &mut self.net,
            &mut self.hud_prompt,
            self.in_hub,
            self.floor_mgr.spawn_pos,
            vote_active,
            vote_cd,
            is_ghost,
            dt,
        );

        // Exit-vote Y/N keys: only act when a vote is active and
        // the local player is still Pending. The actual cast is
        // queued onto `NetState` and shipped by the binary as
        // `ClientMsg::RiftExitVoteCast`.
        if vote_active {
            use winit::keyboard::KeyCode;
            let our_id = self.net.our_net_id_cached;
            let we_pending = self
                .exit_vote
                .as_ref()
                .and_then(|v| {
                    our_id.and_then(|nid| {
                        v.voters
                            .iter()
                            .find(|(id, _)| *id == nid)
                            .map(|(_, c)| *c)
                    })
                })
                .map(|c| matches!(c, rift_net::messages::VoteChoice::Pending))
                .unwrap_or(false);
            if we_pending {
                if input.key_just_pressed(KeyCode::KeyY) {
                    self.net.pending_exit_vote_casts.push(true);
                }
                if input.key_just_pressed(KeyCode::KeyN) {
                    self.net.pending_exit_vote_casts.push(false);
                }
            }
        }

        // Hub stash chest: F-press toggles the stash panel
        // (queues `OpenStash` / `CloseStash` for the server,
        // forces the inventory UI open, and swaps bag-click
        // semantics from equip to deposit).
        stash_system::tick(
            &self.world,
            &self.floor_mgr,
            input,
            &mut self.mp_inventory_ui,
            &mut self.net,
            &mut self.loot,
            &mut self.hud_prompt,
        );

        // Ground loot: hover prompt + F-to-pick.
        loot_system::tick(
            &self.world,
            &mut self.loot,
            &mut self.combat_text,
            input,
        );

        // ECS systems
        let action_cfg = PlayerActionConfig::default();
        let accept_input = !self.is_player_dead();
        player_action_pre_system(&mut self.world, input, dt, &action_cfg, accept_input);
        player_input_system(&mut self.world, input, dt);
        movement_system(&mut self.world, dt);
        player_action_post_system(&mut self.world, &action_cfg);
        collision_system(&mut self.world, &self.wall_colliders);
    }

    /// Phase 2 — ability casting + death + hit-react. Gated by
    /// the inventory pointer test so an in-panel click doesn't
    /// also fire a basic attack.
    fn update_combat(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        let (sw, sh) = renderer.screen_size();
        // Inventory input + draw is fused into the HUD render
        // pass below (single IM pass). Here we only gate gameplay
        // input: when the cursor is inside the inventory panel,
        // skip the combat tick so a click-to-equip doesn't also
        // fire a basic attack. Tab toggling happens inside the
        // inventory's `frame()` and is keyboard-only, so it's
        // safe to leave for the render pass.
        let mp = input.mouse_pos();
        let pointer_in_inventory = self.mp_inventory_ui.consumes_mouse(mp.0, mp.1, sw, sh);

        // Ability-based combat (sends cast requests to the server).
        if !self.is_player_dead() && !self.in_hub && !pointer_in_inventory {
            self.tick_combat(input, renderer, dt);
        }

        // Catch-all death detection: alive last frame, dead this
        // frame. HP is driven by snapshot deltas applied to the
        // local `Health` component by the net layer; the
        // `prev_player_hp` edge gives us a one-shot trigger that
        // doesn't need a parallel `dying` flag.
        let was_alive = self.prev_player_hp.map_or(false, |p| p > 0.001);
        let is_dead = self.is_player_dead();
        if was_alive && is_dead {
            self.trigger_player_death();
        }

        // Edge-detect the down-pose → ghost transition. Server's
        // `GHOST_RISE_DELAY` elapses, the snapshot row gains
        // `entity_flags::GHOST`, the binary mirrors that onto
        // `local_ghost_cached`, and we crossfade out of the
        // death pose into idle so the spectator avatar is
        // animated normally.
        let now_ghost = self.net.local_ghost_cached;
        if now_ghost && !self.prev_local_ghost {
            self.trigger_player_rise();
        }
        // Inverse edge: ghost → respawned. Strip the `Ghost`
        // marker so the engine's dead-gates re-engage if HP
        // somehow lands at 0 again before the world is rebuilt
        // (e.g. a future revive-shrine flow that doesn't trigger
        // a floor regen).
        if !now_ghost && self.prev_local_ghost {
            if let Some(pid) = self.player_id() {
                let _ = self.world.remove_one::<rift_engine::ecs::components::Ghost>(pid);
            }
        }
        self.prev_local_ghost = now_ghost;

        // Hit-react: detect a damage event on the local player and
        // play a one-shot reaction clip on the upper body. Mirrors
        // `enemy_anim_system`'s HitRecieve handling but lives on
        // the client because the local player isn't run through
        // that system. The SpellCast layer's built-in
        // `hit_cooldown` gates retriggering.
        if !is_dead {
            self.tick_player_hit_react();
        } else {
            // Keep `prev_player_hp` pinned to the dying value so
            // the alive→dead edge above stays one-shot.
            self.prev_player_hp = Some(0.0);
        }
    }

    /// Phase 3 — animation + render-side systems. Advances every
    /// system that produces frame-output state (skinning, decals,
    /// camera, fog, VFX, channel beams), plus the per-frame
    /// timer decays for HUD overlays.
    fn update_render(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // Tick combat text
        self.combat_text.tick(dt);

        // Despawn dead entities (animation-finished kills, etc.)
        let _kills = despawn_system(&mut self.world, renderer);

        // Render sync
        render_sync_system(&self.world, renderer);

        locomotion_anim_system(&mut self.world);
        enemy_anim_system(&mut self.world, dt);

        // Spell-cast state machine: advances the upper-body cast layer.
        // The returned `fire_events` list previously drove a deferred
        // `CastAbility` send so the server-spawned projectile would
        // emerge from the casting hand at the wind-up apex. We now
        // send the cast request immediately on click (see
        // `tick_combat`) so remote observers start their cast pose
        // at network-RTT latency instead of waiting out the local
        // wind-up animation. The fire events are still consumed
        // here to advance internal SpellCast state cleanly; we just
        // no longer translate them into network sends.
        let _ = cast_advance_system(&mut self.world, dt);

        skinning_system(&mut self.world, renderer, dt);
        self.decals.update(dt, renderer);

        // Local-avatar ghost tint. The forward pipeline pushes
        // a per-`RenderObject.tint` vec4; default `[1; 4]` is a
        // no-op opaque path. While the local player is in ghost
        // mode we tint the base skinned mesh + every outfit
        // attachment to a pale cyan-white at ~40% alpha so the
        // owner sees their own avatar as a translucent spirit.
        // Reset to opaque on every other frame so the moment
        // the server respawns us, the avatar instantly looks
        // solid again (no ramp / decay).
        self.apply_ghost_tint(renderer);

        // Channel beam visuals (Frost Ray etc.) — driven by reliable
        // `WorldEvent::ChannelTick` events buffered into
        // `self.channel.visuals` by the binary's event loop.
        super::ability::tick_channel_visuals(self, renderer, dt);

        // Equipment visual sync (other gameplay state, like the held
        // weapon's world position) still happens after skinning.
        let player_pos = self.world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.position)
            .next()
            .unwrap_or(Vec3::ZERO);

        // Skip aim updates while the player is dead — otherwise the
        // death pose's spine twist would keep tracking the cursor.
        if !self.is_player_dead() {
            let arm_aim = super::cursor::aim_dir(input, renderer, player_pos);
            if let Some(player_id) = self.player_id() {
                if let Ok(mut p) = self.world.get::<&mut rift_engine::ecs::components::Player>(player_id) {
                    p.aim_dir = arm_aim;
                }
            }
        }

        camera_follow_system(&self.world, renderer, input, &self.wall_aabbs);
        // Anchor the distance fog on the player so zooming the
        // camera out doesn't pull the fog wall in over the
        // character.
        renderer.fog_origin = player_pos;
        // Push the 8 nearest wall-torch lights for this frame.
        self.floor_mgr.torches.update_lights(renderer, player_pos);
        renderer.vfx_system.tick(dt);
        self.player_state.abilities.tick_all(dt);

        if self.damage_flash > 0.0 {
            self.damage_flash = (self.damage_flash - dt * 2.2).max(0.0);
        }
        if self.level_up_flash > 0.0 {
            self.level_up_flash = (self.level_up_flash - dt * 0.4).max(0.0);
        }
        if self.transition_fade > 0.0 {
            // ~0.6 s fade-out from full black. Timed to overlap
            // the first couple of fresh-floor frames so any
            // pop-in (animator first-frame, light flash) is
            // hidden by the curtain.
            self.transition_fade = (self.transition_fade - dt * 1.6).max(0.0);
        }
    }

    /// Phase 4 — HUD + inventory IM pass. One `Ui::begin/end`
    /// scope so layer order and `OverlayBatch` ownership stay
    /// coherent across every widget.
    fn update_ui(&mut self, renderer: &mut Renderer, input: &Input) {
        renderer.overlay_batch.clear();
        let (sw, sh) = renderer.screen_size();

        let nearest_loot = self.nearest_lootable_drop();
        let view_proj = renderer.camera.view_projection();
        let player_facing = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.rotation * Vec3::Z)
            .next()
            .unwrap_or(Vec3::Z);
        let hub_portal_pos = self.hub_portal.as_ref().map(|p| p.position);

        use rift_engine::ui::im::{Color, Ui, DEFAULT_THEME};
        let mut ui = Ui::begin(
            &mut renderer.overlay_batch,
            input,
            &mut self.ui_state,
            &DEFAULT_THEME,
            sw,
            sh,
        );
        if self.damage_flash > 0.001 {
            hud::render_damage_flash(&mut ui, self.damage_flash);
        }
        hud::render_hud(
            &mut ui,
            &self.world,
            &self.rift,
            &self.player_state,
            self.level_up_flash,
            self.in_hub,
        );
        if let Some(slot_idx) = hud::render_ability_bar(
            &mut ui,
            &self.player_state.abilities,
            self.player_state.experience.level,
        ) {
            // Click on a HUD bar slot opens the spellbook with
            // that slot pre-targeted; the next pool click
            // assigns directly without the two-step picker.
            self.spellbook.open_for_slot(slot_idx as u8);
        }
        hud::render_enemy_health_bars(&mut ui, &self.world, view_proj);
        if !self.in_hub {
            hud::render_boss_arrow(&mut ui, &self.world, view_proj);
            hud::render_remote_player_health_bars(&mut ui, &self.world, view_proj);
        }
        hud::render_minimap(
            &mut ui,
            &self.world,
            &self.floor_mgr.nav_grid,
            player_facing,
            hub_portal_pos,
        );
        self.combat_text.render(&mut ui, view_proj);
        self.mp_inventory_ui.frame(
            &mut ui,
            &self.loot.items,
            &self.loot.equipment,
            &mut self.loot.pending_equip_requests,
            self.loot.stash_session,
            &self.loot.stash_items,
            &mut self.loot.pending_stash_requests,
            &self.player_state,
        );

        // Spellbook toggle (B) — open / close the loadout editor.
        // Suppressed while a stash session is active so B doesn't
        // double-bind alongside the inventory drag context.
        if !self.loot.stash_session
            && ui.input().key_just_pressed(winit::keyboard::KeyCode::KeyB)
        {
            self.spellbook.toggle();
        }
        if let Some(action) = self.spellbook.frame(
            &mut ui,
            &self.player_state.loadout,
            self.player_state.experience.level,
        ) {
            match action {
                spellbook::SpellbookAction::AssignSlot { slot_index, ability_id } => {
                    self.net.pending_loadout_changes.push((slot_index, ability_id));
                }
            }
        }

        // Portal prompt: rendered above the loot prompt so a player
        // standing inside both prompt radii (which shouldn't happen
        // in practice — portals don't drop loot) sees both lines.
        if let Some(text) = self.hud_prompt.take() {
            hud::render_hud_prompt(&mut ui, text);
        }

        // Rift exit-vote panel: top-center card, only when we
        // either have an active vote or a non-zero cooldown to
        // surface. Drawn after the prompt so it visually layers
        // above the F-press hint at the bottom of the screen.
        if let Some(vote) = self.exit_vote.as_ref() {
            if vote.active || vote.cooldown_remaining > 0.0 {
                hud::render_exit_vote(&mut ui, vote, self.net.our_net_id_cached);
            }
        }
        if let Some((net_id, _)) = nearest_loot {
            if let Some(drop) = self.loot.drops.iter().find(|d| d.net_id == net_id) {
                let c = drop.item.rarity.color();
                let prompt = format!("PRESS [F]: {}", drop.item.display_name());
                hud::render_loot_prompt(
                    &mut ui,
                    &prompt,
                    Color::rgba(c[0], c[1], c[2], 1.0),
                );
            }
        }

        // Fade overlay sits on top of every other HUD element so
        // it covers the whole frame during a hub ↔ rift transition
        // (and during the post-death respawn, since the server
        // drives that via the same `LoadFloor` path).
        if self.transition_fade > 0.001 {
            hud::render_fade_to_black(&mut ui, self.transition_fade);
        }
        let _ = ui.end();
    }

    /// Tick the character-select screen.
    fn update_character_select(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        renderer.overlay_batch.clear();

        // Preview avatar (independent of UI; needs &mut World/Renderer).
        self.character_select
            .tick_preview(&mut self.world, renderer, dt);
        skinning_system(&mut self.world, renderer, dt);

        // Fused input + render through the immediate-mode UI stack.
        let (sw, sh) = renderer.screen_size();
        let action = {
            use rift_engine::ui::im::{Ui, DEFAULT_THEME};
            let mut ui = Ui::begin(
                &mut renderer.overlay_batch,
                input,
                &mut self.ui_state,
                &DEFAULT_THEME,
                sw,
                sh,
            );
            let action = self.character_select.frame(&mut ui);
            let _ = ui.end();
            action
        };

        match action {
            character_select::SelectAction::None => {}
            character_select::SelectAction::AccountConfirmed { name } => {
                self.net.roster_request = Some(name);
            }
            character_select::SelectAction::Play { account_name, profile } => {
                self.start_with_profile(account_name, profile);
            }
            character_select::SelectAction::Quit => {
                log::info!("Quit requested from character select");
            }
        }
    }

    /// Forward a server-supplied roster into the character-select
    /// screen. Called by the binary once the net client receives
    /// `ServerMsg::Roster` after we issued `RequestRoster`.
    pub fn apply_server_roster(
        &mut self,
        entries: Vec<rift_net::messages::RosterEntry>,
    ) {
        self.character_select.apply_server_roster(entries);
    }

    fn start_with_profile(
        &mut self,
        account_name: String,
        profile: character::CharacterProfile,
    ) {
        log::info!(
            "Entering world as '{}' on account '{}' ({:?})",
            profile.name, account_name, profile.gender,
        );
        self.player_state = PlayerState::with_profile(
            profile.gender,
            profile.name.clone(),
            rift_game::loadout::Loadout::default_hero(),
        );
        // Hand the profile + account to the binary so it can
        // advertise them on the wire. In SP this is just dropped.
        self.net.profile = Some(profile);
        self.net.account_name = Some(account_name);
        self.app_state = AppState::EnteringWorld(EnterPhase::PrepareScene);
    }

    fn tick_entering_world(&mut self, renderer: &mut Renderer, phase: EnterPhase) {
        let (label, next): (&'static str, Option<EnterPhase>) = match phase {
            EnterPhase::PrepareScene => {
                self.character_select.teardown_preview(&mut self.world, renderer);
                renderer.point_lights.clear();
                ("Preparing world…", Some(EnterPhase::PreloadHub))
            }
            EnterPhase::PreloadHub => {
                // Stream a few gltf assets per tick so the netcode
                // loop keeps running and the server doesn't time us
                // out while the hub forest decodes.
                let paths = super::props::nature::hub_asset_paths();
                let loaded = self.floor_mgr.props.preload_step(&paths, 2);
                let total = super::props::nature::hub_total_assets();
                let done = self.floor_mgr.props.loaded_count(&paths);
                let next = if done >= total || loaded == 0 {
                    Some(EnterPhase::GenerateHub)
                } else {
                    Some(EnterPhase::PreloadHub)
                };
                ("Loading environment…", next)
            }
            EnterPhase::GenerateHub => {
                self.in_hub = true;
                self.rift = RiftState::new(1);
                match self.floor_mgr.generate_hub(
                    &mut self.world,
                    renderer,
                    &self.player_state,
                    &mut self.anim_cache,
                ) {
                    Ok(portal_pos) => portal_system::spawn_hub(&mut self.hub_portal, renderer, portal_pos),
                    Err(e) => log::error!("Hub generation failed: {}", e),
                }
                ("Generating hub…", Some(EnterPhase::AttachOutfits))
            }
            EnterPhase::AttachOutfits => {
                ("Preparing outfits…", Some(EnterPhase::LoadOutfits))
            }
            EnterPhase::LoadOutfits => {
                ("Loading outfits…", Some(EnterPhase::RebuildWalls))
            }
            EnterPhase::RebuildWalls => {
                self.rebuild_wall_caches();
                ("Finalizing…", None)
            }
        };

        hud::draw_world_loading_overlay(renderer, 0.0, label);

        match next {
            Some(p) => self.app_state = AppState::EnteringWorld(p),
            None => self.app_state = AppState::Playing,
        }
    }

    fn tick_combat(&mut self, input: &Input, renderer: &mut Renderer, _dt: f32) {
        use glam::Mat4;
        use winit::keyboard::KeyCode;

        let player_data: Option<(Vec3, glam::Quat)> = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| (t.position, t.rotation))
            .next();

        let Some((player_pos, _player_rot)) = player_data else {
            return;
        };

        let aim_dir = super::cursor::aim_dir(input, renderer, player_pos);

        // ─── Placed ability targeting mode ─────────────────────────────────
        if self.targeting.is_some() {
            if let Some(cursor_pos) = super::cursor::world_pos(input, renderer, 0.0) {
                let targeting = self.targeting.as_ref().unwrap();
                let radius = targeting.radius;
                if let Some(obj_idx) = targeting.indicator_obj {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix =
                            Mat4::from_translation(cursor_pos)
                                * Mat4::from_scale(Vec3::splat(radius));
                    }
                }
            }

            // Left-click: confirm placement → forward to server.
            if input.left_clicked() {
                if let Some(cursor_pos) = super::cursor::world_pos(input, renderer, 0.0) {
                    let targeting = self.targeting.take().unwrap();
                    if let Some(obj_idx) = targeting.indicator_obj {
                        if obj_idx < renderer.objects.len() {
                            renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                        }
                    }
                    self.net.casts.push(NetCastRequest {
                        ability_id: targeting.ability.wire_id,
                        origin: player_pos,
                        aim_dir,
                        placed_target: Some(cursor_pos),
                    });
                }
                return;
            }

            // Right-click or Escape: cancel targeting.
            if input.right_clicked() || input.key_just_pressed(KeyCode::Escape) {
                let targeting = self.targeting.take().unwrap();
                if let Some(obj_idx) = targeting.indicator_obj {
                    if obj_idx < renderer.objects.len() {
                        renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                    }
                }
                if let Some(state) = &mut self.player_state.abilities.slots[targeting.slot_index] {
                    state.cooldown_remaining = 0.0;
                }
                return;
            }

            return;
        }

        // ─── Channel hold-to-cast / cancel logic ──────────────────────────
        // If we're currently channeling, a release of the channel's
        // slot key, any movement input, or a manual right-click /
        // Escape ends the channel. Server is authoritative — we
        // just queue the request for the binary to forward.
        if let Some(active) = self.channel.active {
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
                self.channel.pending_ends.push(active.ability_id);
                self.channel.active = None;
                // Tear our local cast pose down. Server will emit
                // ChannelEnd which the binary handles as well, but
                // doing it here keeps the local view snappy.
                if let Some(pid) = self.player_id() {
                    if let Ok(mut cast) = self
                        .world
                        .get::<&mut rift_engine::ecs::components::SpellCast>(pid)
                    {
                        cast.cancel();
                    }
                }
            } else {
                // Decay the local timeout. If the server's ChannelEnd
                // gets dropped this is the safety net.
                let mut a = active;
                a.remaining = (a.remaining - _dt).max(0.0);
                self.channel.active = if a.remaining > 0.0 { Some(a) } else { None };
                // While channeling we suppress new ability presses
                // so a frantic player can't queue another cast on
                // top.
                return;
            }
        }

        // ─── Normal ability keybinds ──────────────────────────────────────
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
            if let Some(ability) = self.player_state.abilities.try_use(i) {
                let ability_clone = ability.clone();

                // Placed ability → enter targeting mode locally.
                if let TargetingMode::Placed { radius } = ability_clone.targeting {
                    let indicator_mesh = rift_engine::Mesh::targeting_circle([0.2, 0.5, 1.0]);
                    let initial_pos = super::cursor::world_pos(input, renderer, 0.0)
                        .unwrap_or(player_pos);
                    let initial_mat = Mat4::from_translation(initial_pos)
                        * Mat4::from_scale(Vec3::splat(radius));
                    let indicator_obj = if let Ok(()) = renderer.add_mesh(&indicator_mesh, initial_mat) {
                        Some(renderer.objects.len() - 1)
                    } else {
                        None
                    };

                    self.targeting = Some(PlacedTargeting {
                        slot_index: i,
                        ability: ability_clone,
                        radius,
                        indicator_obj,
                    });
                    break;
                }

                // Server is authoritative. Send the cast request
                // immediately for every ability kind — including
                // projectiles — so remote observers start their
                // upper-body cast pose at network-RTT latency
                // instead of `wind_up_clip_duration + RTT` (the
                // earlier "defer until apex" path made remote
                // poses lag the local one by the full wind-up
                // animation, which felt heavy on rapid LMB
                // attacks and Multishot but not on Frost Ray
                // because channels were always sent immediately).
                // The trade-off: the server projectile now spawns
                // at chest height when the click lands, rather
                // than from the casting hand at swing apex. The
                // local player still plays the full wind-up clip
                // for input-feedback feel.
                let def = rift_game::abilities::lookup(ability_clone.wire_id)
                    .map(|d| d.kind);
                let is_channel = matches!(
                    def,
                    Some(rift_game::abilities::AbilityKind::Channel { .. })
                );
                let placed_target = if let TargetingMode::Placed { .. } = ability_clone.targeting {
                    super::cursor::world_pos(input, renderer, 0.0)
                } else {
                    None
                };
                // Compute a chest-height (or hand-joint) origin so
                // server-spawned projectiles don't appear to come
                // out of the ground. `player_pos` is the foot anchor
                // (y≈0). Prefer the right-hand joint's current world
                // position from the last skinning pass; fall back to
                // a fixed +1.25m torso offset which the server
                // accepts as "trusted" within its 2m sanity radius.
                let origin = {
                    use rift_engine::ecs::components::Skinned;
                    let pid = self
                        .world
                        .query::<(&Player, &LocalPlayer)>()
                        .iter()
                        .map(|(e, _)| e)
                        .next();
                    let mut hand: Option<Vec3> = None;
                    if let Some(pid) = pid {
                        let mut q = self
                            .world
                            .query_one::<(&Transform, &Player, Option<&Skinned>)>(pid)
                            .ok();
                        hand = q.as_mut().and_then(|q| q.get()).and_then(|(t, p, s)| {
                            if p.hand_joint == u32::MAX {
                                return None;
                            }
                            let s = s?;
                            let m = s.joint_worlds.get(p.hand_joint as usize)?;
                            let local = m.col(3).truncate();
                            Some(t.matrix().transform_point3(local))
                        });
                    }
                    hand.unwrap_or(player_pos + Vec3::Y * 1.25)
                };
                self.net.casts.push(NetCastRequest {
                    ability_id: ability_clone.wire_id,
                    origin,
                    aim_dir,
                    placed_target,
                });
                let _ = is_channel;

                // Hold-to-channel latch. Only infinite-duration
                // channels (Frost Ray) need client-side hold/
                // release tracking — finite-duration channels
                // (Fire Wave, Whirlwind) run on the server's own
                // clock and would otherwise be cancelled by the
                // very next frame's "key not held" check, which
                // strips the ServerChannel before its first tick
                // interval has elapsed and no enemies are ever
                // hit.
                if let Some(def) = rift_game::abilities::lookup(ability_clone.wire_id) {
                    if let rift_game::abilities::AbilityKind::Channel { duration, cancel_on_move, .. } = def.kind {
                        if duration.is_infinite() {
                            self.channel.active = Some(ActiveChannel {
                                ability_id: ability_clone.wire_id,
                                slot_index: i,
                                cancel_on_move,
                                // Grace period: server's
                                // ChannelEnd may arrive a frame
                                // late; this prevents a stale
                                // release from firing.
                                remaining: duration + 0.25,
                            });
                        }
                    }
                }

                // Local visual feedback. The server still owns the
                // damage / projectile spawn — we just play the cast
                // animation + any client-side particles immediately
                // so the input feels responsive.
                super::ability::trigger_local_cast(&ability_clone, aim_dir, player_pos, &mut self.world, renderer, &self.player_state.talents);
            }
        }
    }

    fn player_id(&self) -> Option<hecs::Entity> {
        self.world
            .query::<(&Player, &rift_engine::ecs::components::LocalPlayer)>()
            .iter()
            .map(|(e, _)| e)
            .next()
    }

    /// `true` while the local player is in the post-death
    /// down-pose: HP is at zero AND the server hasn't yet
    /// flipped us to ghost mode. Once `local_ghost_cached`
    /// goes true we leave the down-pose — input + camera +
    /// movement systems all re-engage so the player can scout.
    /// Cast / loot pickup remain server-rejected for ghosts.
    fn is_player_dead(&self) -> bool {
        if self.net.local_ghost_cached {
            return false;
        }
        let Some(pid) = self.player_id() else { return false };
        self.world.get::<&Health>(pid).map(|h| h.is_dead()).unwrap_or(false)
    }

    /// `true` while the local player is a ghost (risen-but-dead).
    /// Mirrors `NetClient::is_local_ghost()`. Used by the HUD to
    /// surface the spectator state and (eventually) by the
    /// renderer to swap the avatar to the translucent ghost
    /// material.
    #[allow(dead_code)]
    fn is_player_ghost(&self) -> bool {
        self.net.local_ghost_cached
    }

    /// Detect HP drops on the local player since last frame and play
    /// a hit-react one-shot on the upper body. Uses the built-in
    /// `SpellCast::play_hit` cooldown so the flinch doesn't repeat
    /// every frame while the player is being chewed on.
    fn tick_player_hit_react(&mut self) {
        use rift_engine::ecs::components::{AnimationSet, SpellCast};

        let Some(player_id) = self.player_id() else {
            self.prev_player_hp = None;
            return;
        };
        let cur_hp = match self.world.get::<&Health>(player_id) {
            Ok(h) => h.current,
            Err(_) => return,
        };
        let prev = self.prev_player_hp;
        self.prev_player_hp = Some(cur_hp);
        let Some(prev) = prev else { return };
        if cur_hp + 0.001 >= prev {
            return;
        }
        // Don't replay if death just triggered.
        if cur_hp <= 0.001 {
            return;
        }

        // Pick a chest/head hit at random for variety. The asset
        // pack ships `Hit_Chest` and `Hit_Head`; either is fine.
        let candidates: &[&str] = if self.rift.floor as u32 % 2 == 0 {
            &["Hit_Chest", "Hit_Head", "HitRecieve", "HitReceive", "Hit"]
        } else {
            &["Hit_Head", "Hit_Chest", "HitRecieve", "HitReceive", "Hit"]
        };
        let clip = match self.world.get::<&AnimationSet>(player_id) {
            Ok(set) => set.find_any(candidates),
            Err(_) => None,
        };
        let Some(clip) = clip else { return };

        if let Ok(mut cast) = self.world.get::<&mut SpellCast>(player_id) {
            cast.play_hit(clip);
        }
    }

    /// Triggered when the snapshot brings local Health to zero. Plays
    /// the death animation and freezes input. Server-authoritative
    /// respawn happens via a follow-up `LoadFloor`.
    fn trigger_player_death(&mut self) {
        use rift_engine::animation::Animator;
        use rift_engine::ecs::components::{
            AnimationSet, Player, PlayerAction, SpellCast, Velocity,
        };

        self.damage_flash = (self.damage_flash + 0.45).min(0.85);
        log::info!("Player death triggered (rift floor {}).", self.rift.floor);

        let Some(player_id) = self.player_id() else { return };

        let candidates: &[&str] = &["Death01", "Death_01", "Death", "Death02", "Death_02"];

        let clip = match self.world.get::<&AnimationSet>(player_id) {
            Ok(set) => set.find_any(candidates),
            Err(_) => None,
        };
        let Some(clip) = clip else {
            log::warn!("Death animation not found in player's clip set");
            return;
        };

        if let Ok(mut cast) = self.world.get::<&mut SpellCast>(player_id) {
            cast.phase = rift_engine::ecs::components::SpellPhase::Idle;
            cast.layer_animator = None;
            cast.weight = 0.0;
            cast.pending_oneshot = None;
            cast.oneshot_is_hit = false;
        }
        if let Ok(mut anim) = self.world.get::<&mut Animator>(player_id) {
            anim.cross_fade(clip, false, 0.18);
            anim.speed = 1.0;
        }
        if let Ok(mut vel) = self.world.get::<&mut Velocity>(player_id) {
            vel.linear = Vec3::ZERO;
        }
        if let Ok(mut p) = self.world.get::<&mut Player>(player_id) {
            p.action = PlayerAction::None;
            p.action_timer = 0.0;
            p.vy = 0.0;
            p.airborne = false;
        }
    }

    /// Crossfade the local avatar out of the death pose into
    /// idle once the server flips us to ghost mode. Mirror
    /// image of [`Self::trigger_player_death`] — same
    /// component touch-points, opposite clip + intent. Runs
    /// once per ghost transition (edge-detected via
    /// `prev_local_ghost`).
    fn trigger_player_rise(&mut self) {
        use rift_engine::animation::Animator;
        use rift_engine::ecs::components::{AnimationSet, Ghost, SpellCast};

        log::info!("Player rose as ghost (rift floor {}).", self.rift.floor);

        let Some(player_id) = self.player_id() else { return };

        // Tag the local avatar with `Ghost` so the engine systems
        // that gate on `Health::is_dead()` (locomotion, input,
        // jump-land, roll dispatch) treat us as alive even though
        // HP is still 0. Removed in `reset_for_regeneration` on
        // respawn.
        let _ = self.world.insert_one(player_id, Ghost);

        // Asset pack ships `LayToIdle` (UAL2) which is the perfect
        // get-up-from-corpse-pose anim. Fall back to plain Idle
        // crossfade if the rig somehow lacks it.
        let lay_to_idle = self.world.get::<&AnimationSet>(player_id)
            .ok()
            .and_then(|set| set.find_any(&["LayToIdle"]));

        if let Ok(mut cast) = self.world.get::<&mut SpellCast>(player_id) {
            cast.phase = rift_engine::ecs::components::SpellPhase::Idle;
            cast.layer_animator = None;
            cast.weight = 0.0;
            cast.pending_oneshot = None;
            cast.oneshot_is_hit = false;
        }
        if let Some(clip) = lay_to_idle {
            // `LayToIdle` is a one-shot get-up animation. After it
            // finishes the animator holds the last frame (standing
            // idle pose); locomotion_anim_system then takes over
            // once the player starts moving (Ghost marker bypasses
            // the dead-gate).
            if let Ok(mut anim) = self.world.get::<&mut Animator>(player_id) {
                anim.cross_fade(clip, false, 0.25);
                anim.speed = 1.0;
            }
            return;
        }

        // Fallback: straight idle crossfade.
        let candidates: &[&str] = &["Idle_Loop", "Idle", "Idle01", "Idle_01", "Idle02"];
        let clip = match self.world.get::<&AnimationSet>(player_id) {
            Ok(set) => set.find_any(candidates),
            Err(_) => None,
        };
        let Some(clip) = clip else {
            log::warn!("Idle animation not found for ghost rise");
            return;
        };
        if let Ok(mut anim) = self.world.get::<&mut Animator>(player_id) {
            anim.cross_fade(clip, true, 0.35);
            anim.speed = 1.0;
        }
    }

    /// Push the ghost tint onto the local player's renderer
    /// slots when `local_ghost_cached` is true; otherwise force
    /// them back to opaque white. Touches the base `Renderable`
    /// slot plus every visible `SkinnedAttachments` piece (so
    /// outfit gear ghosts together with the body). Cheap O(N)
    /// over the local avatar's attachments — runs every frame
    /// from `update_render` so a respawn snaps back to opaque
    /// without a one-frame flicker.
    fn apply_ghost_tint(&mut self, renderer: &mut Renderer) {
        use rift_engine::ecs::components::{LocalPlayer, Renderable, SkinnedAttachments};

        // Pale cyan-white at 40% alpha. RGB > 1.0 in the cyan
        // channels gives the lit colour a faint spectral lift
        // even after the multiply (lit * tint), since the
        // forward pipeline outputs HDR before tonemap.
        const GHOST_TINT: [f32; 4] = [0.75, 0.92, 1.05, 0.40];
        const OPAQUE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        let is_ghost = self.net.local_ghost_cached;
        let tint = if is_ghost { GHOST_TINT } else { OPAQUE };

        // Drive the post-composite ghost-view effect (desat +
        // cool tint + radial vignette). Instant-on for now \u2014
        // could be eased over ~0.3s on the rise edge if we want
        // a softer transition.
        renderer.ghost_mix = if is_ghost { 1.0 } else { 0.0 };

        // Find the local player's avatar entity. There's at most
        // one (`LocalPlayer` is a singleton tag on the predicted
        // avatar) so we just grab the first match.
        let mut local_entity = None;
        for (e, _) in self.world.query::<&LocalPlayer>().iter() {
            local_entity = Some(e);
            break;
        }
        let Some(entity) = local_entity else { return };

        // Base mesh.
        if let Ok(r) = self.world.get::<&Renderable>(entity) {
            if let Some(obj) = renderer.objects.get_mut(r.object_index) {
                obj.tint = tint;
            }
        }
        // Outfit attachments.
        if let Ok(attach) = self.world.get::<&SkinnedAttachments>(entity) {
            for piece in &attach.pieces {
                if let Some(obj) = renderer.objects.get_mut(piece.object_index) {
                    obj.tint = tint;
                }
            }
        }
    }
}

// Ability / channel / remote-death event handlers live in
// [`super::ability`]. They are re-exported below for callers that
// historically reached them through `game::state::*`.
pub use super::ability::{
    on_channel_end, on_channel_tick, on_remote_ability_cast, on_remote_death,
};

// `WorldEvent::LootDropped` handler lives in [`super::loot_system`]
// next to the rest of the loot pickup / inventory plumbing. Callers
// thread `state.loot` explicitly rather than reaching back through
// `GameState`.
pub use super::loot_system::on_loot_dropped;
