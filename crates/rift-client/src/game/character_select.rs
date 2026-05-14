//! Character selection / creation screen.
//!
//! Owns its own UI state machine (loading roster → roster list
//! → create form → delete confirmation) and renders through
//! the immediate-mode UI stack ([`rift_engine::ui::im`]). Also
//! manages a single preview avatar in the world so the player
//! sees the gender choice come to life.
//!
//! There is no account-entry view: the player's identity is
//! always resolved at startup (Steam ticket or dev HMAC) before
//! this screen is ever shown, so the screen starts directly on
//! the loading-roster gate while the server's `Authenticated`
//! reply is in flight.
//!
//! Public entry points:
//!  - [`CharacterSelect::new`]: build initial state.
//!  - [`CharacterSelect::tick_preview`]: drive the preview avatar
//!    (independent of the UI; needs `&mut World` and `&mut Renderer`).
//!  - [`CharacterSelect::frame`]: run one frame's UI inside an
//!    [`rift_engine::ui::im::Ui`]; returns the user-issued action.

use glam::{Mat4, Vec3};
use rift_engine::{
    animation::{self, Animator, Clip},
    ecs::components::{AnimationSet, Renderable, Skinned, SkinnedAttachments, Transform},
    renderer::mesh::SkinnedMesh,
    ui::im::{Color, Ui},
    Renderer,
};
use std::sync::Arc;

use rift_game::character::{CharacterProfile, CharacterRoster, Gender, MAX_CHARACTERS};
use rift_game::hero;
use rift_ui_types::character_select as ui_view;

// Bake-time guard: `rift_ui_types` duplicates the slot count
// (so it can stay free of a `rift-game` dep across the
// hot-reload boundary). If the two ever drift, this fails to
// compile rather than silently rendering only N rows.
const _: () = assert!(MAX_CHARACTERS == ui_view::MAX_CHARACTER_SLOTS);

/// What the screen wants the surrounding game to do this frame.
#[derive(Clone, Debug)]
pub enum SelectAction {
    /// No state change; keep rendering the screen.
    None,
    /// User confirmed a slot to play. Game should build the world
    /// and transition to `Playing`.
    Play {
        /// Account name resolved at startup (Steam ticket
        /// `steam:<id>` or dev `dev:<name>`). Threaded through
        /// to identify the persistent `accounts` row that
        /// owns this profile server-side.
        account_name: String,
        profile: CharacterProfile,
    },
    /// User wants to leave the game entirely.
    Quit,
}

/// Sub-views the screen can be in. Data that needs to persist
/// across frames (typed names, half-filled forms) lives on the
/// parent [`CharacterSelect`] — the variants here are
/// discriminator-only so view switches stay cheap.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ViewKind {
    LoadingRoster,
    Roster,
    Create,
    DeleteConfirm { idx: usize },
}

#[derive(Clone, Debug)]
struct CreateForm {
    name: String,
    gender: Gender,
}

impl CreateForm {
    fn new() -> Self {
        Self {
            name: String::new(),
            gender: Gender::Female,
        }
    }
}

pub struct CharacterSelect {
    roster: CharacterRoster,
    view: ViewKind,
    /// Currently-selected roster slot in the `Roster` view.
    /// Drives the red "name container" highlight and gates
    /// the panel-level Play / Delete buttons. Kept on the host
    /// (not in the view-model) so it survives the per-frame
    /// roster snapshot rebuild. Reset to `Some(0)` whenever the
    /// roster is replaced and a slot is filled; cleared when
    /// the roster is empty.
    selected_idx: Option<usize>,
    /// Account identity resolved at startup (Steam ticket or
    /// dev HMAC). Set via [`Self::set_account_identity`] before
    /// the first frame and threaded through to
    /// [`SelectAction::Play`].
    account_name: String,
    /// In-progress create form. Reset each time we enter
    /// [`ViewKind::Create`].
    create_form: CreateForm,
    /// Time accumulator for the slow podium rotation + caret blink.
    rotation_t: f32,
    /// What the preview avatar currently represents. `None` means
    /// no preview entity is alive in the world right now.
    preview_state: Option<PreviewState>,
    /// Cached animation library, lazily loaded the first time the
    /// preview spawns. Re-bound per-skeleton when gender changes.
    anim_clips: Option<Vec<Clip>>,
}

