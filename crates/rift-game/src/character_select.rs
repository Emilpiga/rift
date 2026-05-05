//! Character selection / creation screen.
//!
//! Owns its own UI state machine (roster view → create form → delete
//! confirmation) and renders entirely through the existing
//! [`OverlayBatch`]. Also manages a single preview avatar in the world
//! so the player sees the gender choice come to life.
//!
//! Public entry points:
//!  - [`CharacterSelect::new`]: build initial state.
//!  - [`CharacterSelect::update`]: tick UI, return any user-issued action.
//!  - [`CharacterSelect::render`]: paint the overlay + preview backdrop.

use glam::{Mat4, Vec3};
use rift_engine::{
    animation::{Animator, Clip},
    ecs::components::{AnimationSet, Renderable, Skinned, Transform},
    input::Input,
    renderer::{mesh::SkinnedMesh, overlay::OverlayBatch},
    Mesh, Renderer,
};
use std::sync::Arc;

use crate::character::{CharacterProfile, CharacterRoster, Gender, MAX_CHARACTERS};
use crate::classes;

/// What the screen wants the surrounding game to do this frame.
#[derive(Clone, Debug)]
pub enum SelectAction {
    /// No state change; keep rendering the screen.
    None,
    /// User confirmed a slot to play. Game should build the world and
    /// transition to `Playing`.
    Play(CharacterProfile),
    /// User wants to leave the game entirely.
    Quit,
}

/// Sub-views the screen can be in.
#[derive(Clone, Debug)]
enum View {
    /// Roster list with Create / Play / Delete buttons.
    Roster,
    /// Filling out the create form.
    Create(CreateForm),
    /// Confirming deletion of a roster slot.
    DeleteConfirm { idx: usize },
}

#[derive(Clone, Debug)]
struct CreateForm {
    name: String,
    gender: Gender,
    class_idx: usize,
}

impl CreateForm {
    fn new() -> Self {
        Self {
            name: String::new(),
            gender: Gender::Female,
            class_idx: 0,
        }
    }
}

