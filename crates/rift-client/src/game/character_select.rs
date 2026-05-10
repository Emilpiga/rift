//! Character selection / creation screen.
//!
//! Owns its own UI state machine (account-entry → loading roster
//! → roster list → create form → delete confirmation) and renders
//! through the immediate-mode UI stack ([`rift_engine::ui::im`]).
//! Also manages a single preview avatar in the world so the
//! player sees the gender choice come to life.
//!
//! Public entry points:
//!  - [`CharacterSelect::new`]: build initial state.
//!  - [`CharacterSelect::tick_preview`]: drive the preview avatar
//!    (independent of the UI; needs `&mut World` and `&mut Renderer`).
//!  - [`CharacterSelect::frame`]: run one frame's UI inside an
//!    [`rift_engine::ui::im::Ui`]; returns the user-issued action.

use glam::{Mat4, Vec3};
use rift_engine::{
    animation::{Animator, Clip},
    ecs::components::{AnimationSet, Renderable, Skinned, SkinnedAttachments, Transform},
    renderer::mesh::SkinnedMesh,
    ui::im::{
        widgets::{label, text_field, title},
        Button, Color, Frame, Id, Pad, Pos2, Rect, Stroke, Ui, Vec2,
    },
    Renderer,
};
use std::sync::Arc;

use rift_game::character::{CharacterProfile, CharacterRoster, Gender, MAX_CHARACTERS};
use rift_game::hero;