#[derive(Clone, Debug)]
struct PreviewState {
    entity: hecs::Entity,
    gender: Gender,
    hidden_frames_remaining: u8,
}

impl CharacterSelect {
    pub fn new() -> Self {
        Self {
            roster: CharacterRoster::new(),
            view: ViewKind::LoadingRoster,
            selected_idx: None,
            account_name: String::new(),
            create_form: CreateForm::new(),
            rotation_t: 0.0,
            preview_state: None,
            anim_clips: None,
        }
    }

    pub fn roster(&self) -> &CharacterRoster {
        &self.roster
    }

    /// Install the account identity (Steam ticket or dev HMAC)
    /// resolved at startup. The screen lands directly in the
    /// loading-roster view; the next `apply_server_roster`
    /// promotes it to the roster view.
    pub fn skip_to_loading(&mut self, account_identity: String) {
        self.account_name = account_identity;
        self.view = ViewKind::LoadingRoster;
    }

    /// Replace the local roster with a server-supplied one and
    /// move past the `LoadingRoster` gate. Idempotent: callers
    /// can re-invoke after a reconnect without resetting any
    /// other view state.
    pub fn apply_server_roster(&mut self, entries: Vec<rift_net::messages::RosterEntry>) {
        self.roster = CharacterRoster::new();
        for e in entries {
            let gender = match e.gender {
                rift_net::messages::Gender::Male => Gender::Male,
                rift_net::messages::Gender::Female => Gender::Female,
            };
            let mut profile = CharacterProfile::new(e.character_name, gender);
            profile.level = e.level;
            profile.equipped_base_ids = e.equipped_base_ids;
            self.roster.add(profile);
        }
        if matches!(self.view, ViewKind::LoadingRoster) {
            self.view = ViewKind::Roster;
        }
        // Pick first slot by default so the panel-level Play /
        // Delete buttons are immediately actionable; clear when
        // the roster came back empty so the buttons stay
        // disabled until the user creates a character.
        self.selected_idx = if self.roster.len() == 0 {
            None
        } else {
            Some(0)
        };
    }

    // ─── Preview management ──────────────────────────────────────────

    /// Drive the preview avatar (spawn / despawn / rotate /
    /// camera). Pure side-effect on `world` and `renderer`; UI
    /// is handled separately by [`Self::frame`].
    pub fn tick_preview(&mut self, world: &mut hecs::World, renderer: &mut Renderer, dt: f32) {
        self.rotation_t += dt;
        let desired_gender = self.desired_preview_gender();
        self.ensure_preview(world, renderer, desired_gender);
        self.update_preview_camera(world, renderer);
    }

    fn desired_preview_gender(&self) -> Option<Gender> {
        match &self.view {
            ViewKind::LoadingRoster => None,
            ViewKind::Create => Some(self.create_form.gender),
            ViewKind::Roster => self
                .selected_idx
                .and_then(|i| self.roster.get(i))
                .or_else(|| self.roster.slots().first())
                .map(|p| p.gender),
            ViewKind::DeleteConfirm { idx } => self.roster.get(*idx).map(|p| p.gender),
        }
    }

    /// Equipped base-item indices to dress the preview avatar
    /// with. `None` when no profile drives the preview (account
    /// entry / loading / create form). Pulled from the cached
    /// `RosterEntry::equipped_base_ids` the server sent on
    /// roster lookup, so the preview matches what that
    /// character is actually wearing on the server.
    pub fn preview_equipped_base_ids(&self) -> Option<&[u16]> {
        let profile = match &self.view {
            ViewKind::LoadingRoster | ViewKind::Create => None,
            ViewKind::Roster => self
                .selected_idx
                .and_then(|i| self.roster.get(i))
                .or_else(|| self.roster.slots().first()),
            ViewKind::DeleteConfirm { idx } => self.roster.get(*idx),
        }?;
        Some(&profile.equipped_base_ids)
    }

    /// Currently-alive preview avatar entity + the gender it
    /// was spawned with. `None` between view changes when
    /// `tick_preview` is about to rebuild it.
    pub fn preview_entity(&self) -> Option<(hecs::Entity, Gender)> {
        self.preview_state.as_ref().map(|s| (s.entity, s.gender))
    }