/// All available classes, in display order.
fn class_options() -> [(rift_engine::combat::ClassId, &'static str); 1] {
    [(classes::HUNTER, "Hunter")]
}

pub struct CharacterSelect {
    roster: CharacterRoster,
    view: View,
    /// Hovered button index (visual only).
    hover: Option<u32>,
    /// Time accumulator for the slow podium rotation.
    rotation_t: f32,
    /// What the preview avatar currently represents. `None` means no
    /// preview entity is alive in the world right now.
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

// ─── Layout constants ────────────────────────────────────────────────────
//
// All in screen-pixel units; the renderer scales.

const PANEL_BG: [f32; 4] = [0.05, 0.05, 0.07, 0.92];
const PANEL_BORDER: [f32; 4] = [0.18, 0.16, 0.12, 1.0];
const TEXT_PRIMARY: [f32; 4] = [0.95, 0.92, 0.84, 1.0];
const TEXT_DIM: [f32; 4] = [0.65, 0.62, 0.55, 1.0];
const TEXT_TITLE: [f32; 4] = [1.0, 0.84, 0.45, 1.0];
const BTN_BG: [f32; 4] = [0.10, 0.10, 0.13, 0.95];
const BTN_BG_HOVER: [f32; 4] = [0.18, 0.18, 0.22, 1.0];
const BTN_BG_PRIMARY: [f32; 4] = [0.55, 0.32, 0.10, 1.0];
const BTN_BG_PRIMARY_HOVER: [f32; 4] = [0.72, 0.45, 0.16, 1.0];
const BTN_BG_DANGER: [f32; 4] = [0.45, 0.10, 0.10, 1.0];
const BTN_BG_DANGER_HOVER: [f32; 4] = [0.65, 0.18, 0.18, 1.0];

impl CharacterSelect {
    pub fn new() -> Self {
        Self {
            roster: CharacterRoster::new(),
            view: View::Roster,
            hover: None,
            rotation_t: 0.0,
            preview_state: None,
            anim_clips: None,
        }
    }

    pub fn roster(&self) -> &CharacterRoster {
        &self.roster
    }

    /// Run one frame's worth of input + UI logic. Returns the action
    /// the surrounding `GameState` should perform.
    pub fn update(
        &mut self,
        world: &mut hecs::World,
        renderer: &mut Renderer,
        input: &Input,
        dt: f32,
    ) -> SelectAction {
        self.rotation_t += dt;
        self.hover = None;

        let (sw, sh) = renderer.screen_size();
        let mouse = input.mouse_pos();
        let clicked = input.left_clicked();

        // Make sure the right preview is alive for the active view.
        let desired_gender = self.desired_preview_gender();
        self.ensure_preview(world, renderer, desired_gender);

        // Drive camera at the podium.
        self.update_preview_camera(world, renderer);

        // Hit-test in display order: top-most UI first.
        let action = match &self.view.clone() {
            View::Roster => self.update_roster(mouse, clicked, sw, sh),
            View::Create(form) => self.update_create(form.clone(), input, mouse, clicked, sw, sh),
            View::DeleteConfirm { idx } => self.update_delete(*idx, mouse, clicked, sw, sh),
        };

        action
    }

    // ─── Preview management ──────────────────────────────────────────

    fn desired_preview_gender(&self) -> Option<Gender> {
        match &self.view {
            View::Create(form) => Some(form.gender),
            View::Roster => self.roster.slots().first().map(|p| p.gender),
            View::DeleteConfirm { idx } => self.roster.get(*idx).map(|p| p.gender),
        }
    }

    /// Drop the preview avatar entity and free its render-object slot.
    /// Called by `GameState` right before generating the hub so the
    /// dynamic mesh slot can be reclaimed (and so the preview model
    /// doesn't briefly appear inside the hub).
    pub fn teardown_preview(&mut self, world: &mut hecs::World, renderer: &mut Renderer) {
        if let Some(prev) = self.preview_state.take() {
            if let Ok(r) = world.get::<&Renderable>(prev.entity) {
                let idx = r.object_index;
                if idx < renderer.objects.len() {
                    renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
                }
            }
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
            (Some(_), None) => true, // tear down
            (None, Some(_)) => true, // build
            (Some(s), Some(g)) => s.gender != g,
        };
        if !needs_rebuild {
            return;
        }
        // Clear previous preview entity (and its render slot).
        if let Some(prev) = self.preview_state.take() {
            if let Ok(r) = world.get::<&Renderable>(prev.entity) {
                let idx = r.object_index;
                if idx < renderer.objects.len() {
                    renderer.objects[idx].model_matrix = glam::Mat4::ZERO;
                }
            }
            let _ = world.despawn(prev.entity);
        }
        // Spawn fresh preview at the podium.
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
        let (model_path, tex_path) = classes::base_model_paths(gender);
        let skinned = match SkinnedMesh::from_gltf(model_path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Preview model load failed ({:?}): {}", gender, e);
                return None;
            }
        };

        // Bind animation library to this skeleton.
        let clips = self.load_clips_lazy();
        let mut anim_set = AnimationSet::default();
        if let Some(clips) = clips {
            for clip in clips {
                let bound = clip.bind_to_skeleton(
                    &skinned.joint_index_by_name,
                    skinned.joints.len(),
                );
                anim_set
                    .clips
                    .insert(clip.name.to_ascii_lowercase(), Arc::new(bound));
            }
        }
        let idle_clip = anim_set
            .find_any(&["Idle_Loop", "Idle"])
            .or_else(|| anim_set.clips.values().next().cloned());
        let animator = idle_clip.map(Animator::new);

        // Add the bind-pose mesh to the renderer as a dynamic skinned object.
        let mut bind_mesh = Mesh::empty();
        bind_mesh.vertices = skinned.bind_vertices.clone();
        bind_mesh.indices = skinned.indices.clone();
        let podium_pos = Vec3::new(0.0, 0.0, 0.0);
        let obj_idx = match renderer.add_dynamic_mesh(&bind_mesh, Mat4::from_translation(podium_pos)) {
            Ok(i) => i,
            Err(e) => {
                log::warn!("Preview mesh upload failed: {}", e);
                return None;
            }
        };
        if let Err(e) = renderer.set_object_texture(obj_idx, tex_path) {
            log::warn!("Preview texture load failed: {}", e);
        }

        let comp = Skinned {
            mesh: Arc::new(skinned),
            scratch: Vec::new(),
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
        // Slowly rotate the podium and stand the camera in front of it.
        if let Some(prev) = &self.preview_state {
            let rot = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0))
                * Mat4::from_rotation_y(self.rotation_t * 0.35);
            // Update the entity's Transform too so it rotates in sync
            // (skinning_system reads Transform).
            if let Ok(mut t) = world.get::<&mut Transform>(prev.entity) {
                t.rotation = glam::Quat::from_rotation_y(self.rotation_t * 0.35);
            }
            // Also update the renderer model matrix in case skinning
            // hasn't kicked in yet on the very first frame.
            let idx = world
                .get::<&Renderable>(prev.entity)
                .map(|r| r.object_index)
                .unwrap_or(usize::MAX);
            if idx < renderer.objects.len() {
                renderer.objects[idx].model_matrix = rot;
            }
        }
        // Camera: shifted to the left of world-origin so the avatar
        // (which sits at origin) ends up on the right side of the
        // viewport, clear of the UI panel.
        renderer.camera.position = Vec3::new(OFFSET_X, 1.4, 3.6);
        renderer.camera.target = Vec3::new(OFFSET_X, 1.0, 0.0);
        // One-shot diagnostic.
        if self.rotation_t < 0.1 {
            if let Some(prev) = &self.preview_state {
                let idx = world.get::<&Renderable>(prev.entity)
                    .map(|r| r.object_index).unwrap_or(usize::MAX);
                if idx < renderer.objects.len() {
                    let m = renderer.objects[idx].model_matrix;
                    log::info!(
                        "Preview obj {}: center={:?} radius={} clear_color={:?}",
                        idx, m.w_axis.truncate(), renderer.objects[idx].bounds_radius,
                        renderer.clear_color,
                    );
                }
            }
        }
        // Brighter backdrop so the avatar stands out.
        renderer.clear_color = [0.10, 0.10, 0.14, 1.0];
        renderer.fog_color = [0.12, 0.12, 0.16];
        renderer.fog_start = 8.0;
        renderer.fog_end = 30.0;

        // Studio-style fill: warm key in front, cool rim from behind,
        // soft top-fill so the figure reads even with low ambient.
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

    // ─── View updates ────────────────────────────────────────────────

    fn update_roster(
        &mut self,
        mouse: (f32, f32),
        clicked: bool,
        sw: f32,
        sh: f32,
    ) -> SelectAction {
        // Layout: left half = roster panel, right half = preview backdrop.
        let panel_w = sw * 0.42;
        let panel_x = sw * 0.04;
        let panel_y = sh * 0.10;
        let panel_h = sh * 0.80;

        let row_h = 90.0;
        let row_pad = 12.0;
        let row_x = panel_x + 24.0;
        let row_w = panel_w - 48.0;

        let mut hover_id: Option<u32> = None;

        // Slot rows.
        for i in 0..MAX_CHARACTERS {
            let y = panel_y + 110.0 + (i as f32) * (row_h + row_pad);
            if let Some(profile) = self.roster.get(i) {
                let _ = profile;
                // Play button right side
                let play_btn = (row_x + row_w - 220.0, y + 24.0, 90.0, 40.0);
                if hit(mouse, play_btn) {
                    hover_id = Some(100 + i as u32);
                    if clicked {
                        let p = self.roster.slots()[i].clone();
                        return SelectAction::Play(p);
                    }
                }
                let del_btn = (row_x + row_w - 110.0, y + 24.0, 90.0, 40.0);
                if hit(mouse, del_btn) {
                    hover_id = Some(200 + i as u32);
                    if clicked {
                        self.view = View::DeleteConfirm { idx: i };
                        self.hover = hover_id;
                        return SelectAction::None;
                    }
                }
            } else if i == self.roster.len() && !self.roster.is_full() {
                // Create-new placeholder row
                let create_btn = (row_x, y, row_w, row_h);
                if hit(mouse, create_btn) {
                    hover_id = Some(900);
                    if clicked {
                        self.view = View::Create(CreateForm::new());
                        self.hover = hover_id;
                        return SelectAction::None;
                    }
                }
            }
        }

        // Quit button bottom-left of the panel.
        let quit_btn = (panel_x + 24.0, panel_y + panel_h - 60.0, 120.0, 40.0);
        if hit(mouse, quit_btn) {
            hover_id = Some(999);
            if clicked {
                return SelectAction::Quit;
            }
        }

        self.hover = hover_id;
        SelectAction::None
    }

    fn update_create(
        &mut self,
        mut form: CreateForm,
        input: &Input,
        mouse: (f32, f32),
        clicked: bool,
        sw: f32,
        sh: f32,
    ) -> SelectAction {
        let panel_w = sw * 0.42;
        let panel_x = sw * 0.04;
        let panel_y = sh * 0.10;
        let panel_h = sh * 0.80;

        // Name text input — always receives typed chars while in this view.
        for ch in input.chars_typed() {
            if form.name.chars().count() < 18 && !ch.is_control() {
                form.name.push(*ch);
            }
        }
        for _ in 0..input.backspace_count() {
            form.name.pop();
        }

        let mut hover_id: Option<u32> = None;

        // Gender toggle buttons.
        let male_btn = (panel_x + 30.0, panel_y + 200.0, 140.0, 50.0);
        let female_btn = (panel_x + 180.0, panel_y + 200.0, 140.0, 50.0);
        if hit(mouse, male_btn) {
            hover_id = Some(1);
            if clicked {
                form.gender = Gender::Male;
            }
        }
        if hit(mouse, female_btn) {
            hover_id = Some(2);
            if clicked {
                form.gender = Gender::Female;
            }
        }

        // Class cycler (left/right arrows).
        let class_count = class_options().len();
        let class_left = (panel_x + 30.0, panel_y + 320.0, 40.0, 40.0);
        let class_right = (panel_x + 280.0, panel_y + 320.0, 40.0, 40.0);
        if hit(mouse, class_left) {
            hover_id = Some(3);
            if clicked {
                form.class_idx = (form.class_idx + class_count - 1) % class_count;
            }
        }
        if hit(mouse, class_right) {
            hover_id = Some(4);
            if clicked {
                form.class_idx = (form.class_idx + 1) % class_count;
            }
        }

        // Confirm + Cancel.
        let can_confirm = !form.name.trim().is_empty();
        let confirm_btn = (panel_x + 30.0, panel_y + panel_h - 80.0, 160.0, 50.0);
        let cancel_btn = (panel_x + 210.0, panel_y + panel_h - 80.0, 120.0, 50.0);
        let enter_pressed = input.enter_just_pressed() && can_confirm;
        if hit(mouse, confirm_btn) {
            hover_id = Some(5);
            if clicked && can_confirm {
                let (class_id, _) = class_options()[form.class_idx];
                let profile = CharacterProfile::new(form.name.trim().to_string(), form.gender, class_id);
                self.roster.add(profile);
                self.view = View::Roster;
                self.hover = None;
                return SelectAction::None;
            }
        }
        if hit(mouse, cancel_btn) {
            hover_id = Some(6);
            if clicked {
                self.view = View::Roster;
                self.hover = None;
                return SelectAction::None;
            }
        }
        if enter_pressed {
            let (class_id, _) = class_options()[form.class_idx];
            let profile = CharacterProfile::new(form.name.trim().to_string(), form.gender, class_id);
            self.roster.add(profile);
            self.view = View::Roster;
            self.hover = None;
            return SelectAction::None;
        }

        // Persist form state edits.
        self.view = View::Create(form);
        self.hover = hover_id;
        SelectAction::None
    }

    fn update_delete(
        &mut self,
        idx: usize,
        mouse: (f32, f32),
        clicked: bool,
        sw: f32,
        sh: f32,
    ) -> SelectAction {
        let cx = sw * 0.5;
        let cy = sh * 0.5;
        let yes_btn = (cx - 140.0, cy + 30.0, 120.0, 50.0);
        let no_btn = (cx + 20.0, cy + 30.0, 120.0, 50.0);
        let mut hover_id: Option<u32> = None;
        if hit(mouse, yes_btn) {
            hover_id = Some(7);
            if clicked {
                self.roster.remove(idx);
                self.view = View::Roster;
                self.hover = None;
                return SelectAction::None;
            }
        }
        if hit(mouse, no_btn) {
            hover_id = Some(8);
            if clicked {
                self.view = View::Roster;
                self.hover = None;
                return SelectAction::None;
            }
        }
        self.hover = hover_id;
        SelectAction::None
    }

    // ─── Rendering ───────────────────────────────────────────────────

    pub fn render(&self, batch: &mut OverlayBatch, sw: f32, sh: f32) {
        // Backdrop dim across the right half so the avatar reads cleanly.
        batch.rect_px(0.0, 0.0, sw, sh, [0.0, 0.0, 0.0, 0.35], sw, sh);

        match &self.view {
            View::Roster => self.draw_roster(batch, sw, sh),
            View::Create(form) => self.draw_create(batch, form, sw, sh),
            View::DeleteConfirm { idx } => {
                self.draw_roster(batch, sw, sh);
                self.draw_delete_confirm(batch, *idx, sw, sh);
            }
        }
    }

    fn draw_roster(&self, batch: &mut OverlayBatch, sw: f32, sh: f32) {
        let panel_w = sw * 0.42;
        let panel_x = sw * 0.04;
        let panel_y = sh * 0.10;
        let panel_h = sh * 0.80;

        draw_panel(batch, panel_x, panel_y, panel_w, panel_h, sw, sh);
        batch.text(
            "CHARACTER SELECT",
            panel_x + 24.0,
            panel_y + 30.0,
            32.0,
            TEXT_TITLE,
            sw,
            sh,
        );
        batch.text(
            "Choose a character to enter the rift",
            panel_x + 24.0,
            panel_y + 70.0,
            14.0,
            TEXT_DIM,
            sw,
            sh,
        );

        let row_h = 90.0;
        let row_pad = 12.0;
        let row_x = panel_x + 24.0;
        let row_w = panel_w - 48.0;

        for i in 0..MAX_CHARACTERS {
            let y = panel_y + 110.0 + (i as f32) * (row_h + row_pad);
            if let Some(profile) = self.roster.get(i) {
                draw_panel_inner(batch, row_x, y, row_w, row_h, sw, sh);
                batch.text(&profile.name, row_x + 18.0, y + 14.0, 22.0, TEXT_PRIMARY, sw, sh);
                let sub = format!(
                    "Lv {}  -  {}  -  {}",
                    profile.level,
                    profile.gender.label(),
                    classes::config_for(profile.class).name,
                );
                batch.text(&sub, row_x + 18.0, y + 46.0, 14.0, TEXT_DIM, sw, sh);

                let play_hover = self.hover == Some(100 + i as u32);
                let del_hover = self.hover == Some(200 + i as u32);
                draw_button(
                    batch,
                    "Play",
                    (row_x + row_w - 220.0, y + 24.0, 90.0, 40.0),
                    if play_hover { BTN_BG_PRIMARY_HOVER } else { BTN_BG_PRIMARY },
                    sw,
                    sh,
                );
                draw_button(
                    batch,
                    "Delete",
                    (row_x + row_w - 110.0, y + 24.0, 90.0, 40.0),
                    if del_hover { BTN_BG_DANGER_HOVER } else { BTN_BG_DANGER },
                    sw,
                    sh,
                );
            } else if i == self.roster.len() && !self.roster.is_full() {
                let create_hover = self.hover == Some(900);
                draw_button(
                    batch,
                    "+ Create New Character",
                    (row_x, y, row_w, row_h),
                    if create_hover { BTN_BG_HOVER } else { BTN_BG },
                    sw,
                    sh,
                );
            } else {
                // Empty slot (locked or unused)
                draw_panel_dashed(batch, row_x, y, row_w, row_h, sw, sh);
                batch.text(
                    "(empty slot)",
                    row_x + 18.0,
                    y + row_h * 0.5 - 6.0,
                    14.0,
                    TEXT_DIM,
                    sw,
                    sh,
                );
            }
        }

        let quit_hover = self.hover == Some(999);
        draw_button(
            batch,
            "Quit",
            (panel_x + 24.0, panel_y + panel_h - 60.0, 120.0, 40.0),
            if quit_hover { BTN_BG_HOVER } else { BTN_BG },
            sw,
            sh,
        );
    }

    fn draw_create(&self, batch: &mut OverlayBatch, form: &CreateForm, sw: f32, sh: f32) {
        let panel_w = sw * 0.42;
        let panel_x = sw * 0.04;
        let panel_y = sh * 0.10;
        let panel_h = sh * 0.80;

        draw_panel(batch, panel_x, panel_y, panel_w, panel_h, sw, sh);
        batch.text(
            "CREATE CHARACTER",
            panel_x + 24.0,
            panel_y + 30.0,
            32.0,
            TEXT_TITLE,
            sw,
            sh,
        );

        // Name field.
        batch.text("Name", panel_x + 30.0, panel_y + 90.0, 16.0, TEXT_DIM, sw, sh);
        draw_panel_inner(batch, panel_x + 30.0, panel_y + 110.0, panel_w - 60.0, 50.0, sw, sh);
        let display = if form.name.is_empty() {
            "Type a name…".to_string()
        } else {
            // Add a blinking caret.
            let caret = if (self.rotation_t * 2.0) as i32 % 2 == 0 { "_" } else { " " };
            format!("{}{}", form.name, caret)
        };
        let name_color = if form.name.is_empty() {
            TEXT_DIM
        } else {
            TEXT_PRIMARY
        };
        batch.text(&display, panel_x + 46.0, panel_y + 124.0, 22.0, name_color, sw, sh);

        // Gender.
        batch.text("Gender", panel_x + 30.0, panel_y + 180.0, 16.0, TEXT_DIM, sw, sh);
        let male_btn = (panel_x + 30.0, panel_y + 200.0, 140.0, 50.0);
        let female_btn = (panel_x + 180.0, panel_y + 200.0, 140.0, 50.0);
        let male_active = form.gender == Gender::Male;
        let female_active = form.gender == Gender::Female;
        draw_button(
            batch,
            "Male",
            male_btn,
            if male_active {
                BTN_BG_PRIMARY
            } else if self.hover == Some(1) {
                BTN_BG_HOVER
            } else {
                BTN_BG
            },
            sw,
            sh,
        );
        draw_button(
            batch,
            "Female",
            female_btn,
            if female_active {
                BTN_BG_PRIMARY
            } else if self.hover == Some(2) {
                BTN_BG_HOVER
            } else {
                BTN_BG
            },
            sw,
            sh,
        );

        // Class.
        batch.text("Class", panel_x + 30.0, panel_y + 300.0, 16.0, TEXT_DIM, sw, sh);
        let (_, class_name) = class_options()[form.class_idx];
        draw_button(
            batch,
            "<",
            (panel_x + 30.0, panel_y + 320.0, 40.0, 40.0),
            if self.hover == Some(3) { BTN_BG_HOVER } else { BTN_BG },
            sw,
            sh,
        );
        draw_panel_inner(batch, panel_x + 80.0, panel_y + 320.0, 190.0, 40.0, sw, sh);
        batch.text(class_name, panel_x + 100.0, panel_y + 332.0, 18.0, TEXT_PRIMARY, sw, sh);
        draw_button(
            batch,
            ">",
            (panel_x + 280.0, panel_y + 320.0, 40.0, 40.0),
            if self.hover == Some(4) { BTN_BG_HOVER } else { BTN_BG },
            sw,
            sh,
        );

        // Confirm + Cancel.
        let can_confirm = !form.name.trim().is_empty();
        let confirm_color = if can_confirm {
            if self.hover == Some(5) {
                BTN_BG_PRIMARY_HOVER
            } else {
                BTN_BG_PRIMARY
            }
        } else {
            [0.25, 0.20, 0.12, 1.0]
        };
        draw_button(
            batch,
            "Confirm",
            (panel_x + 30.0, panel_y + panel_h - 80.0, 160.0, 50.0),
            confirm_color,
            sw,
            sh,
        );
        draw_button(
            batch,
            "Cancel",
            (panel_x + 210.0, panel_y + panel_h - 80.0, 120.0, 50.0),
            if self.hover == Some(6) { BTN_BG_HOVER } else { BTN_BG },
            sw,
            sh,
        );
    }

    fn draw_delete_confirm(&self, batch: &mut OverlayBatch, idx: usize, sw: f32, sh: f32) {
        let cx = sw * 0.5;
        let cy = sh * 0.5;
        let mw = 460.0;
        let mh = 200.0;
        // Modal backdrop.
        batch.rect_px(0.0, 0.0, sw, sh, [0.0, 0.0, 0.0, 0.55], sw, sh);
        draw_panel(batch, cx - mw * 0.5, cy - mh * 0.5, mw, mh, sw, sh);
        let name = self
            .roster
            .get(idx)
            .map(|p| p.name.as_str())
            .unwrap_or("?");
        batch.text("Delete character?", cx - 110.0, cy - 70.0, 22.0, TEXT_TITLE, sw, sh);
        let line = format!("\"{}\" will be permanently removed.", name);
        batch.text(&line, cx - 180.0, cy - 30.0, 14.0, TEXT_DIM, sw, sh);

        draw_button(
            batch,
            "Delete",
            (cx - 140.0, cy + 30.0, 120.0, 50.0),
            if self.hover == Some(7) { BTN_BG_DANGER_HOVER } else { BTN_BG_DANGER },
            sw,
            sh,
        );
        draw_button(
            batch,
            "Cancel",
            (cx + 20.0, cy + 30.0, 120.0, 50.0),
            if self.hover == Some(8) { BTN_BG_HOVER } else { BTN_BG },
            sw,
            sh,
        );
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn hit(mouse: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    let (mx, my) = mouse;
    let (x, y, w, h) = rect;
    mx >= x && mx <= x + w && my >= y && my <= y + h
}

fn draw_panel(batch: &mut OverlayBatch, x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32) {
    batch.rect_px(x, y, w, h, PANEL_BG, sw, sh);
    // 2px border using four thin rects.
    batch.rect_px(x, y, w, 2.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y + h - 2.0, w, 2.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y, 2.0, h, PANEL_BORDER, sw, sh);
    batch.rect_px(x + w - 2.0, y, 2.0, h, PANEL_BORDER, sw, sh);
}

fn draw_panel_inner(batch: &mut OverlayBatch, x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32) {
    batch.rect_px(x, y, w, h, [0.10, 0.10, 0.13, 0.95], sw, sh);
    batch.rect_px(x, y, w, 1.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y + h - 1.0, w, 1.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y, 1.0, h, PANEL_BORDER, sw, sh);
    batch.rect_px(x + w - 1.0, y, 1.0, h, PANEL_BORDER, sw, sh);
}

fn draw_panel_dashed(batch: &mut OverlayBatch, x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32) {
    batch.rect_px(x, y, w, h, [0.06, 0.06, 0.08, 0.5], sw, sh);
    let dash: f32 = 6.0;
    let gap: f32 = 4.0;
    let mut cx = x;
    while cx < x + w {
        let segw = (dash).min(x + w - cx);
        batch.rect_px(cx, y, segw, 1.0, PANEL_BORDER, sw, sh);
        batch.rect_px(cx, y + h - 1.0, segw, 1.0, PANEL_BORDER, sw, sh);
        cx += dash + gap;
    }
}

fn draw_button(
    batch: &mut OverlayBatch,
    label: &str,
    rect: (f32, f32, f32, f32),
    bg: [f32; 4],
    sw: f32,
    sh: f32,
) {
    let (x, y, w, h) = rect;
    batch.rect_px(x, y, w, h, bg, sw, sh);
    batch.rect_px(x, y, w, 1.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y + h - 1.0, w, 1.0, PANEL_BORDER, sw, sh);
    batch.rect_px(x, y, 1.0, h, PANEL_BORDER, sw, sh);
    batch.rect_px(x + w - 1.0, y, 1.0, h, PANEL_BORDER, sw, sh);
    let label_size = 18.0;
    let text_w = batch.measure_text(label, label_size);
    let tx = x + (w - text_w) * 0.5;
    let ty = y + (h - label_size) * 0.5;
    batch.text(label, tx, ty, label_size, TEXT_PRIMARY, sw, sh);
}
