use glam::Vec3;
use rift_game::abilities::{Ability, AbilitySlot, TargetingMode};
use rift_game::attributes::{AttributeScaling, Attributes};
use rift_game::classes::{ClassConfig, ClassId};
use rift_game::experience::Experience;
use rift_game::talents::TalentTree;
use rift_engine::ecs::components::{Health, LocalPlayer, Player, Transform};
use rift_engine::loot::inventory::PlayerStats;
use rift_engine::ui::CombatTextSystem;
use rift_engine::renderer::decals::DecalSystem;
use super::equipment_visuals::{self, EquipmentVisuals};
use rift_engine::ecs::systems::{
    camera_follow_system, cast_advance_system, collision_system, despawn_system,
    enemy_anim_system, locomotion_anim_system, movement_system, player_action_post_system,
    player_action_pre_system, player_input_system, render_sync_system, skinning_system,
    PlayerActionConfig,
};
use rift_engine::loot::{Equipment, Inventory};
use rift_engine::ui::InventoryUI;
use rift_engine::{Input, LoadStatus, Renderer};

use rift_game::abilities;
use rift_game::character::{self, Gender};
use rift_game::classes;
use rift_game::monsters;
use rift_game::talents;
use super::character_select;
use super::character_spawn;
use super::floor::FloorManager;
use super::hud;
use super::monster_assets::load_role;
use super::mp_inventory_ui;
use super::rift_state::RiftState;

/// Slim, client-side player profile. The server is authoritative for
/// damage / XP / loot, so this struct only carries data the local
/// rendering + UX paths read: ability cooldowns (HUD ability bar),
/// experience level (HUD XP bar), gender / config (skinned avatar
/// spawn), talents (visual ability tweaks).
pub struct PlayerState {
    pub class: ClassId,
    pub gender: Gender,
    pub name: String,
    pub config: ClassConfig,
    pub attributes: Attributes,
    pub attribute_scaling: AttributeScaling,
    pub experience: Experience,
    pub abilities: AbilitySlot,
    pub talents: TalentTree,
}

impl PlayerState {
    pub fn new(class: ClassId) -> Self {
        Self::with_profile(class, Gender::Female, String::new())
    }

    pub fn with_profile(class: ClassId, gender: Gender, name: String) -> Self {
        let config = classes::config_for(class);
        let attributes = Attributes::for_class(config.primary_attribute);
        let attribute_scaling = AttributeScaling::new(config.primary_attribute);

        let mut ability_slots = AbilitySlot::new();
        let roster: [Ability; 6] = match class {
            classes::HUNTER => abilities::hunter_roster(),
            _ => abilities::hunter_roster(),
        };
        for (i, ab) in roster.into_iter().enumerate() {
            ability_slots.set(i, ab);
        }

        let talents = match class {
            classes::HUNTER => talents::hunter_tree(),
            _ => talents::hunter_tree(),
        };

        Self {
            class,
            gender,
            name,
            config,
            attributes,
            attribute_scaling,
            experience: Experience::new(),
            abilities: ability_slots,
            talents,
        }
    }

    pub fn max_hp(&self) -> f32 {
        self.config.base_hp + self.config.hp_per_level * self.experience.level as f32
    }

    /// Stub kept so HUD/equipment_visuals code that still wants a
    /// PlayerStats + base_damage formula has a stable callable, even
    /// though server resolves real damage.
    pub fn compute_attack_damage(&self, equip_stats: &PlayerStats) -> f32 {
        self.config.base_damage + equip_stats.flat_damage
    }
}

/// Top-level game state — the single struct that orchestrates all
/// rendering / input / UI. Authoritative gameplay (enemies, hits,
/// loot, transitions) lives in `rift-server`.
pub struct GameState {
    pub world: hecs::World,
    pub rift: RiftState,
    pub player_state: PlayerState,
    pub floor_mgr: FloorManager,
    pub inventory: Inventory,
    pub equipment: Equipment,
    pub inventory_ui: InventoryUI,
    /// New multiplayer inventory panel — operates on
    /// [`Self::mp_inventory`] (the server-mirrored bag) instead of
    /// the legacy engine `Inventory`. Owns the Tab toggle now;
    /// the legacy `inventory_ui` is kept around because
    /// `equip_visuals` still reads `Equipment` for outfit
    /// attachments, but it is force-closed every frame so its
    /// own Tab handler can't fight us for the keypress.
    pub mp_inventory_ui: mp_inventory_ui::MpInventoryUI,
    pub combat_text: CombatTextSystem,
    pub equip_visuals: EquipmentVisuals,
    pub decals: DecalSystem,
    needs_new_floor: bool,
    /// Cached wall colliders for physics (rebuilt on floor change).
    wall_colliders: Vec<(Vec3, rift_engine::ecs::components::Collider)>,
    /// Cached wall AABBs for raycasting (rebuilt on floor change).
    pub wall_aabbs: Vec<rift_engine::physics::Aabb>,
    /// Active placed-ability targeting (if any). Pure visual / input
    /// state — the actual cast is sent to the server.
    targeting: Option<PlacedTargeting>,
    /// Where we are in the per-frame staged init.
    load_phase: LoadPhase,
    monster_load_index: usize,
    /// Eases from 1 -> 0 over ~0.5 s after the player takes damage.
    damage_flash: f32,
    /// `true` once we've triggered the player's death animation.
    player_dying: bool,
    /// True while the player is in the safe hub zone.
    in_hub: bool,
    /// Glowing entry portal placed in the hub. `Some` while we're in
    /// the hub and have a portal to interact with; cleared on every
    /// floor regeneration. Walking close + pressing F sends a
    /// `NetTransitionRequest::EnterRift` to the server.
    hub_portal: Option<HubPortal>,
    /// Server-supplied dungeon seed (matches `LoadFloor.seed`).
    pub net_floor_seed: Option<u64>,
    /// SP-suppressed transition request: drained by the binary and
    /// shipped to the server. Currently nothing inside this crate
    /// fills it, but the slot is kept so the client binary can wire
    /// up UI buttons (return-to-hub, enter-rift) later.
    pub pending_net_request: Option<NetTransitionRequest>,
    /// Ability casts the local player wants to fire. Drained by the
    /// binary each frame and forwarded to the server.
    pub pending_net_casts: Vec<NetCastRequest>,
    /// Channel-end requests the binary must forward to the server
    /// (button release or movement cancel during a hold-to-channel
    /// ability). Drained every frame.
    pub pending_end_channels: Vec<u8>,
    /// Currently-channeling ability, if any. Tracks which slot the
    /// player is holding so input can route releases to the right
    /// `EndChannel` and so the cast clip stays looping.
    pub active_channel: Option<ActiveChannel>,
    /// Active beam / sweep visuals driven by `WorldEvent::ChannelTick`.
    /// Each entry owns a renderer object index that's updated every
    /// tick and zeroed on `ChannelEnd` (or after a brief idle
    /// timeout if ticks stop arriving). Keyed by `(caster, ability)`.
    pub channel_visuals: Vec<ChannelVisual>,
    /// Active ground-loot pillars driven by
    /// `WorldEvent::LootDropped` and the snapshot's
    /// `EntityKind::Loot` rows. Each entry owns a particle emitter
    /// for the rising-pillar visual; the actual `Item` is held so
    /// pickup interaction (later) can hand it to the inventory
    /// without another wire round-trip. Keyed by loot net-id.
    pub loot_drops: Vec<LootDropVisual>,
    /// Loot drops the local player has asked to pick up — drained
    /// by the binary every frame and shipped as
    /// `ClientMsg::PickUpLoot`. Ordered FIFO so a stack of drops
    /// gets claimed in the order the player pressed F.
    pub pending_loot_pickups: Vec<rift_net::NetId>,
    /// Local mirror of the server-authoritative multiplayer
    /// inventory. Items are appended on every `LootClaimed`
    /// confirmation where `claimed_by == our_client_id`. The
    /// server is the persistence authority — this Vec is purely
    /// for instant UI feedback (tooltip, count badge). Cleared
    /// on disconnect; survives floor transitions.
    pub mp_inventory: Vec<rift_game::loot::Item>,
    /// One-shot cosmetic profile advertisement. Set by
    /// `start_with_profile` once character-select completes; the
    /// client binary drains this and pushes it to the `NetClient`
    /// so the server's `Hello` carries the player's actual choice.
    pub pending_profile: Option<character::CharacterProfile>,
    /// Account name selected on the character-select screen.
    /// Drained alongside `pending_profile` by the binary so the
    /// net client can advertise it on the wire.
    pub pending_account_name: Option<String>,
    /// Account name the user just confirmed on the account-entry
    /// screen. Drained by the binary on the next net step so it
    /// can fire `RequestRoster`. Independent from
    /// `pending_account_name`, which is only filled in alongside
    /// `pending_profile` once a character has been picked.
    pub pending_roster_request: Option<String>,
    /// Top-level state (character-select vs playing).
    app_state: AppState,
    /// Owns the character-select screen UI + preview avatar.
    character_select: character_select::CharacterSelect,
    /// Shared cache of bound player-skeleton animation sets, keyed by
    /// gender. Populated lazily on first spawn (local or remote).
    pub anim_cache: character_spawn::AnimLibraryCache,
}