    /// Keep a freshly-spawned preview invisible until GPU skinning has
    /// had a couple of frames to overwrite the bind-pose output buffer.
    pub fn settle_preview_pose(&mut self, world: &hecs::World, renderer: &mut Renderer) {
        let Some(prev) = self.preview_state.as_mut() else {
            return;
        };
        if prev.hidden_frames_remaining == 0 {
            return;
        }
        collapse_preview_render_slots(world, renderer, prev.entity);
        prev.hidden_frames_remaining = prev.hidden_frames_remaining.saturating_sub(1);
    }

    /// Drop the preview avatar entity and free its render-object
    /// slot. Called by `GameState` right before generating the
    /// hub so the dynamic mesh slot can be reclaimed (and so the
    /// preview model doesn't briefly appear inside the hub).
    pub fn teardown_preview(&mut self, world: &mut hecs::World, renderer: &mut Renderer) {
        if let Some(prev) = self.preview_state.take() {
            collapse_preview_render_slots(world, renderer, prev.entity);
            let _ = world.despawn(prev.entity);
        }
    }

    fn ensure_preview(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        desired: Option<Gender>,
    ) {
        let needs_rebuild = match (&self.preview_state, desired) {
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
            (Some(s), Some(g)) => s.gender != g,
        };
        if !needs_rebuild {
            return;
        }
        if let Some(prev) = self.preview_state.take() {
            collapse_preview_render_slots(world, renderer, prev.entity);
            let _ = world.despawn(prev.entity);
        }
        if let Some(gender) = desired {
            if let Some(entity) = self.spawn_preview_entity(world, renderer, gender) {
                self.preview_state = Some(PreviewState {
                    entity,
                    gender,
                    hidden_frames_remaining: 4,
                });
            }
        }
    }