/// What the screen wants the surrounding game to do this frame.
#[derive(Clone, Debug)]
pub enum SelectAction {
    /// No state change; keep rendering the screen.
    None,
    /// Account-entry view confirmed. Game should fire a roster
    /// lookup against the server with this account name. Until
    /// the roster arrives the screen renders a "Loading…" stub.
    AccountConfirmed { name: String },
    /// User confirmed a slot to play. Game should build the world
    /// and transition to `Playing`.
    Play {
        /// Account name typed in the entry view; used by the
        /// server to resolve / create the persistent `accounts`
        /// row that owns this profile.
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
    AccountEntry,
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
    /// Account name typed in the [`ViewKind::AccountEntry`] gate.
    /// Persists across frames; copied into [`Self::account_name`]
    /// once confirmed so it survives the view transition into
    /// `Roster`.
    account_entry: String,
    /// Account name confirmed in the entry gate. Empty before
    /// confirmation. Sent with [`SelectAction::Play`].
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
}

impl CharacterSelect {
    pub fn new() -> Self {
        Self {
            roster: CharacterRoster::new(),
            view: ViewKind::AccountEntry,
            account_entry: String::new(),
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

    /// Replace the local roster with a server-supplied one and
    /// move past the `LoadingRoster` gate. Idempotent: callers
    /// can re-invoke after a reconnect without resetting any
    /// other view state.
    pub fn apply_server_roster(
        &mut self,
        entries: Vec<rift_net::messages::RosterEntry>,
    ) {
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
    }

    // ─── Preview management ──────────────────────────────────────────

    /// Drive the preview avatar (spawn / despawn / rotate /
    /// camera). Pure side-effect on `world` and `renderer`; UI
    /// is handled separately by [`Self::frame`].
    pub fn tick_preview(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        dt: f32,
    ) {
        self.rotation_t += dt;
        let desired_gender = self.desired_preview_gender();
        self.ensure_preview(world, renderer, desired_gender);
        self.update_preview_camera(world, renderer);
    }

    fn desired_preview_gender(&self) -> Option<Gender> {
        match &self.view {
            ViewKind::AccountEntry | ViewKind::LoadingRoster => None,
            ViewKind::Create => Some(self.create_form.gender),
            ViewKind::Roster => self.roster.slots().first().map(|p| p.gender),
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
            ViewKind::AccountEntry | ViewKind::LoadingRoster | ViewKind::Create => None,
            ViewKind::Roster => self.roster.slots().first(),
            ViewKind::DeleteConfirm { idx } => self.roster.get(*idx),
        }?;
        Some(&profile.equipped_base_ids)
    }

    /// Currently-alive preview avatar entity + the gender it
    /// was spawned with. `None` between view changes when
    /// `tick_preview` is about to rebuild it.
    pub fn preview_entity(&self) -> Option<(hecs::Entity, Gender)> {
        self.preview_state
            .as_ref()
            .map(|s| (s.entity, s.gender))
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
                self.preview_state = Some(PreviewState { entity, gender });
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
        let skinned = match SkinnedMesh::from_gltf_filtered(
            model_path,
            |n| super::avatar_cosmetics::is_body_mesh_name(n),
        ) {
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
        if let Err(e) = renderer.set_object_texture(
            obj_idx,
            rift_engine::TextureSource::File(std::path::Path::new(tex_path)),
        ) {
            log::warn!("Preview texture load failed: {}", e);
        }

        let comp = Skinned {
            mesh: Arc::new(skinned),
            scratch: Vec::new(),
            joint_worlds: Vec::new(),
        };
        let entity = world.spawn((
            Transform::from_position(podium_pos),
            Renderable { object_index: obj_idx },
        ));
        let _ = world.insert_one(entity, comp);
        let _ = world.insert_one(entity, anim_set);
        if let Some(anim) = animator {
            let _ = world.insert_one(entity, anim);
        }
        log::info!(
            "Preview spawned: entity={:?} obj_idx={} gender={:?}",
            entity, obj_idx, gender
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

    fn update_preview_camera(&self, world: &mut hecs::World, renderer: &mut Renderer) {
        // The roster / create panel covers roughly the left 46% of the
        // screen at near-full opacity, so we offset the camera so the
        // avatar reads in the empty right half (~73% of width).
        const OFFSET_X: f32 = -0.95;
        if let Some(prev) = &self.preview_state {
            let rot = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0))
                * Mat4::from_rotation_y(self.rotation_t * 0.35);
            if let Ok(mut t) = world.get::<&mut Transform>(prev.entity) {
                t.rotation = glam::Quat::from_rotation_y(self.rotation_t * 0.35);
            }
            let idx = world
                .get::<&Renderable>(prev.entity)
                .map(|r| r.object_index)
                .unwrap_or(usize::MAX);
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = rot;
            }
        }
        renderer.camera.position = Vec3::new(OFFSET_X, 1.4, 3.6);
        renderer.camera.target = Vec3::new(OFFSET_X, 1.0, 0.0);
        renderer.clear_color = [0.10, 0.10, 0.14, 1.0];
        renderer.fog_color = [0.12, 0.12, 0.16];
        renderer.fog_start = 8.0;
        renderer.fog_end = 30.0;

        renderer.point_lights.clear();
        renderer.point_lights.push(rift_engine::PointLight {
            position: Vec3::new(1.4, 1.6, 2.4),
            color: Vec3::new(1.0, 0.92, 0.78),
            radius: 12.0,
            intensity: 4.5,
        });
        renderer.point_lights.push(rift_engine::PointLight {
            position: Vec3::new(-1.6, 1.4, -1.8),
            color: Vec3::new(0.55, 0.70, 1.0),
            radius: 10.0,
            intensity: 3.0,
        });
        renderer.point_lights.push(rift_engine::PointLight {
            position: Vec3::new(0.0, 3.2, 1.5),
            color: Vec3::new(0.95, 0.95, 1.0),
            radius: 10.0,
            intensity: 2.0,
        });
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
            ViewKind::AccountEntry => self.frame_account_entry(ui),
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

    /// Common left-side panel rect used by every full-screen view.
    fn panel_rect(ui: &Ui<'_>) -> Rect {
        let s = ui.screen_size();
        Rect::from_xywh(s.x * 0.04, s.y * 0.10, s.x * 0.42, s.y * 0.80)
    }

    fn frame_account_entry(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        let panel = Self::panel_rect(ui);
        let theme = *ui.theme();
        let mut action = SelectAction::None;

        Frame::panel(&theme).show(ui, panel, |ui, body| {
            let s = theme.scale;
            let _ = title(ui, body.min, "ACCOUNT");
            label(
                ui,
                body.min + Vec2::new(0.0, 44.0 * s),
                "Enter the account name to load your characters",
            );

            // Field label + text input.
            label(ui, body.min + Vec2::new(0.0, 100.0 * s), "Account name");
            let field_rect = Rect::from_xywh(
                body.x(),
                body.y() + 122.0 * s,
                body.width(),
                50.0 * s,
            );
            let field_id = Id::root("char_select").child("account_field");
            let resp = text_field(
                ui,
                field_id,
                field_rect,
                &mut self.account_entry,
                "Type a name…",
                18,
                self.rotation_t,
            );

            label(
                ui,
                body.min + Vec2::new(0.0, 190.0 * s),
                "New name? A fresh account is created automatically.",
            );

            // Confirm + Quit row pinned to the bottom of the panel.
            let can_confirm = !self.account_entry.trim().is_empty();
            let btn_y = body.max.y - 50.0 * s;
            let confirm = Button::primary("Continue")
                .enabled(can_confirm)
                .show(
                    ui,
                    Rect::from_xywh(body.x(), btn_y, 160.0 * s, 50.0 * s),
                );
            let quit = Button::new("Quit").show(
                ui,
                Rect::from_xywh(body.x() + 180.0 * s, btn_y, 120.0 * s, 50.0 * s),
            );

            // Fire on click, on Enter (when field is focused), or
            // on Enter while the confirm button is hovered (so a
            // mouse-only player still has Enter as a confirm key).
            let enter = ui.input().enter_just_pressed() && (resp.focused || confirm.hovered);
            if (confirm.clicked || enter) && can_confirm {
                let trimmed = self.account_entry.trim().to_string();
                self.account_name = trimmed.clone();
                self.view = ViewKind::LoadingRoster;
                action = SelectAction::AccountConfirmed { name: trimmed };
            } else if quit.clicked {
                action = SelectAction::Quit;
            }
        });

        action
    }

    fn frame_loading_roster(&mut self, ui: &mut Ui<'_>) {
        let panel = Self::panel_rect(ui);
        let theme = *ui.theme();
        Frame::panel(&theme).show(ui, panel, |ui, body| {
            let s = theme.scale;
            let _ = title(ui, body.min, "ACCOUNT");
            let dots = match (self.rotation_t * 1.5) as i32 % 4 {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            label(
                ui,
                body.min + Vec2::new(0.0, 44.0 * s),
                &format!("Loading roster for '{}'{dots}", self.account_name),
            );
        });
    }

    fn frame_roster(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        let panel = Self::panel_rect(ui);
        let theme = *ui.theme();
        let mut action = SelectAction::None;

        Frame::panel(&theme).show(ui, panel, |ui, body| {
            let s = theme.scale;
            let _ = title(ui, body.min, "CHARACTER SELECT");
            label(
                ui,
                body.min + Vec2::new(0.0, 44.0 * s),
                "Choose a character to enter the rift",
            );

            let row_h = 90.0 * s;
            let row_pad = 12.0 * s;

            for i in 0..MAX_CHARACTERS {
                let y = body.y() + 88.0 * s + (i as f32) * (row_h + row_pad);
                let row_rect = Rect::from_xywh(body.x(), y, body.width(), row_h);

                if let Some(profile) = self.roster.get(i) {
                    // Row chrome.
                    Frame::inset(&theme).show(ui, row_rect, |ui, inner| {
                        ui.draw_text(
                            inner.min + Vec2::new(8.0 * s, 4.0 * s),
                            &profile.name,
                            theme.fonts.size_lg,
                            theme.colors.text,
                        );
                        let sub = format!(
                            "Lv {}  -  {}",
                            profile.level,
                            profile.gender.label(),
                        );
                        ui.draw_text(
                            inner.min + Vec2::new(8.0 * s, 36.0 * s),
                            &sub,
                            theme.fonts.size_sm,
                            theme.colors.text_dim,
                        );

                        let btn_y = inner.y() + 14.0 * s;
                        let play = Button::primary("Play").show_with_id(
                            ui,
                            Id::root("char_select").child(("play", i)),
                            Rect::from_xywh(inner.max.x - 200.0 * s, btn_y, 90.0 * s, 40.0 * s),
                        );
                        let del = Button::danger("Delete").show_with_id(
                            ui,
                            Id::root("char_select").child(("delete", i)),
                            Rect::from_xywh(inner.max.x - 100.0 * s, btn_y, 90.0 * s, 40.0 * s),
                        );
                        if play.clicked {
                            let p = self.roster.slots()[i].clone();
                            action = SelectAction::Play {
                                account_name: self.account_name.clone(),
                                profile: p,
                            };
                        } else if del.clicked {
                            self.view = ViewKind::DeleteConfirm { idx: i };
                        }
                    });
                } else if i == self.roster.len() && !self.roster.is_full() {
                    // "+ Create new" row.
                    let create = Button::new("+ Create New Character")
                        .show_with_id(
                            ui,
                            Id::root("char_select").child(("create_slot", i)),
                            row_rect,
                        );
                    if create.clicked {
                        self.create_form = CreateForm::new();
                        self.view = ViewKind::Create;
                    }
                } else {
                    // Empty / locked slot — dashed placeholder.
                    let dashed = Frame::inset(&theme)
                        .with_fill(Color::rgba(0.06, 0.06, 0.08, 0.5))
                        .with_stroke(Stroke::new(1.0, theme.colors.border));
                    dashed.show_only(ui, row_rect);
                    ui.draw_text(
                        Pos2::new(row_rect.x() + 14.0 * s, row_rect.y() + row_rect.height() * 0.5 - 7.0 * s),
                        "(empty slot)",
                        theme.fonts.size_sm,
                        theme.colors.text_muted,
                    );
                }
            }

            // Quit button bottom-left.
            let quit = Button::new("Quit").show_with_id(
                ui,
                Id::root("char_select").child("roster_quit"),
                Rect::from_xywh(body.x(), body.max.y - 40.0 * s, 120.0 * s, 40.0 * s),
            );
            if quit.clicked {
                action = SelectAction::Quit;
            }
        });

        action
    }

    fn frame_create(&mut self, ui: &mut Ui<'_>) -> SelectAction {
        let panel = Self::panel_rect(ui);
        let theme = *ui.theme();
        let action = SelectAction::None;

        Frame::panel(&theme).show(ui, panel, |ui, body| {
            let s = theme.scale;
            let _ = title(ui, body.min, "CREATE CHARACTER");

            // Name field.
            label(ui, body.min + Vec2::new(0.0, 60.0 * s), "Name");
            let name_rect = Rect::from_xywh(body.x(), body.y() + 82.0 * s, body.width(), 50.0 * s);
            let name_resp = text_field(
                ui,
                Id::root("char_select").child("create_name"),
                name_rect,
                &mut self.create_form.name,
                "Type a name…",
                18,
                self.rotation_t,
            );

            // Gender row.
            label(ui, body.min + Vec2::new(0.0, 150.0 * s), "Gender");
            let male_rect = Rect::from_xywh(body.x(), body.y() + 172.0 * s, 140.0 * s, 50.0 * s);
            let female_rect = Rect::from_xywh(body.x() + 150.0 * s, body.y() + 172.0 * s, 140.0 * s, 50.0 * s);
            let male_active = self.create_form.gender == Gender::Male;
            let female_active = self.create_form.gender == Gender::Female;
            let male_btn = if male_active {
                Button::active("Male")
            } else {
                Button::new("Male")
            };
            let female_btn = if female_active {
                Button::active("Female")
            } else {
                Button::new("Female")
            };
            if male_btn.show(ui, male_rect).clicked {
                self.create_form.gender = Gender::Male;
            }
            if female_btn.show(ui, female_rect).clicked {
                self.create_form.gender = Gender::Female;
            }

            // Confirm + Cancel.
            let can_confirm = !self.create_form.name.trim().is_empty();
            let btn_y = body.max.y - 50.0 * s;
            let confirm = Button::primary("Confirm")
                .enabled(can_confirm)
                .show_with_id(
                    ui,
                    Id::root("char_select").child("create_confirm"),
                    Rect::from_xywh(body.x(), btn_y, 160.0 * s, 50.0 * s),
                );
            let cancel = Button::new("Cancel").show_with_id(
                ui,
                Id::root("char_select").child("create_cancel"),
                Rect::from_xywh(body.x() + 180.0 * s, btn_y, 120.0 * s, 50.0 * s),
            );
            let enter = ui.input().enter_just_pressed() && name_resp.focused && can_confirm;
            if (confirm.clicked || enter) && can_confirm {
                let profile = CharacterProfile::new(
                    self.create_form.name.trim().to_string(),
                    self.create_form.gender,
                );
                self.roster.add(profile);
                self.view = ViewKind::Roster;
            } else if cancel.clicked {
                self.view = ViewKind::Roster;
            }
        });

        action
    }

    fn frame_delete_confirm(&mut self, ui: &mut Ui<'_>, idx: usize) -> SelectAction {
        // Modal dim covering the screen.
        let screen = ui.screen_rect();
        ui.with_layer(rift_engine::ui::im::Layer::Modal, |ui| {
            ui.draw_rect(screen, Color::rgba(0.0, 0.0, 0.0, 0.55));
        });

        let theme = *ui.theme();
        let s = ui.screen_size();
        let sc = theme.scale;
        let mw = 460.0 * sc;
        let mh = 200.0 * sc;
        let modal_rect = Rect::from_xywh(
            (s.x - mw) * 0.5,
            (s.y - mh) * 0.5,
            mw,
            mh,
        );
        let name = self
            .roster
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".to_string());

        ui.with_layer(rift_engine::ui::im::Layer::Modal, |ui| {
            Frame::panel(&theme)
                .with_padding(Pad::all(20.0 * sc))
                .show(ui, modal_rect, |ui, body| {
                    let _ = title(ui, body.min, "Delete character?");
                    label(
                        ui,
                        body.min + Vec2::new(0.0, 40.0 * sc),
                        &format!("\"{}\" will be permanently removed.", name),
                    );
                    let btn_y = body.max.y - 50.0 * sc;
                    let yes = Button::danger("Delete").show_with_id(
                        ui,
                        Id::root("char_select").child("del_yes"),
                        Rect::from_xywh(body.x(), btn_y, 140.0 * sc, 50.0 * sc),
                    );
                    let no = Button::new("Cancel").show_with_id(
                        ui,
                        Id::root("char_select").child("del_no"),
                        Rect::from_xywh(body.max.x - 140.0 * sc, btn_y, 140.0 * sc, 50.0 * sc),
                    );
                    if yes.clicked {
                        self.roster.remove(idx);
                        self.view = ViewKind::Roster;
                    } else if no.clicked {
                        self.view = ViewKind::Roster;
                    }
                });
        });
        SelectAction::None
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