/// Multiplayer-only: a request for the binary to forward to the server.
#[derive(Clone, Copy, Debug)]
pub enum NetTransitionRequest {
    EnterRift,
    ReturnToHub,
}

/// Hub entry portal. Visual + interaction state for the glowing ring
/// the player walks into to start a rift run.
struct HubPortal {
    /// World-space position of the portal centre.
    position: Vec3,
    /// Render-object index of the portal mesh in `renderer.objects`.
    /// We mutate `model_matrix` here every frame to spin it.
    obj_idx: usize,
    /// Particle emitter index for the swirling vortex.
    emitter_idx: usize,
    /// Seconds since the portal was spawned. Drives rotation.
    age: f32,
}

/// Walk-to-interact range. Within this distance the player gets a
/// press-F prompt and the F key triggers an `EnterRift` request.
const HUB_PORTAL_INTERACT_RADIUS: f32 = 2.2;

/// Walk-to-pickup range for ground loot drops. Slightly tighter
/// than the hub portal so the player has to actually step onto
/// the pillar of light. Mirrored on the server as
/// `rift_server::sim::PICKUP_RANGE`; we keep them roughly in
/// sync to avoid client-side prompts that the server would
/// reject.
const LOOT_PICKUP_RADIUS: f32 = 1.8;

/// Multiplayer ability cast request, queued locally and shipped to
/// the server next frame.
#[derive(Clone, Copy, Debug)]
pub struct NetCastRequest {
    pub ability_id: u8,
    pub origin: Vec3,
    pub aim_dir: Vec3,
    pub placed_target: Option<Vec3>,
}

/// Locally-tracked channel state. We keep this client-side so the
/// hold-to-channel input loop can detect button release / movement
/// without round-tripping the server, and so the cast clip stays
/// looping for the channel's expected duration.
#[derive(Clone, Copy, Debug)]
pub struct ActiveChannel {
    /// Wire ability id of the channel ability in flight.
    pub ability_id: u8,
    /// Which action-bar slot the player is holding. Used to decide
    /// which input edge (left-click vs Digit1..5) ends the channel.
    pub slot_index: usize,
    /// Whether the ability cancels on movement input (mirrors the
    /// server flag so the client agrees with the server about when
    /// to send `EndChannel`).
    pub cancel_on_move: bool,
    /// Seconds remaining before we time the channel out locally.
    /// Server is authoritative — this is just so the client tears
    /// down its own state if `WorldEvent::ChannelEnd` is dropped.
    pub remaining: f32,
}

/// Per-channel visual (e.g. Frost Ray's beam). Spawned lazily on
/// the first `ChannelTick` for a given caster+ability and torn down
/// on `ChannelEnd` (or after a short idle timeout if ticks stop).
#[derive(Debug)]
pub struct ChannelVisual {
    /// Channeling caster (network id).
    pub caster: rift_net::NetId,
    /// Wire id of the ability driving the visual.
    pub ability_id: u8,
    /// Most recently reported caster position (chest-height-ish; we
    /// bias the beam upward in `update` so it leaves the hand).
    pub position: Vec3,
    /// Most recently reported aim direction (XZ unit vector with Y=0).
    pub aim: Vec3,
    /// Seconds since the last tick. Used to fade the visual out if
    /// ticks stop arriving without an explicit `ChannelEnd`.
    pub idle: f32,
    /// Renderer object index for the beam mesh, allocated lazily on
    /// the first `update` frame after spawn.
    pub obj_idx: Option<usize>,
    /// Set by [`clear_channel_visual`] when the server sends
    /// `ChannelEnd`. The next `update` frame zeros the mesh's
    /// model matrix and drops the entry.
    pub ending: bool,
    /// Accumulator for the impact-burst cadence (Frost Ray spawns
    /// `frost_impact` at every pierced target every ~0.10 s rather
    /// than every frame, to keep the particle count bounded).
    pub impact_acc: f32,
    /// Per-visual xorshift state for spark-jitter randomness.
    pub rng_state: u64,
}

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