    fn spawn_preview_entity(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        gender: Gender,
    ) -> Option<hecs::Entity> {
        let (model_path, tex_path) = hero::base_model_paths(gender);
        let skinned = match SkinnedMesh::from_gltf_filtered(model_path, |node, mesh| {
            super::avatar_cosmetics::is_body_mesh_name(node, mesh)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Preview model load failed ({:?}): {}", gender, e);
                return None;
            }
        };

        let clips = self.load_clips_lazy();
        let mut anim_set = AnimationSet::default();
        if let Some(clips) = clips {
            for clip in clips {
                let bound =
                    clip.bind_to_skeleton(&skinned.joint_index_by_name, skinned.joints.len());
                anim_set
                    .clips
                    .insert(clip.name.to_ascii_lowercase(), Arc::new(bound));
            }
        }
        let idle_clip = anim_set
            .find_any(&["Idle_Loop", "Idle"])
            .or_else(|| anim_set.clips.values().next().cloned());
        let animator = idle_clip.map(Animator::new);
        let mut initial_palette = Vec::new();
        let mut initial_joint_worlds = Vec::new();
        let mut initial_vertices = Vec::new();
        if let Some(animator) = animator.as_ref() {
            animation::build_bone_palette(
                animator,
                &skinned.joints,
                &mut initial_palette,
                Some(&mut initial_joint_worlds),
            );
            skinned.skin_to(&initial_palette, &mut initial_vertices);
        }

        let podium_pos = Vec3::new(0.0, 0.0, 0.0);
        let obj_idx = match renderer.add_skinned_mesh(
            &skinned.bind_vertices,
            &skinned.vertex_skin,
            &skinned.indices,
            Mat4::from_translation(podium_pos),
            0.0,
        ) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("Preview mesh upload failed: {}", e);
                return None;
            }
        };
        if !initial_vertices.is_empty() {
            if let Err(e) = renderer.prime_skinned_mesh_output(obj_idx, &initial_vertices) {
                log::warn!("Preview idle-pose upload failed: {}", e);
            }
            renderer.update_palette(obj_idx, &initial_palette);
        }
        if let Err(e) = renderer.set_object_texture(
            obj_idx,
            rift_engine::TextureSource::File(std::path::Path::new(tex_path)),
        ) {
            log::warn!("Preview texture load failed: {}", e);
        }

        let comp = Skinned {
            mesh: Arc::new(skinned),
            scratch: Vec::new(),
            joint_worlds: initial_joint_worlds,
        };
        let entity = world.spawn((
            Transform::from_position(podium_pos),
            Renderable {
                object_index: obj_idx,
            },
        ));
        let _ = world.insert_one(entity, comp);
        let _ = world.insert_one(entity, anim_set);
        if let Some(anim) = animator {
            let _ = world.insert_one(entity, anim);
        }
        log::info!(
            "Preview spawned: entity={:?} obj_idx={} gender={:?}",
            entity,
            obj_idx,
            gender
        );
        Some(entity)
    }

    fn load_clips_lazy(&mut self) -> Option<&Vec<Clip>> {
        if self.anim_clips.is_none() {
            let path = "assets/models/animation-library/Unreal-Godot/UAL1_Standard.glb";
            match Clip::load_all(path) {
                Ok(clips) => self.anim_clips = Some(clips),
                Err(e) => {
                    log::warn!("Animation library load failed (preview): {}", e);
                    return None;
                }
            }
        }
        self.anim_clips.as_ref()
    }

    fn update_preview_camera(&mut self, world: &mut hecs::World, renderer: &mut Renderer) {
        // The roster / create panel covers roughly the left 46% of the
        // screen at near-full opacity, so we offset the camera so the
        // avatar reads in the empty right half (~73% of width).
        const OFFSET_X: f32 = -0.95;
        // Avatar stands still and faces the camera — no auto-
        // spin. The user explicitly asked for a static pose;
        // the world transform is identity so whatever the
        // model's authored forward is points at +Z (camera).
        if let Some(prev) = &self.preview_state {
            let model_rot = glam::Quat::IDENTITY;
            if let Ok(mut t) = world.get::<&mut Transform>(prev.entity) {
                t.rotation = model_rot;
            }
            let idx = world
                .get::<&Renderable>(prev.entity)
                .map(|r| r.object_index)
                .unwrap_or(usize::MAX);
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = Mat4::IDENTITY;
            }
        }
        renderer.camera.position = Vec3::new(OFFSET_X, 1.4, 3.6);
        renderer.camera.target = Vec3::new(OFFSET_X, 1.0, 0.0);
        // Gameplay anchors fog/animation LOD at the local player. The preview
        // entity lives at the origin, so returning from a far-away floor must
        // re-anchor here or the non-player skinning LOD updates it at 15-30 Hz.
        renderer.fog_origin = Vec3::ZERO;
        // Atmosphere + backdrop disc + dune ring + drifting
        // sand haze are owned by `FloorManager` and installed
        // by `transition::update_character_select` so the
        // sandstorm visual recipe stays in one place (shared
        // with `generate_hub`).
    }

    // ─── Per-frame UI ────────────────────────────────────────────────

    /// Run one frame's worth of UI (input + draw fused). Returns
    /// the action the surrounding `GameState` should perform.
    pub fn frame(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        // Backdrop dim across the whole screen so the preview reads
        // cleanly behind the panel.
        let screen = ui.screen_rect();
        ui.draw_rect(screen, Color::rgba(0.0, 0.0, 0.0, 0.35));

        match self.view.clone() {
            ViewKind::LoadingRoster => {
                self.frame_loading_roster(ui);
                SelectAction::None
            }
            ViewKind::Roster => self.frame_roster(ui),
            ViewKind::Create => self.frame_create(ui),
            ViewKind::DeleteConfirm { idx } => {
                // Render the roster behind the modal so the page
                // identity persists during the confirmation.
                let _ = self.frame_roster(ui);
                self.frame_delete_confirm(ui, idx)
            }
        }
    }

    fn frame_loading_roster(&mut self, ui: &mut Ui<'_>) {
        let view = ui_view::LoadingRosterView {
            account_name: &self.account_name,
            anim_time: self.rotation_t,
        };
        rift_ui::character_select::frame_loading_roster(ui, &view);
    }

    fn frame_roster(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        // Build the view-model snapshot. The widget needs string /
        // numeric scalars only; no engine types cross the boundary.
        let entries: Vec<ui_view::RosterEntryView<'_>> = self
            .roster
            .slots()
            .iter()
            .map(|p| ui_view::RosterEntryView {
                name: &p.name,
                level: p.level as u32,
                gender_label: p.gender.label(),
            })
            .collect();
        // Clamp a stale selection (e.g. after a Delete) so the
        // widget never receives an out-of-range index.
        let selected = self.selected_idx.filter(|i| *i < entries.len());
        let view = ui_view::RosterView {
            entries: &entries,
            selected,
            allow_create: !self.roster.is_full(),
        };

        match rift_ui::character_select::frame_roster(ui, &view) {
            ui_view::RosterAction::None => SelectAction::None,
            ui_view::RosterAction::Select(i) => {
                self.selected_idx = Some(i);
                SelectAction::None
            }
            ui_view::RosterAction::Play => match selected {
                Some(i) => {
                    let p = self.roster.slots()[i].clone();
                    SelectAction::Play {
                        account_name: self.account_name.clone(),
                        profile: p,
                    }
                }
                None => SelectAction::None,
            },
            ui_view::RosterAction::Delete => {
                if let Some(i) = selected {
                    self.view = ViewKind::DeleteConfirm { idx: i };
                }
                SelectAction::None
            }
            ui_view::RosterAction::Create => {
                self.create_form = CreateForm::new();
                self.view = ViewKind::Create;
                SelectAction::None
            }
            ui_view::RosterAction::Quit => SelectAction::Quit,
        }
    }

    fn frame_create(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        // The widget mutates `name` and `gender_is_male` in place
        // through `&mut` borrows — String / bool both have stable
        // std layout across the hot-reload boundary.
        let mut gender_is_male = self.create_form.gender == Gender::Male;
        let mut view = ui_view::CreateFormView {
            name: &mut self.create_form.name,
            gender_is_male: &mut gender_is_male,
            anim_time: self.rotation_t,
        };
        let action = rift_ui::character_select::frame_create(ui, &mut view);
        // Reflect the gender toggle back into the strongly-typed
        // `Gender` the rest of the game code expects.
        self.create_form.gender = if gender_is_male {
            Gender::Male
        } else {
            Gender::Female
        };

        match action {
            ui_view::CreateAction::None => SelectAction::None,
            ui_view::CreateAction::Confirm => {
                let trimmed = self.create_form.name.trim().to_string();
                if !trimmed.is_empty() {
                    let profile = CharacterProfile::new(trimmed, self.create_form.gender);
                    self.roster.add(profile);
                    // New character becomes the selection so
                    // the panel-level Play button is
                    // immediately actionable.
                    self.selected_idx = Some(self.roster.len().saturating_sub(1));
                    self.view = ViewKind::Roster;
                }
                SelectAction::None
            }
            ui_view::CreateAction::Cancel => {
                self.view = ViewKind::Roster;
                SelectAction::None
            }
        }
    }

    fn frame_delete_confirm(&mut self, ui: &mut Ui<'_>, idx: usize) -> SelectAction {
        let name = self
            .roster
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".to_string());
        let view = ui_view::DeleteConfirmView {
            character_name: &name,
        };
        match rift_ui::character_select::frame_delete_confirm(ui, &view) {
            ui_view::DeleteAction::None => SelectAction::None,
            ui_view::DeleteAction::Confirm => {
                self.roster.remove(idx);
                // Drop a stale selection; if the roster still
                // has entries, fall back to the first slot so
                // the buttons stay usable.
                self.selected_idx = if self.roster.len() == 0 {
                    None
                } else {
                    Some(0)
                };
                self.view = ViewKind::Roster;
                SelectAction::None
            }
            ui_view::DeleteAction::Cancel => {
                self.view = ViewKind::Roster;
                SelectAction::None
            }
        }
    }
}

/// Zero out the renderer model matrices of every dynamic-mesh
/// slot the preview avatar owns: the base body, plus every
/// modular-outfit `AttachmentPiece`. Without this the slots
/// linger as garbage in the renderer until reused, which would
/// briefly draw the preview gear inside the freshly-generated
/// hub.
fn collapse_preview_render_slots(
    world: &hecs::World,
    renderer: &mut Renderer,
    entity: hecs::Entity,
) {
    if let Ok(r) = world.get::<&Renderable>(entity) {
        if r.object_index < renderer.objects.len() {
            renderer.objects[r.object_index].model_matrix = glam::Mat4::ZERO;
        }
    }
    if let Ok(atts) = world.get::<&SkinnedAttachments>(entity) {
        for piece in &atts.pieces {
            if piece.object_index < renderer.objects.len() {
                renderer.objects[piece.object_index].model_matrix = glam::Mat4::ZERO;
            }
        }
    }
}
