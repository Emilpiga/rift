// Ability / TargetingMode types are referenced through `super::combat_system`.
use rift_engine::ui::CombatTextSystem;
use rift_engine::{Input, LoadStatus, Renderer};

use super::character_select;
use super::character_spawn;
use super::floor::FloorManager;
use super::loot_system;
use super::monster_assets::load_role;
use super::player_state::PlayerState;
use super::rift_state::RiftState;
use super::spellbook;
pub use super::sub_state::*;
use rift_game::monsters;

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
    /// Inventory-screen UI state. Logic lives in
    /// `rift_ui::inventory`; the host owns the persistent
    /// state struct so it survives hot-reload.
    pub inventory_ui: rift_ui_types::inventory::InventoryUiState,
    pub combat_text: CombatTextSystem,
    /// Cross-frame immediate-mode UI state — owns focus, hover,
    /// drag, and the modal stack. Borrowed by `Ui::begin` once
    /// per frame; widgets in [`rift_engine::ui::im`] thread it
    /// transparently. Landing 1 is scaffolding-only; subsequent
    /// landings migrate the bespoke panels onto it.
    pub ui_state: rift_engine::ui::im::UiState,
    pub(super) needs_new_floor: bool,
    /// Per-floor state: walls, portals, hub flag. Rebuilt on
    /// every floor regen.
    pub floor: super::floor_state::FloorState,
    /// Per-frame transient state: targeting, HUD timers, hit-react
    /// edge detectors. Wiped each regen via `FrameState::reset`.
    pub frame: super::frame_state::FrameState,
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
    /// Server-mirrored revive-shrine visuals + local channel
    /// intent. Spawned/despawned in lockstep with snapshot
    /// `EntityKind::ReviveShrine` rows.
    pub shrines: super::sub_state::ShrineClientState,

    /// Top-level state (character-select vs playing).
    pub(super) app_state: AppState,
    /// Owns the character-select screen UI + preview avatar.
    pub(super) character_select: character_select::CharacterSelect,
    /// Spellbook overlay state (open/closed + selected ability).
    /// Toggled with `B`; mutates the loadout via
    /// `request_set_loadout_slot` and waits for the server to
    /// echo the new bar back through `ServerMsg::Loadout`.
    pub spellbook: spellbook::SpellbookUi,
    /// In-game chat HUD: scrollback panel + input field +
    /// per-player mute list. Inbound lines flow in from the
    /// binary draining `NetClient::take_pending_chats`;
    /// outbound lines flow out via `state.net.pending_chats_out`.
    pub chat: super::chat::ChatUi,
    /// Party HUD: top-left frames, invite/error toasts,
    /// portal-entry modal, per-member confirm modal, and the
    /// right-click context menu. Mirrors authoritative
    /// `ServerMsg::PartyState` snapshots; intents flow back
    /// out via `state.net.pending_party_msgs`,
    /// `pending_propose_rift_entry`, `pending_portal_confirm`.
    pub party: super::party::PartyUi,
    /// Combat-meter HUD panel (bottom-right while in a rift).
    /// Displays authoritative DMG/HPS/TAKEN/THREAT scores
    /// pushed by the server roughly once per second. Drained
    /// from `NetClient::take_pending_meters` by the binary's
    /// frame loop.
    pub meters: super::meters::MeterUi,
    /// Shared cache of bound player-skeleton animation sets, keyed by
    /// gender. Populated lazily on first spawn (local or remote).
    pub anim_cache: character_spawn::AnimLibraryCache,
    /// Cache of remapped equipment-attachment meshes keyed by
    /// (model path, host skeleton size). Populated lazily as items
    /// with a `model_path` get equipped on any visible player.
    pub equipment_visual_cache: super::equipment_visuals::EquipmentVisualCache,
    /// Cache of remapped head-cosmetic meshes (eyes / eyebrows /
    /// hair) keyed by (gltf path, sub-mesh name, host skeleton
    /// size). Populated lazily on first avatar spawn per gender.
    pub avatar_cosmetics_cache: super::avatar_cosmetics::AvatarCosmeticsCache,
    /// Bind-pose mesh cache for ground-loot 3D visuals,
    /// keyed by glTF/GLB path. Populated lazily the first
    /// time a base item with `models` set drops on the floor.
    pub loot_model_cache: super::loot_models::LootModelCache,
    /// Rigid-prop mesh cache for equipped weapons, keyed by
    /// glTF/GLB path. Populated lazily the first time any
    /// player equips a weapon whose `BaseItem::models` is set.
    /// Drives the casting-hand-attached weapon visuals managed
    /// by [`super::weapon_visuals`].
    pub weapon_visual_cache: super::weapon_visuals::WeaponMeshCache,
    /// Spatial-audio runtime. `None` when the audio backend
    /// failed to initialise (no output device available, OS
    /// audio service down, etc.) — every helper short-circuits
    /// on `None` so a missing audio device is never fatal.
    /// Owns a kira `AudioManager`, a single listener, a
    /// path-keyed sound cache, and a generational emitter
    /// table. See `rift_audio` for the full API.
    pub audio: Option<rift_audio::AudioSystem>,
}