/// Stages of `GameState::load_step`. Floor + outfits + walls happen
/// later, once the player has picked a character.
enum LoadPhase {
    /// Pre-load skinned monster glTFs (one role per call).
    Monsters,
    /// Loading complete; subsequent calls return `Done` immediately.
    Done,
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
            player_state: PlayerState::new(classes::HUNTER),
            floor_mgr: FloorManager::new(),
            inventory: Inventory::new(),
            equipment: Equipment::new(),
            inventory_ui: InventoryUI::new(),
            mp_inventory_ui: mp_inventory_ui::MpInventoryUI::new(),
            combat_text: CombatTextSystem::new(),
            equip_visuals: EquipmentVisuals::new(),
            decals: DecalSystem::new(),
            needs_new_floor: false,
            wall_colliders: Vec::new(),
            wall_aabbs: Vec::new(),
            targeting: None,
            load_phase: LoadPhase::Monsters,
            monster_load_index: 0,
            damage_flash: 0.0,
            player_dying: false,
            in_hub: true,
            hub_portal: None,
            net_floor_seed: None,
            pending_net_request: None,
            pending_net_casts: Vec::new(),
            pending_end_channels: Vec::new(),
            active_channel: None,
            channel_visuals: Vec::new(),
            loot_drops: Vec::new(),
            pending_loot_pickups: Vec::new(),
            mp_inventory: Vec::new(),
            pending_profile: None,
            pending_account_name: None,
            pending_roster_request: None,
            app_state: AppState::CharacterSelect,
            character_select: character_select::CharacterSelect::new(),
            anim_cache: character_spawn::AnimLibraryCache::new(),
        }
    }

    /// Drive one stage of staged initialization.
    pub fn load_step(&mut self, _renderer: &mut Renderer) -> anyhow::Result<LoadStatus> {
        let monster_total = monsters::ALL_ROLES.len();
        let total_steps = monster_total as f32;

        let done_before = match self.load_phase {
            LoadPhase::Monsters => self.monster_load_index,
            LoadPhase::Done => return Ok(LoadStatus::Done),
        };

        let label = match self.load_phase {
            LoadPhase::Monsters => {
                let role = monsters::ALL_ROLES[self.monster_load_index];
                let asset = load_role(role);
                *self.floor_mgr.monsters.slot_mut(role) = asset;
                self.monster_load_index += 1;
                if self.monster_load_index >= monsters::ALL_ROLES.len() {
                    self.load_phase = LoadPhase::Done;
                }
                format!("Loading monster: {:?}", role)
            }
            LoadPhase::Done => return Ok(LoadStatus::Done),
        };

        let done_after = (done_before + 1) as f32;
        let progress = (done_after / total_steps).min(1.0);

        if matches!(self.load_phase, LoadPhase::Done) {
            Ok(LoadStatus::Done)
        } else {
            Ok(LoadStatus::Loading { progress, label })
        }
    }

    fn attach_outfit_pieces(&mut self, renderer: &mut Renderer) {
        let Some(player_id) = self.player_id() else { return };
        let host_table = match self.world.get::<&rift_engine::ecs::components::Skinned>(player_id) {
            Ok(s) => s.mesh.joint_index_by_name.clone(),
            Err(_) => return,
        };
        self.equip_visuals.clear();
        let atts = self.equip_visuals.build_attachments(renderer, &host_table);
        self.world.insert_one(player_id, atts).ok();
    }

    fn load_all_outfit_pieces(&mut self, renderer: &mut Renderer) {
        let Some(player_id) = self.player_id() else { return };
        if let Ok(mut atts) = self.world.get::<&mut rift_engine::ecs::components::SkinnedAttachments>(player_id) {
            while self.equip_visuals.step_load(renderer, &mut atts).is_some() {}
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
        self.player_dying = false;
        self.damage_flash = 0.0;
        self.targeting = None;
        self.hub_portal = None;
        self.decals.clear();
        // `clear_emitters` below kills every particle emitter
        // wholesale, including the ones owned by loot drops, so
        // just drop the bookkeeping Vec — there's nothing left to
        // deactivate individually.
        self.loot_drops.clear();
        self.pending_loot_pickups.clear();
        renderer.particle_system.clear_emitters();
    }

    /// Spawn the hub entry portal mesh + vortex emitter at `pos`.
    /// Records the render slots in `self.hub_portal` so we can spin
    /// the mesh and check the interaction radius each frame.
    fn spawn_hub_portal(&mut self, renderer: &mut Renderer, pos: Vec3) {
        use glam::Mat4;
        use rift_engine::renderer::mesh::Mesh;
        use rift_engine::renderer::particles::{Emitter, EmitterConfig};

        let portal_mesh = Mesh::portal();
        if renderer.add_mesh(&portal_mesh, Mat4::from_translation(pos)).is_err() {
            log::error!("failed to add hub portal mesh");
            return;
        }
        let obj_idx = renderer.objects.len() - 1;
        let emitter = Emitter::new(pos, EmitterConfig::portal_vortex());
        let emitter_idx = renderer.particle_system.add_emitter(emitter);
        self.hub_portal = Some(HubPortal {
            position: pos,
            obj_idx,
            emitter_idx,
            age: 0.0,
        });
    }

    /// Per-frame hub-portal tick: spin the mesh and let the local
    /// player walk up + press F to enqueue an `EnterRift` request.
    /// `emitter_idx` is recorded so a future tweak (e.g. cooldown
    /// pulse) can mutate the emitter; today the emitter spawns its
    /// own particles continuously so we leave it alone.
    fn tick_hub_portal(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        use glam::Mat4;
        use winit::keyboard::KeyCode;

        let Some(portal) = self.hub_portal.as_mut() else { return };
        let _ = portal.emitter_idx;
        portal.age += dt;
        if let Some(obj) = renderer.objects.get_mut(portal.obj_idx) {
            obj.model_matrix = Mat4::from_translation(portal.position)
                * Mat4::from_rotation_y(portal.age * 0.6);
        }

        let Some(player_pos) = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.position)
            .next()
        else {
            return;
        };
        if player_pos.distance(portal.position) <= HUB_PORTAL_INTERACT_RADIUS
            && input.key_just_pressed(KeyCode::KeyF)
            && self.pending_net_request.is_none()
        {
            log::info!("hub portal: requesting EnterRift");
            self.pending_net_request = Some(NetTransitionRequest::EnterRift);
        }
    }

    /// Per-frame ground-loot interaction. Picks the closest drop
    /// inside [`LOOT_PICKUP_RADIUS`] of the local player and, if
    /// the F key was just pressed this frame, queues a
    /// [`ClientMsg::PickUpLoot`] for the binary to forward. The
    /// hub-portal tick runs first; we only fire if the portal
    /// didn't already consume the F press this frame (since both
    /// share the key).
    fn tick_loot_pickup(&mut self, input: &Input) {
        use winit::keyboard::KeyCode;
        let Some((net_id, _)) = self.nearest_lootable_drop() else {
            return;
        };
        if input.key_just_pressed(KeyCode::KeyF) {
            // De-dupe: one in-flight request per drop.
            if !self.pending_loot_pickups.contains(&net_id) {
                self.pending_loot_pickups.push(net_id);
            }
        }
    }

    /// Closest loot drop inside [`LOOT_PICKUP_RADIUS`] of the local
    /// player. Used by [`Self::tick_loot_pickup`] to pick a target
    /// for the F press, and by the HUD to render a "Press F: <item>"
    /// tooltip. Returns the drop's `NetId` and the squared distance.
    fn nearest_lootable_drop(&self) -> Option<(rift_net::NetId, f32)> {
        if self.loot_drops.is_empty() {
            return None;
        }
        let player_pos = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.position)
            .next()?;
        let mut best: Option<(rift_net::NetId, f32)> = None;
        for drop in &self.loot_drops {
            let d2 = (drop.position - player_pos).length_squared();
            if d2 > LOOT_PICKUP_RADIUS * LOOT_PICKUP_RADIUS {
                continue;
            }
            if best.map_or(true, |(_, b)| d2 < b) {
                best = Some((drop.net_id, d2));
            }
        }
        best
    }

    /// Tear down the visual for a loot drop that was claimed
    /// (either by the local player or another). If `add_to_local`
    /// is set, the rolled item is also appended to our local
    /// inventory \u2014 the server is the persistence authority,
    /// but the local mirror lets the UI react instantly.
    pub fn resolve_loot_claim(
        &mut self,
        renderer: &mut Renderer,
        loot: rift_net::NetId,
        add_to_local: bool,
    ) {
        let idx = self.loot_drops.iter().position(|d| d.net_id == loot);
        let Some(idx) = idx else { return };
        let drop = self.loot_drops.swap_remove(idx);
        renderer.particle_system.deactivate_emitter(drop.pillar_emitter);
        renderer.particle_system.deactivate_emitter(drop.base_emitter);
        if add_to_local {
            log::info!(
                "loot picked up: {} (item-level {})",
                drop.item.display_name(),
                drop.item.ilvl
            );
            // Mirror the server's authoritative inventory so the UI
            // can react instantly. The server's `try_pickup_loot`
            // has already pushed the same item onto its own
            // `ServerPlayer.inventory`, so the two stay in sync as
            // long as the wire confirmation arrives.
            self.mp_inventory.push(drop.item);
            log::debug!("inventory: {} item(s) total", self.mp_inventory.len());
        }
    }

    pub fn shutdown(&mut self, renderer: &mut Renderer) {
        unsafe { renderer.ash_device().device_wait_idle().ok(); }
        let device = renderer.ash_device().clone();
        let allocator = renderer.allocator_arc();
        self.equip_visuals.cleanup_gpu(&device, &allocator);
        self.floor_mgr.monsters.cleanup_gpu(&device, &allocator);
        self.floor_mgr.props.cleanup_gpu(&device, &allocator);
        self.floor_mgr.env.cleanup_gpu(&device, &allocator);
    }

    /// Apply a server-driven floor transition.
    pub fn apply_net_transition(&mut self, renderer: &mut Renderer, index: u32) {
        self.reset_for_regeneration(renderer);
        if index == 0 {
            self.in_hub = true;
            self.rift = RiftState::new(1);
            match self.floor_mgr.generate_hub(
                &mut self.world,
                renderer,
                &self.player_state,
                &mut self.anim_cache,
            ) {
                Ok(portal_pos) => self.spawn_hub_portal(renderer, portal_pos),
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
                self.net_floor_seed,
            ) {
                log::error!("Net floor regeneration failed: {}", e);
            }
        }
        self.attach_outfit_pieces(renderer);
        self.load_all_outfit_pieces(renderer);
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

        self.rift.timer += if self.in_hub { 0.0 } else { dt };

        // Hub portal: spin the mesh and watch for the local player
        // walking up + pressing F to start a rift run.
        self.tick_hub_portal(renderer, input, dt);

        // Ground loot: hover prompt + F-to-pick.
        self.tick_loot_pickup(input);

        // ECS systems
        let action_cfg = PlayerActionConfig::default();
        let accept_input = !self.player_dying;
        player_action_pre_system(&mut self.world, input, dt, &action_cfg, accept_input);
        player_input_system(&mut self.world, input, dt);
        movement_system(&mut self.world, dt);
        player_action_post_system(&mut self.world, &action_cfg);
        collision_system(&mut self.world, &self.wall_colliders);

        // Loot hover + click-to-pickup (placeholder: empty Inventory/Equipment).
        let (_sw, _sh) = renderer.screen_size();
        // Suppress the legacy engine UI \u2014 its Tab handler would
        // fight the new MP panel and its data sources are always
        // empty in MP mode anyway. We still pass it through
        // `equip_visuals.sync` later, hence we keep the struct
        // alive instead of removing it outright.
        self.inventory_ui.open = false;
        let _ui_consumed = self.mp_inventory_ui.update(input);

        // Ability-based combat (sends cast requests to the server).
        self.player_state.abilities.tick_all(dt);
        if !self.player_dying && !self.in_hub {
            self.tick_combat(input, renderer, dt);
        }

        // Catch-all death detection. HP is driven by snapshot deltas
        // applied to the local Health component by the net layer.
        if !self.player_dying {
            let dead = self.world.query::<(&Health, &Player, &LocalPlayer)>().iter()
                .any(|(_, (h, _, _))| h.is_dead());
            if dead {
                self.trigger_player_death();
            }
        }

        // Tick combat text
        self.combat_text.tick(dt);

        // Despawn dead entities (animation-finished kills, etc.)
        let _kills = despawn_system(&mut self.world, renderer);

        // Render sync
        render_sync_system(&self.world, renderer);

        locomotion_anim_system(&mut self.world);
        enemy_anim_system(&mut self.world, dt);

        // Spell-cast state machine: advances the upper-body cast layer.
        // The returned `fire_events` list contains one entry per
        // projectile that should leave the player's hand *now* (i.e.
        // the wind-up just finished). For our local player we
        // forward each as a `CastAbility` to the server with the
        // hand's world position as the spawn origin, so server-side
        // projectiles visually emerge from the casting hand instead
        // of the chest. Remote casts are driven entirely by server
        // snapshots, so we ignore fires for non-local entities.
        let cast_fires = cast_advance_system(&mut self.world, dt);
        for (entity, aim, _damage) in cast_fires {
            // Only forward fires for the local player.
            if self.world.get::<&LocalPlayer>(entity).is_err() {
                continue;
            }
            // Pull the in-flight ability id off the SpellCast layer.
            let ability_id = self
                .world
                .get::<&rift_engine::ecs::components::SpellCast>(entity)
                .ok()
                .and_then(|c| c.pending_ability.as_ref().map(|a| a.wire_id));
            let Some(ability_id) = ability_id else { continue };
            // Compute the hand position in world space, falling
            // back to the entity's transform if no hand joint
            // resolved on this rig. `joint_worlds` was last
            // refreshed by the previous frame's `skinning_system`,
            // which is close enough — the hand is already at its
            // apex by the time the wind-up clip ends.
            let origin = {
                use rift_engine::ecs::components::Skinned;
                let mut q = self
                    .world
                    .query_one::<(&Transform, &Player, Option<&Skinned>)>(entity)
                    .ok();
                let computed = q
                    .as_mut()
                    .and_then(|q| q.get())
                    .and_then(|(t, p, s)| {
                        if p.hand_joint == u32::MAX {
                            return Some(t.position);
                        }
                        let s = s?;
                        let m = s.joint_worlds.get(p.hand_joint as usize)?;
                        let local = m.col(3).truncate();
                        Some(t.matrix().transform_point3(local))
                    });
                computed.unwrap_or(Vec3::ZERO)
            };
            self.pending_net_casts.push(NetCastRequest {
                ability_id,
                origin,
                aim_dir: aim,
                placed_target: None,
            });
        }

        // Sync modular outfit visibility to the equipment state.
        if let Some(player_id) = self.player_id() {
            if let Ok(mut atts) = self.world.get::<&mut rift_engine::ecs::components::SkinnedAttachments>(player_id) {
                let hide_base = self.equip_visuals.sync(&self.equipment, &mut atts, renderer);
                atts.hide_base = hide_base;
            }
        }

        skinning_system(&mut self.world, renderer, dt);
        self.decals.update(dt, renderer);

        // Channel beam visuals (Frost Ray etc.) — driven by reliable
        // `WorldEvent::ChannelTick` events buffered into
        // `self.channel_visuals` by the binary's event loop.
        self.tick_channel_visuals(renderer, dt);

        // Equipment visual sync (other gameplay state, like the held
        // weapon's world position) still happens after skinning.
        let player_pos = self.world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.position)
            .next()
            .unwrap_or(Vec3::ZERO);

        let arm_aim = Self::cursor_aim_dir(input, renderer, player_pos);
        if let Some(player_id) = self.player_id() {
            if let Ok(mut p) = self.world.get::<&mut rift_engine::ecs::components::Player>(player_id) {
                p.aim_dir = arm_aim;
            }
        }

        camera_follow_system(&self.world, renderer, input, &self.wall_aabbs);
        renderer.particle_system.tick(dt);

        if self.damage_flash > 0.0 {
            self.damage_flash = (self.damage_flash - dt * 2.2).max(0.0);
        }

        // HUD
        renderer.overlay_batch.clear();
        let (sw, sh) = renderer.screen_size();
        let stats = self.equipment.total_stats();
        if self.damage_flash > 0.001 {
            hud::render_damage_flash(&mut renderer.overlay_batch, self.damage_flash, sw, sh);
        }
        hud::render_hud(
            &mut renderer.overlay_batch,
            &self.world,
            &self.rift,
            &self.player_state,
            &self.equipment,
            sw,
            sh,
            stats.max_hp_bonus,
            self.in_hub,
        );
        hud::render_ability_bar(
            &mut renderer.overlay_batch,
            &self.player_state.abilities,
            input.mouse_pos(),
            sw,
            sh,
        );
        hud::render_enemy_health_bars(
            &mut renderer.overlay_batch,
            &self.world,
            renderer.camera.view_projection(),
            sw,
            sh,
        );
        if !self.in_hub {
            hud::render_boss_arrow(
                &mut renderer.overlay_batch,
                &self.world,
                renderer.camera.view_projection(),
                sw,
                sh,
            );
        }
        let player_facing = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer)>()
            .iter()
            .map(|(_, (t, _, _))| t.rotation * Vec3::Z)
            .next()
            .unwrap_or(Vec3::Z);
        hud::render_minimap(
            &mut renderer.overlay_batch,
            &self.world,
            &self.floor_mgr.nav_grid,
            player_facing,
            self.hub_portal.as_ref().map(|p| p.position),
            sw,
            sh,
        );
        self.combat_text.render(
            &mut renderer.overlay_batch,
            renderer.camera.view_projection(),
            sw,
            sh,
        );
        self.mp_inventory_ui.render(
            &mut renderer.overlay_batch,
            &self.mp_inventory,
            input,
            sw,
            sh,
        );

        // Loot pickup tooltip: when the local player is within
        // range of a dropped item, show "PRESS [F]: <item-name>"
        // so the input is discoverable. Tier color matches the
        // pillar / beam color the player sees in-world.
        if let Some((net_id, _)) = self.nearest_lootable_drop() {
            if let Some(drop) = self.loot_drops.iter().find(|d| d.net_id == net_id) {
                let c = drop.item.rarity.color();
                let prompt = format!("PRESS [F]: {}", drop.item.display_name());
                hud::render_loot_prompt(
                    &mut renderer.overlay_batch,
                    &prompt,
                    [c[0], c[1], c[2], 1.0],
                    sw,
                    sh,
                );
            }
        }

        // Mark needs_new_floor as consumed (kept for future use,
        // but no SP path sets it any more).
        if self.needs_new_floor {
            self.needs_new_floor = false;
        }
    }

    /// Tick the character-select screen.
    fn update_character_select(&mut self, renderer: &mut Renderer, input: &Input, dt: f32) {
        renderer.overlay_batch.clear();
        let action = self
            .character_select
            .update(&mut self.world, renderer, input, dt);

        skinning_system(&mut self.world, renderer, dt);

        let (sw, sh) = renderer.screen_size();
        self.character_select.render(&mut renderer.overlay_batch, sw, sh);

        match action {
            character_select::SelectAction::None => {}
            character_select::SelectAction::AccountConfirmed { name } => {
                self.pending_roster_request = Some(name);
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
            "Entering world as '{}' on account '{}' ({:?} {:?})",
            profile.name, account_name, profile.gender, profile.class,
        );
        self.player_state = PlayerState::with_profile(
            profile.class,
            profile.gender,
            profile.name.clone(),
        );
        // Hand the profile + account to the binary so it can
        // advertise them on the wire. In SP this is just dropped.
        self.pending_profile = Some(profile);
        self.pending_account_name = Some(account_name);
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
                    Ok(portal_pos) => self.spawn_hub_portal(renderer, portal_pos),
                    Err(e) => log::error!("Hub generation failed: {}", e),
                }
                ("Generating hub…", Some(EnterPhase::AttachOutfits))
            }
            EnterPhase::AttachOutfits => {
                self.attach_outfit_pieces(renderer);
                ("Preparing outfits…", Some(EnterPhase::LoadOutfits))
            }
            EnterPhase::LoadOutfits => {
                let player_id = self.player_id();
                let mut still_loading = false;
                if let Some(pid) = player_id {
                    if let Ok(mut atts) = self
                        .world
                        .get::<&mut rift_engine::ecs::components::SkinnedAttachments>(pid)
                    {
                        for _ in 0..2 {
                            if self.equip_visuals.step_load(renderer, &mut atts).is_none() {
                                break;
                            }
                            still_loading = true;
                        }
                        if !still_loading
                            && self.equip_visuals.loaded_pieces()
                                < self.equip_visuals.total_pieces()
                        {
                            still_loading = true;
                        }
                    }
                }
                let next = if still_loading {
                    Some(EnterPhase::LoadOutfits)
                } else {
                    Some(EnterPhase::RebuildWalls)
                };
                ("Loading outfits…", next)
            }
            EnterPhase::RebuildWalls => {
                self.rebuild_wall_caches();
                ("Finalizing…", None)
            }
        };

        let progress = compute_enter_progress(phase, &self.equip_visuals);
        draw_world_loading_overlay(renderer, progress, label);

        match next {
            Some(p) => self.app_state = AppState::EnteringWorld(p),
            None => self.app_state = AppState::Playing,
        }
    }

    /// Per-frame update for active channel beam visuals.
    ///
    /// For each entry in `self.channel_visuals` we lazily allocate a
    /// stretched beam mesh on the renderer the first time we see it,
    /// then on subsequent frames we update its model matrix so the
    /// beam tracks the caster's hand and aim direction. Walls clip
    /// the beam length via a raycast against `self.wall_aabbs`.
    /// Idle entries (no tick for ~0.4 s) and entries flagged
    /// `ending` get their model matrix zeroed and are dropped.
    fn tick_channel_visuals(&mut self, renderer: &mut Renderer, dt: f32) {
        use glam::Mat4;
        use rift_engine::physics::{self, Ray};
        use rift_engine::renderer::mesh::Mesh;
        use rift_engine::renderer::particles::{Emitter, EmitterConfig};

        // Visuals (frost-ray colour). Other channel kinds can branch
        // here later; for now Whirlwind doesn't need a beam.
        const FROST_RAY_WIRE_ID: u8 = 6;
        const BEAM_WIDTH: f32 = 0.18;
        const BEAM_HAND_OFFSET: f32 = 1.25; // chest fallback when no hand joint
        const IDLE_TIMEOUT: f32 = 0.4;
        const IMPACT_INTERVAL: f32 = 0.10; // 10 Hz cold-burst cadence

        // Pull the local player's live transform + aim, and the
        // *world-space* position of its right-hand joint if the
        // skinning pass has produced one this frame. Beam visuals
        // for our own channel anchor at the hand for accuracy
        // (server tick rate of ~5 Hz would otherwise look choppy
        // *and* off-anatomy).
        use rift_engine::ecs::components::Skinned;
        let local_live: Option<(Vec3, Vec3, Option<Vec3>)> = self
            .world
            .query::<(&Transform, &Player, &LocalPlayer, Option<&Skinned>)>()
            .iter()
            .map(|(_, (t, p, _, s))| {
                let hand = s.and_then(|s| {
                    if p.hand_joint == u32::MAX { return None; }
                    let idx = p.hand_joint as usize;
                    s.joint_worlds.get(idx).map(|m| {
                        let local = m.col(3).truncate();
                        // joint_worlds are mesh-local; lift into
                        // world via the entity transform.
                        t.matrix().transform_point3(local)
                    })
                });
                (t.position, p.aim_dir, hand)
            })
            .next();
        let local_active_ability = self.active_channel.map(|c| c.ability_id);

        // Snapshot enemy positions for client-side beam-corridor
        // hit detection (so we can spawn impact particles on every
        // pierced target). Mirrors the server-side logic in
        // `sim::channel::collect_hits` for `ChannelEffect::Beam`.
        use rift_engine::ecs::components::Enemy;
        let enemy_positions: Vec<Vec3> = self
            .world
            .query::<(&Transform, &Enemy)>()
            .iter()
            .map(|(_, (t, _))| t.position)
            .collect();

        // Drain a temporary list of indices to drop after the loop so
        // we can mutate `channel_visuals` while still holding `&mut
        // renderer`.
        let mut drop_indices: Vec<usize> = Vec::new();

        for (i, vis) in self.channel_visuals.iter_mut().enumerate() {
            let (beam_range, beam_corridor_width, pierce_targets) =
                match rift_game::abilities::lookup(vis.ability_id).map(|d| d.kind) {
                    Some(rift_game::abilities::AbilityKind::Channel {
                        effect: rift_game::abilities::ChannelEffect::Beam {
                            range, width, pierce_targets, ..
                        },
                        ..
                    }) => (range, width, pierce_targets),
                    _ => (0.0, 0.0, 0),
                };

            // Hide-and-drop path: ending flag set by `clear_channel_visual`
            // or idle timeout exceeded.
            vis.idle += dt;
            let expired = vis.ending || vis.idle > IDLE_TIMEOUT;

            // Resolve the caster: prefer matching to a known
            // remote-player avatar by net id; if no remote matches
            // (and we're channeling locally) treat the visual as
            // belonging to us. This keeps remote and local beams
            // visually consistent even if both happen at once.
            use rift_engine::ecs::components::{RemotePlayer, Skinned};
            let remote_data = self
                .world
                .query::<(&Transform, &Player, &RemotePlayer, Option<&Skinned>)>()
                .iter()
                .find(|(_, (_, _, rp, _))| rp.net_id == vis.caster.0)
                .map(|(_, (t, p, _, s))| {
                    let hand = s.and_then(|s| {
                        if p.hand_joint == u32::MAX { return None; }
                        let idx = p.hand_joint as usize;
                        s.joint_worlds.get(idx).map(|m| {
                            let local = m.col(3).truncate();
                            t.matrix().transform_point3(local)
                        })
                    });
                    (t.position, p.aim_dir, hand)
                });
            let is_local = remote_data.is_none()
                && local_active_ability == Some(vis.ability_id);
            let mut hand_override: Option<Vec3> = None;
            if let Some((pos, aim, hand)) = remote_data {
                // Remote caster: anchor the beam to their hand
                // joint and pull pos/aim from the live (interpolated)
                // transform instead of the stale `ChannelTick`
                // payload, so the beam doesn't visibly trail the
                // body while they move.
                vis.position = pos;
                if aim.length_squared() > 1e-6 {
                    vis.aim = Vec3::new(aim.x, 0.0, aim.z).normalize_or_zero();
                }
                hand_override = hand;
            } else if is_local {
                if let Some((pos, aim, hand)) = local_live {
                    vis.position = pos;
                    if aim.length_squared() > 1e-6 {
                        vis.aim = Vec3::new(aim.x, 0.0, aim.z).normalize_or_zero();
                    }
                    hand_override = hand;
                    // Heartbeat the idle timer so we don't fade out
                    // between server ticks.
                    vis.idle = 0.0;
                }
            }
            let _ = is_local;

            // Skip non-beam channels (Whirlwind etc.); just let them
            // age out without spawning a mesh.
            if beam_range <= 0.0 || vis.ability_id != FROST_RAY_WIRE_ID {
                if expired {
                    drop_indices.push(i);
                }
                continue;
            }

            // Lazy mesh alloc on first frame.
            if vis.obj_idx.is_none() && !expired {
                let mesh = Mesh::light_beam([0.55, 0.85, 1.0]);
                if renderer.add_mesh(&mesh, Mat4::ZERO).is_ok() {
                    vis.obj_idx = Some(renderer.objects.len() - 1);
                }
            }

            let Some(obj_idx) = vis.obj_idx else { continue };
            if obj_idx >= renderer.objects.len() {
                continue;
            }

            if expired {
                renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                drop_indices.push(i);
                continue;
            }

            // Compute clipped beam length (stops at first wall).
            // Anchor at the right-hand joint when we have one;
            // otherwise fall back to a chest-height offset above
            // the caster's transform.
            let origin = hand_override
                .unwrap_or_else(|| vis.position + Vec3::Y * BEAM_HAND_OFFSET);
            let dir = if vis.aim.length_squared() > 1e-6 {
                vis.aim.normalize()
            } else {
                Vec3::Z
            };
            let ray = Ray { origin, direction: dir };
            let length = match physics::raycast(&ray, beam_range, &self.wall_aabbs) {
                Some(hit) => hit.distance.max(0.05),
                None => beam_range,
            };

            // `Mesh::light_beam` is built along +Y with height=1 and
            // base radius ~BEAM_WIDTH baked in. Rotate +Y to `dir`,
            // then scale Y by the clipped length and X/Z so the
            // baked-in width comes out roughly `BEAM_WIDTH`.
            let from = Vec3::Y;
            let to = dir;
            let rot = if (from - to).length_squared() < 1e-6 {
                glam::Quat::IDENTITY
            } else if (from + to).length_squared() < 1e-6 {
                // Antiparallel: pick any perpendicular axis.
                glam::Quat::from_axis_angle(Vec3::X, std::f32::consts::PI)
            } else {
                glam::Quat::from_rotation_arc(from, to)
            };
            let scale = Vec3::new(BEAM_WIDTH / 0.12, length, BEAM_WIDTH / 0.12);
            let model =
                Mat4::from_translation(origin) * Mat4::from_quat(rot) * Mat4::from_scale(scale);
            renderer.objects[obj_idx].model_matrix = model;

            // ---- Beam pulse: tiny one-shot spark emitter at a
            // random point along the beam each frame. The sparks
            // are aimed forward + a touch of cone spread so they
            // *flow* along the beam, giving it motion.
            let r1 = vis_rand_f32(&mut vis.rng_state);
            let r2 = vis_rand_f32(&mut vis.rng_state);
            let r3 = vis_rand_f32(&mut vis.rng_state);
            let t_along = 0.05 + r1 * 0.90; // bias slightly off the hand
            // Sub-pixel lateral wobble so sparks don't all sit on the axis.
            let perp1 = if dir.y.abs() < 0.99 {
                dir.cross(Vec3::Y).normalize()
            } else {
                dir.cross(Vec3::X).normalize()
            };
            let perp2 = dir.cross(perp1);
            let wobble = (r2 - 0.5) * BEAM_WIDTH * 1.2;
            let wobble2 = (r3 - 0.5) * BEAM_WIDTH * 1.2;
            let spark_pos = origin + dir * (length * t_along) + perp1 * wobble + perp2 * wobble2;
            renderer.particle_system.add_emitter(Emitter::new(
                spark_pos,
                EmitterConfig::frost_beam_spark(dir),
            ));

            // ---- Impact bursts at every pierced enemy + the
            // terminal point. Cadence-gated so we don't spew
            // hundreds of particles per second.
            vis.impact_acc += dt;
            if vis.impact_acc >= IMPACT_INTERVAL {
                vis.impact_acc = 0.0;

                // Replicate the server's beam-corridor hit math so
                // visuals match what's actually being damaged.
                // Right vector in XZ plane (rotate aim 90°).
                let right = Vec3::new(dir.z, 0.0, -dir.x);
                let mut hits: Vec<(f32, Vec3)> = Vec::new();
                for ep in &enemy_positions {
                    let to = Vec3::new(ep.x - origin.x, 0.0, ep.z - origin.z);
                    let along = to.dot(dir);
                    if along < 0.0 || along > length {
                        continue;
                    }
                    if to.dot(right).abs() > beam_corridor_width {
                        continue;
                    }
                    hits.push((along, *ep));
                }
                hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                let cap = (pierce_targets as usize).saturating_add(1);
                hits.truncate(cap);

                for (_along, pos) in &hits {
                    // Centre on the enemy's torso, not their feet.
                    let burst_pos = *pos + Vec3::Y * 0.9;
                    renderer.particle_system.add_emitter(Emitter::new(
                        burst_pos,
                        EmitterConfig::frost_impact(),
                    ));
                }

                // Terminal-point burst: when the beam clipped a wall
                // (length < beam_range) or pierced through everything
                // and reached max range, sparkle at the tip.
                let clipped = length + 0.01 < beam_range;
                if clipped || hits.len() < cap {
                    let tip = origin + dir * length;
                    renderer.particle_system.add_emitter(Emitter::new(
                        tip,
                        EmitterConfig::frost_impact(),
                    ));
                }
            }
        }

        // Remove expired entries (back-to-front so earlier indices
        // stay valid).
        for &i in drop_indices.iter().rev() {
            self.channel_visuals.swap_remove(i);
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

        let aim_dir = Self::cursor_aim_dir(input, renderer, player_pos);

        // ─── Placed ability targeting mode ─────────────────────────────────
        if self.targeting.is_some() {
            if let Some(cursor_pos) = Self::cursor_world_pos(input, renderer, 0.0) {
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
                if let Some(cursor_pos) = Self::cursor_world_pos(input, renderer, 0.0) {
                    let targeting = self.targeting.take().unwrap();
                    if let Some(obj_idx) = targeting.indicator_obj {
                        if obj_idx < renderer.objects.len() {
                            renderer.objects[obj_idx].model_matrix = Mat4::ZERO;
                        }
                    }
                    self.pending_net_casts.push(NetCastRequest {
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
        if let Some(active) = self.active_channel {
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
                self.pending_end_channels.push(active.ability_id);
                self.active_channel = None;
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
                self.active_channel = if a.remaining > 0.0 { Some(a) } else { None };
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
                    let initial_pos = Self::cursor_world_pos(input, renderer, 0.0)
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

                // Server is authoritative. For projectile abilities we
                // *defer* the cast send until the wind-up animation
                // finishes (see `cast_advance_system` → drained
                // below in `update`), so the projectile spawns from
                // the player's hand at the moment of the Shoot clip.
                // Channels and instant abilities still push immediately.
                let def = rift_game::abilities::lookup(ability_clone.wire_id)
                    .map(|d| d.kind);
                let is_channel = matches!(
                    def,
                    Some(rift_game::abilities::AbilityKind::Channel { .. })
                );
                let is_projectile = matches!(
                    def,
                    Some(rift_game::abilities::AbilityKind::Projectiles { .. })
                );
                let placed_target = if let TargetingMode::Placed { .. } = ability_clone.targeting {
                    Self::cursor_world_pos(input, renderer, 0.0)
                } else {
                    None
                };
                if !is_projectile {
                    self.pending_net_casts.push(NetCastRequest {
                        ability_id: ability_clone.wire_id,
                        origin: player_pos,
                        aim_dir,
                        placed_target,
                    });
                }
                let _ = is_channel;

                // If this is a channel ability, latch the local
                // active-channel state so subsequent frames can
                // detect button release and movement.
                if let Some(def) = rift_game::abilities::lookup(ability_clone.wire_id) {
                    if let rift_game::abilities::AbilityKind::Channel { duration, cancel_on_move, .. } = def.kind {
                        self.active_channel = Some(ActiveChannel {
                            ability_id: ability_clone.wire_id,
                            slot_index: i,
                            cancel_on_move,
                            // Add a small grace period so a release
                            // event slightly after the server's
                            // expiry doesn't fire a stale EndChannel.
                            remaining: duration + 0.25,
                        });
                    }
                }

                // Local visual feedback. The server still owns the
                // damage / projectile spawn — we just play the cast
                // animation + any client-side particles immediately
                // so the input feels responsive.
                trigger_local_cast(&ability_clone, aim_dir, player_pos, &mut self.world, renderer, &self.player_state.talents);
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

    /// Triggered when the snapshot brings local Health to zero. Plays
    /// the death animation and freezes input. Server-authoritative
    /// respawn happens via a follow-up `LoadFloor`.
    fn trigger_player_death(&mut self) {
        use rift_engine::animation::Animator;
        use rift_engine::ecs::components::{
            AnimationSet, Player, PlayerAction, SpellCast, Velocity,
        };

        self.player_dying = true;
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

    /// Compute the world position where the cursor ray hits a ground plane at the given Y.
    fn cursor_world_pos(input: &Input, renderer: &Renderer, ground_y: f32) -> Option<Vec3> {
        let (mx, my) = input.mouse_pos();
        let [w, h] = renderer.window_extent();
        if w == 0 || h == 0 {
            return None;
        }

        let ndc_x = (mx / w as f32) * 2.0 - 1.0;
        let ndc_y = (my / h as f32) * 2.0 - 1.0;

        let inv_vp = (renderer.camera.projection_matrix() * renderer.camera.view_matrix()).inverse();
        let near_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 0.0));
        let far_point = inv_vp.project_point3(glam::Vec3::new(ndc_x, ndc_y, 1.0));
        let ray_dir = (far_point - near_point).normalize();

        if ray_dir.y.abs() < 1e-6 {
            return None;
        }
        let t = (ground_y - near_point.y) / ray_dir.y;
        Some(near_point + ray_dir * t)
    }

    /// Compute a horizontal aim direction from the cursor position to the ground plane.
    fn cursor_aim_dir(input: &Input, renderer: &Renderer, player_pos: Vec3) -> Vec3 {
        if let Some(hit) = Self::cursor_world_pos(input, renderer, player_pos.y) {
            let delta = hit - player_pos;
            let flat = Vec3::new(delta.x, 0.0, delta.z);
            if flat.length_squared() > 0.01 {
                return flat.normalize();
            }
        }
        Vec3::NEG_Z
    }
}

/// Tiny xorshift64 → f32 in [0, 1). Used by `tick_channel_visuals`
/// for spark-jitter randomness without pulling in `rand`.
fn vis_rand_f32(state: &mut u64) -> f32 {
    let mut x = if *state == 0 { 0x9E37_79B9_7F4A_7C15 } else { *state };
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 40) as u32) as f32 / (1u32 << 24) as f32
}

/// Local cast feedback. The server still owns damage / projectile
/// spawn — this just plays the cast animation + any client-side
/// particles immediately so the input feels responsive.
///
/// Strategy:
/// - Projectile abilities (`SpawnProjectiles`) trigger the upper-body
///   `SpellCast` FSM on the player's skeleton via `cast.begin`.
///   Handled by `cast_advance_system`.
/// - AoE / movement abilities (`SpawnAoeZone`, `SetPlayerAction`)
///   route through `execute_ability_instant`, which spawns the
///   client-side particles and (for movement abilities) sets
///   `Player.action` + cross-fades the matching one-shot clip.
fn trigger_local_cast(
    ability: &Ability,
    aim_dir: Vec3,
    origin: Vec3,
    world: &mut hecs::World,
    renderer: &mut Renderer,
    talents: &TalentTree,
) {
    use rift_engine::ecs::components::{LocalPlayer, SpellCast};

    let has_projectile = ability
        .effects
        .iter()
        .any(|e| matches!(e, rift_game::abilities::AbilityEffect::SpawnProjectiles { .. }));
    // Channeled abilities have empty `effects` (the server drives
    // the actual ticks) but still want the cast pose. Detect by
    // looking at the wire-side `AbilityKind`.
    let is_channeled = matches!(
        rift_game::abilities::lookup(ability.wire_id).map(|d| d.kind),
        Some(rift_game::abilities::AbilityKind::Channel { .. })
    );

    if has_projectile || is_channeled {
        let pid = world
            .query::<(&Player, &LocalPlayer)>()
            .iter()
            .map(|(e, _)| e)
            .next();
        if let Some(pid) = pid {
            if let Ok(mut cast) = world.get::<&mut SpellCast>(pid) {
                if is_channeled {
                    cast.begin_channel(ability.clone(), aim_dir);
                } else {
                    cast.begin(ability.clone(), aim_dir, 0.0);
                }
            }
        }
    } else {
        rift_engine::combat::execute_ability_instant(
            ability,
            origin,
            aim_dir,
            0.0,
            Some(talents),
            world,
            renderer,
        );
    }
}

/// Spawn the AoE-zone particle visual for an ability on the local
/// renderer. Driven from the server's `WorldEvent::AbilityCast` so
/// every observer (including the caster) sees the same effect at
/// the same authoritative position; the local placement path
/// otherwise returns out of `tick_combat` without spawning a
/// particle emitter, leaving the caster with no visual feedback.
///
/// No-op for abilities without a `SpawnAoeZone` effect, or when the
/// effect's `visual` is `None`.
pub fn spawn_ability_aoe_visual(
    renderer: &mut Renderer,
    ability: &Ability,
    origin: Vec3,
    aim_dir: Vec3,
    target: Option<Vec3>,
) {
    use rift_engine::combat::emitter_for_preset;
    for effect in ability.effects {
        if let rift_game::abilities::AbilityEffect::SpawnAoeZone {
            visual,
            visual_y,
            ..
        } = effect
        {
            let Some(preset) = visual else { continue };
            // Match `AbilityCtx::placed_position`: use `target` if
            // the cast was placed (e.g. Rain of Fire), otherwise
            // fall back to a forward offset along aim from the
            // caster origin.
            let pos = target.unwrap_or(origin + aim_dir * 5.0)
                + Vec3::new(0.0, *visual_y, 0.0);
            renderer
                .particle_system
                .add_emitter(emitter_for_preset(*preset, pos));
        }
    }
}

/// Trigger the upper-body spell-cast layer on a *remote* avatar.
/// Used by the binary when a `WorldEvent::AbilityCast` arrives for a
/// caster that isn't us. Only projectile abilities are visualised
/// here today — server already drives the rest through snapshots.
pub fn trigger_remote_cast(
    world: &mut hecs::World,
    entity: hecs::Entity,
    ability: &Ability,
    aim_dir: Vec3,
) {
    use rift_engine::ecs::components::SpellCast;

    let has_projectile = ability
        .effects
        .iter()
        .any(|e| matches!(e, rift_game::abilities::AbilityEffect::SpawnProjectiles { .. }));
    let is_channeled = matches!(
        rift_game::abilities::lookup(ability.wire_id).map(|d| d.kind),
        Some(rift_game::abilities::AbilityKind::Channel { .. })
    );
    if !has_projectile && !is_channeled {
        return;
    }
    if let Ok(mut cast) = world.get::<&mut SpellCast>(entity) {
        if is_channeled {
            cast.begin_channel(ability.clone(), aim_dir);
        } else {
            cast.begin(ability.clone(), aim_dir, 0.0);
        }
    }
}

/// Update or create a per-channel visual entry from a server
/// `ChannelTick`. Called by the binary when reliable events arrive.
/// The actual mesh is allocated lazily inside `GameState::update`,
/// where we have access to the renderer.
pub fn push_channel_visual(
    state: &mut GameState,
    caster: rift_net::NetId,
    ability_id: u8,
    position: Vec3,
    aim: Vec3,
) {
    if let Some(entry) = state
        .channel_visuals
        .iter_mut()
        .find(|v| v.caster == caster && v.ability_id == ability_id)
    {
        entry.position = position;
        entry.aim = aim;
        entry.idle = 0.0;
        return;
    }
    state.channel_visuals.push(ChannelVisual {
        caster,
        ability_id,
        position,
        aim,
        idle: 0.0,
        obj_idx: None,
        ending: false,
        impact_acc: 0.0,
        rng_state: 0x9E37_79B9_7F4A_7C15 ^ (caster.0 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9),
    });
}

/// Hide and forget the visual for a channel that just ended.
/// Zeroing the model matrix removes it from the scene; the renderer
/// slot is recycled implicitly the next time we add a beam.
pub fn clear_channel_visual(
    state: &mut GameState,
    caster: rift_net::NetId,
    ability_id: u8,
) {
    if let Some(entry) = state
        .channel_visuals
        .iter_mut()
        .find(|v| v.caster == caster && v.ability_id == ability_id)
    {
        // Defer the actual hide-and-drop to `update`, which has
        // access to the renderer to zero the mesh's model matrix.
        entry.ending = true;
    }
}

/// One ground-loot drop the local client is rendering. Owned by
/// [`GameState::loot_drops`]; constructed by
/// [`spawn_loot_drop_visual`]; consumed by the (future) pickup
/// path which will translate the held [`Item`] into an inventory
/// add and stop the visual.
#[derive(Debug)]
pub struct LootDropVisual {
    /// Server-allocated loot id. Used for `PickUpLoot` requests
    /// and for de-duping when a drop arrives via both the
    /// `LootDropped` event and a snapshot `EntityKind::Loot` row.
    pub net_id: rift_net::NetId,
    pub position: Vec3,
    /// The fully-rolled item held by the drop. Cloned out on
    /// pickup; until then it just drives the visual's tier color
    /// and a hover-tooltip later.
    pub item: rift_game::loot::Item,
    /// Particle-system emitter handle for the pillar of light. We
    /// keep it so a future pickup can stop the emitter cleanly.
    pub pillar_emitter: usize,
    /// Particle-system emitter handle for the bright base pulse.
    pub base_emitter: usize,
}

/// Spawn the loot-pillar visual at `position` for a freshly-dropped
/// item. Idempotent on `loot_id` so receiving both the
/// `WorldEvent::LootDropped` and the next snapshot's
/// `EntityKind::Loot` row doesn't double-spawn the emitter.
pub fn spawn_loot_drop_visual(
    state: &mut GameState,
    renderer: &mut rift_engine::renderer::Renderer,
    loot_id: rift_net::NetId,
    position: Vec3,
    blob: rift_net::messages::ItemBlob,
) {
    use rift_engine::renderer::particles::{Emitter, EmitterConfig};

    if state.loot_drops.iter().any(|d| d.net_id == loot_id) {
        return;
    }
    // Rehydrate the wire blob into a full Item. Mismatched indices
    // (e.g. server running a newer build) → drop the visual.
    let Some(item) = rift_game::loot::Item::from_wire(
        blob.base_id,
        blob.rarity,
        blob.ilvl,
        &blob.affixes,
    ) else {
        log::warn!(
            "loot drop {loot_id:?} has unknown indices base={} affixes={:?}; skipping visual",
            blob.base_id,
            blob.affixes
        );
        return;
    };

    let color = item.rarity.color();
    let pillar = renderer
        .particle_system
        .add_emitter(Emitter::new(position, EmitterConfig::loot_beam(color)));
    let base = renderer
        .particle_system
        .add_emitter(Emitter::new(position, EmitterConfig::loot_beam_base(color)));
    log::info!(
        "loot dropped: {} (item-level {}) at {:?}",
        item.display_name(),
        item.ilvl,
        position
    );
    state.loot_drops.push(LootDropVisual {
        net_id: loot_id,
        position,
        item,
        pillar_emitter: pillar,
        base_emitter: base,
    });
}

/// Map an `EnterPhase` to a 0..=1 fraction for the loading bar.
fn compute_enter_progress(
    phase: EnterPhase,
    equip: &equipment_visuals::EquipmentVisuals,
) -> f32 {
    const PREP_END: f32 = 0.05;
    const HUB_END: f32 = 0.45;
    const ATTACH_END: f32 = 0.50;
    const OUTFITS_END: f32 = 0.95;
    const WALLS_END: f32 = 1.0;

    match phase {
        EnterPhase::PrepareScene => PREP_END * 0.5,
        EnterPhase::PreloadHub => PREP_END + (HUB_END - PREP_END) * 0.25,
        EnterPhase::GenerateHub => PREP_END + (HUB_END - PREP_END) * 0.75,
        EnterPhase::AttachOutfits => HUB_END + (ATTACH_END - HUB_END) * 0.5,
        EnterPhase::LoadOutfits => {
            let total = equip.total_pieces().max(1) as f32;
            let done = equip.loaded_pieces() as f32;
            ATTACH_END + (OUTFITS_END - ATTACH_END) * (done / total)
        }
        EnterPhase::RebuildWalls => OUTFITS_END + (WALLS_END - OUTFITS_END) * 0.5,
    }
}

fn draw_world_loading_overlay(renderer: &mut Renderer, progress: f32, label: &str) {
    let (sw, sh) = renderer.screen_size();
    let batch = &mut renderer.overlay_batch;

    batch.rect_px(0.0, 0.0, sw, sh, [0.02, 0.02, 0.03, 0.92], sw, sh);

    let title = "Entering World";
    let title_size = 30.0;
    let title_w = batch.measure_text(title, title_size);
    batch.text(
        title,
        (sw - title_w) * 0.5,
        sh * 0.40 - title_size,
        title_size,
        [0.85, 0.80, 0.65, 1.0],
        sw,
        sh,
    );

    let bar_w = (sw * 0.45).max(240.0);
    let bar_h = 18.0;
    let bar_x = (sw - bar_w) * 0.5;
    let bar_y = sh * 0.50;
    batch.rect_px(bar_x, bar_y, bar_w, bar_h, [0.10, 0.10, 0.14, 1.0], sw, sh);
    let fill_w = bar_w * progress.clamp(0.0, 1.0);
    if fill_w > 0.5 {
        batch.rect_px(bar_x, bar_y, fill_w, bar_h, [0.55, 0.45, 0.20, 1.0], sw, sh);
    }
    let border = [0.30, 0.28, 0.22, 1.0];
    let t = 1.5;
    batch.rect_px(bar_x, bar_y, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y + bar_h - t, bar_w, t, border, sw, sh);
    batch.rect_px(bar_x, bar_y, t, bar_h, border, sw, sh);
    batch.rect_px(bar_x + bar_w - t, bar_y, t, bar_h, border, sw, sh);

    let label_size = 14.0;
    let label_w = batch.measure_text(label, label_size);
    batch.text(
        label,
        (sw - label_w) * 0.5,
        bar_y + bar_h + 16.0,
        label_size,
        [0.65, 0.62, 0.55, 1.0],
        sw,
        sh,
    );
}