/// Hub entry portal. Visual + interaction state for the glowing ring
/// the player walks into to start a rift run. Lives in
/// `portal_system::HubPortal` now; this re-export keeps the
/// existing `Option<HubPortal>` field declarations below wired
/// up without extra path noise.
// (HubPortal type imported above from portal_system.)

/// Active placed-ability targeting state (player is choosing where to place an AoE).
// Moved to [`super::combat_system::PlacedTargeting`].

/// Top-level app state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AppState {
    /// Showing the roster / create / delete screen.
    CharacterSelect,
    /// User picked Play. Run heavy world setup one chunk per frame.
    EnteringWorld(super::transition::EnterPhase),
    /// Server told us to switch floors. Multi-step so the player
    /// sees a "Entering Floor N…" loading screen instead of a
    /// frozen frame while dungeon regen runs.
    NetEntering(super::transition::NetEnterPhase),
    /// Player is in-game (hub or rift).
    Playing,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            world: hecs::World::new(),
            rift: RiftState::new(1),
            player_state: PlayerState::new(),
            floor_mgr: FloorManager::new(),
            inventory_ui: rift_ui_types::inventory::InventoryUiState::new(),
            combat_text: CombatTextSystem::new(),
            ui_state: rift_engine::ui::im::UiState::new(),
            needs_new_floor: false,
            floor: super::floor_state::FloorState::default(),
            frame: super::frame_state::FrameState::default(),
            exit_vote: None,
            loading: LoadingState::default(),
            net: NetState::default(),
            channel: ChannelState::default(),
            loot: LootClientState::default(),
            shrines: super::sub_state::ShrineClientState::default(),
            app_state: AppState::CharacterSelect,
            character_select: character_select::CharacterSelect::new(),
            anim_cache: character_spawn::AnimLibraryCache::new(),
            equipment_visual_cache: super::equipment_visuals::EquipmentVisualCache::new(),
            avatar_cosmetics_cache: super::avatar_cosmetics::AvatarCosmeticsCache::new(),
            loot_model_cache: super::loot_models::LootModelCache::new(),
            weapon_visual_cache: super::weapon_visuals::WeaponMeshCache::new(),
            spellbook: spellbook::SpellbookUi::new(),
            chat: super::chat::ChatUi::new(),
            party: super::party::PartyUi::new(),
            meters: super::meters::MeterUi::new(),
            // Audio system: best-effort init. Failures here
            // leave `audio = None`; every audio call site
            // tolerates the missing system gracefully.
            audio: match rift_audio::AudioSystem::new("assets") {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("audio: backend init failed: {e}; running silent");
                    None
                }
            },
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

    /// Surface a "Not your loot" warning above the local player.
    /// Called by the binary when the server replies with
    /// `PickupRejected::NotEligible` — the picker isn't on the
    /// share-window eligibility snapshot for that drop.
    pub fn warn_not_eligible(&mut self) {
        loot_system::warn_not_eligible(&self.world, &mut self.combat_text);
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
        unsafe {
            renderer.ash_device().device_wait_idle().ok();
        }
        let device = renderer.ash_device().clone();
        let allocator = renderer.allocator_arc();
        self.floor_mgr.monsters.cleanup_gpu(&device, &allocator);
        self.floor_mgr.props.cleanup_gpu(&device, &allocator);
        self.floor_mgr.env.cleanup_gpu(&device, &allocator);
    }

    /// `true` while the staged net-transition state machine is
    /// active. The binary uses this to gate snapshot → ECS sync
    /// so we don't try to spawn remote avatars / enemies into a
    /// half-rebuilt world during the loading screen.
    pub fn is_net_transitioning(&self) -> bool {
        matches!(self.app_state, AppState::NetEntering(_))
    }

    /// Queue a server-driven floor transition. Sets app state
    /// to [`AppState::NetEntering`]; the staged tick presents a
    /// loading screen, runs the heavy regen behind it, and
    /// fades back to gameplay — same UX as the hub-entry flow.
    pub fn apply_net_transition(&mut self, renderer: &mut Renderer, index: u32) {
        crate::game::transition::queue_net_transition(self, renderer, index);
    }
    pub fn update(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        // Suppress gameplay-style key polling whenever the
        // chat HUD is open *or* an inventory text widget (e.g.
        // the stash-tab rename field) is active so Space /
        // 1..6 / WASD / T don't leak into movement, hotbar or
        // chat-open while the player types. The chat itself
        // uses widget-facing accessors (chars_typed /
        // enter_just_pressed), which the flag intentionally
        // leaves alone.
        input.set_text_capture(self.chat.is_typing() || self.inventory_ui.wants_text_input());
        match self.app_state.clone() {
            AppState::CharacterSelect => {
                crate::game::transition::update_character_select(self, renderer, input, dt);
                return;
            }
            AppState::EnteringWorld(phase) => {
                crate::game::transition::tick_entering_world(self, renderer, phase);
                return;
            }
            AppState::NetEntering(phase) => {
                crate::game::transition::tick_net_entering(self, renderer, phase);
                return;
            }
            AppState::Playing => {}
        }

        // Gameplay → combat → render → UI. Each phase lives in
        // its own module so the `update` header reads as a
        // high-level outline; field access works because the
        // phase modules are siblings of `state` and reach in
        // through `pub(super)` visibility.
        super::gameplay_phase::tick(self, renderer, input, dt);
        super::combat_phase::tick(self, renderer, input, dt);
        super::render_phase::tick(self, renderer, input, dt);
        super::ui_phase::tick(self, renderer, input);

        // Mark needs_new_floor as consumed (kept for future use,
        // but no SP path sets it any more).
        if self.needs_new_floor {
            self.needs_new_floor = false;
        }
    }

    /// Forward a server-supplied roster into the character-select
    /// screen. Called by the binary once the net client receives
    /// `ServerMsg::Roster` after we issued `RequestRoster`.
    pub fn apply_server_roster(&mut self, entries: Vec<rift_net::messages::RosterEntry>) {
        crate::game::transition::apply_server_roster(self, entries);
    }

    /// Bypass the in-screen account-entry view and jump straight
    /// to the "loading roster" placeholder. Called by the binary
    /// when the auth resolver provides credentials at startup so
    /// the player never has to type an account name (the dev
    /// `identity` or Steam persona stands in).
    pub fn character_select_skip_to_loading(&mut self, account_identity: String) {
        self.character_select.skip_to_loading(account_identity);
    }
}

// Ability / channel / remote-death event handlers live in
// [`super::ability`]. They are re-exported below for callers that
// historically reached them through `game::state::*`.
pub use super::ability::{
    on_channel_end, on_channel_pulse, on_channel_tick, on_remote_ability_cast, on_remote_death,
};

// `WorldEvent::LootDropped` handler lives in [`super::loot_system`]
// next to the rest of the loot pickup / inventory plumbing. Callers
// thread `state.loot` explicitly rather than reaching back through
// `GameState`.
pub use super::loot_system::on_loot_dropped;
