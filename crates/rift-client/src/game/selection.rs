use glam::Vec3;
use rift_engine::ecs::components::{
    LocalPlayer, Player, RemoteEnemy, RemoteMinion, RemotePlayer, Renderable, SkinnedAttachments,
    Transform,
};
use rift_engine::ui::im::{Id, Rect, Ui};
use rift_engine::{Input, Renderer};
use rift_game::monsters::MonsterRole;
use rift_net::messages::EntityKind;
use rift_net::NetId;
use std::collections::HashMap;

use crate::game::chat::ChatUi;
use crate::game::sub_state::NetState;
use crate::game::unit_frame::{
    apply_unit_context_action, draw_unit_context_menu, draw_unit_frame,
    unit_context_menu_should_close, UnitContextMenuState, UnitFrameBars, UnitFrameData,
};
use crate::net::RemoteEntity;

const SELECTED_FLAG: u32 = 16;
const HOVERED_FLAG: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectableKind {
    OwnPlayer,
    OtherPlayer,
    Enemy,
    Minion,
    Loot,
    Interactable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionRelation {
    SelfUnit,
    Friendly,
    Hostile,
    Neutral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRef {
    pub net_id: NetId,
    pub kind: SelectableKind,
    pub relation: SelectionRelation,
}

#[derive(Clone, Debug)]
pub struct SelectionCandidate {
    pub reference: SelectionRef,
    pub position: Vec3,
    pub radius: f32,
    pub health_pct: f32,
    pub resource_pct: f32,
    pub display_name: Option<String>,
    pub target_net_id: Option<NetId>,
    pub owner_net_id: Option<NetId>,
    object_indices: Vec<usize>,
}

#[derive(Default)]
pub struct SelectionState {
    candidates: Vec<SelectionCandidate>,
    hovered: Option<SelectionRef>,
    selected: Option<SelectionRef>,
    outlined_objects: Vec<usize>,
    context_menu: Option<UnitContextMenuState>,
}

impl SelectionState {
    pub fn hovered(&self) -> Option<SelectionRef> {
        self.hovered
    }

    pub fn selected(&self) -> Option<SelectionRef> {
        self.selected
    }

    pub fn clear_selected(&mut self) {
        self.selected = None;
        self.context_menu = None;
    }

    pub fn selected_target(&self) -> Option<SelectionRef> {
        let selected = self.selected?;
        let target_id = self.by_id(selected.net_id)?.target_net_id?;
        self.by_id(target_id).map(|candidate| candidate.reference)
    }

    pub fn target_for_ability(
        &self,
        relation: SelectionRelation,
        range: f32,
        caster: Vec3,
    ) -> Option<NetId> {
        self.selected
            .and_then(|selected| self.valid_target(selected, relation, range, caster))
            .or_else(|| {
                self.hovered
                    .and_then(|hovered| self.valid_target(hovered, relation, range, caster))
            })
    }

    pub fn refresh(
        &mut self,
        world: &hecs::World,
        remote: &std::collections::HashMap<NetId, RemoteEntity>,
        names: &HashMap<NetId, String>,
        our_net_id: Option<NetId>,
    ) {
        self.candidates.clear();

        if let Some(net_id) = our_net_id {
            for (_, (transform, _, _, renderable, attachments)) in world
                .query::<(
                    &Transform,
                    &Player,
                    &LocalPlayer,
                    &Renderable,
                    Option<&SkinnedAttachments>,
                )>()
                .iter()
            {
                self.candidates.push(SelectionCandidate {
                    reference: SelectionRef {
                        net_id,
                        kind: SelectableKind::OwnPlayer,
                        relation: SelectionRelation::SelfUnit,
                    },
                    position: transform.position,
                    radius: 1.0,
                    health_pct: 1.0,
                    resource_pct: 1.0,
                    display_name: names.get(&net_id).cloned(),
                    target_net_id: None,
                    owner_net_id: None,
                    object_indices: object_indices(renderable, attachments),
                });
                break;
            }
        }

        let mut remote_objects: HashMap<NetId, Vec<usize>> = HashMap::new();
        for (_, (_, remote, renderable, attachments)) in world
            .query::<(
                &Player,
                &RemotePlayer,
                &Renderable,
                Option<&SkinnedAttachments>,
            )>()
            .iter()
        {
            remote_objects.insert(
                NetId(remote.net_id),
                object_indices(renderable, attachments),
            );
        }
        for (_, (remote, renderable, attachments)) in world
            .query::<(&RemoteEnemy, &Renderable, Option<&SkinnedAttachments>)>()
            .iter()
        {
            remote_objects.insert(
                NetId(remote.net_id),
                object_indices(renderable, attachments),
            );
        }
        for (_, (remote, renderable, attachments)) in world
            .query::<(&RemoteMinion, &Renderable, Option<&SkinnedAttachments>)>()
            .iter()
        {
            remote_objects.insert(
                NetId(remote.net_id),
                object_indices(renderable, attachments),
            );
        }

        for remote_entity in remote.values() {
            if Some(remote_entity.net_id) == our_net_id {
                continue;
            }
            let (kind, relation, owner_net_id, radius, fallback_name) = match &remote_entity.kind {
                EntityKind::Player { .. } => (
                    SelectableKind::OtherPlayer,
                    SelectionRelation::Friendly,
                    None,
                    1.0,
                    None,
                ),
                EntityKind::Enemy { role, .. } => (
                    SelectableKind::Enemy,
                    SelectionRelation::Hostile,
                    None,
                    1.15,
                    monster_name(*role),
                ),
                EntityKind::Minion { owner, .. } => (
                    SelectableKind::Minion,
                    if Some(*owner) == our_net_id {
                        SelectionRelation::Friendly
                    } else {
                        SelectionRelation::Neutral
                    },
                    Some(*owner),
                    0.8,
                    None,
                ),
                EntityKind::Loot { .. } => (
                    SelectableKind::Loot,
                    SelectionRelation::Neutral,
                    None,
                    0.75,
                    None,
                ),
                EntityKind::ReviveShrine { .. } => (
                    SelectableKind::Interactable,
                    SelectionRelation::Neutral,
                    None,
                    1.2,
                    Some("Revive Shrine".to_string()),
                ),
                EntityKind::Projectile { .. } | EntityKind::AoeZone { .. } => continue,
            };
            self.candidates.push(SelectionCandidate {
                reference: SelectionRef {
                    net_id: remote_entity.net_id,
                    kind,
                    relation,
                },
                position: remote_entity.position,
                radius,
                health_pct: remote_entity.health_pct,
                resource_pct: remote_entity.resource_pct,
                display_name: names.get(&remote_entity.net_id).cloned().or(fallback_name),
                target_net_id: remote_entity.target_net_id,
                owner_net_id,
                object_indices: remote_objects
                    .get(&remote_entity.net_id)
                    .cloned()
                    .unwrap_or_default(),
            });
        }

        self.prune_stale_refs();
    }

    pub fn tick(&mut self, input: &Input, renderer: &mut Renderer) {
        self.hovered = self
            .pick_mesh(input, renderer)
            .map(|candidate| candidate.reference);
        if input.left_just_pressed() {
            if let Some(hovered) = self.hovered {
                let _ = input.left_clicked();
                self.selected = Some(hovered);
            }
        }
        self.update_outline(renderer);
    }

    pub fn candidate(&self, reference: SelectionRef) -> Option<&SelectionCandidate> {
        self.by_id(reference.net_id)
    }

    pub fn display_name_for_net_id(&self, net_id: NetId) -> Option<&str> {
        self.by_id(net_id)
            .and_then(|candidate| candidate.display_name.as_deref())
    }

    pub fn frame_target_ui(
        &mut self,
        ui: &mut Ui<'_>,
        net: &mut NetState,
        chat: &mut ChatUi,
        dt: f32,
        consume_rects: &mut Vec<Rect>,
    ) {
        let Some(selected_ref) = self.selected else {
            self.context_menu = None;
            return;
        };
        let Some(candidate) = self.by_id(selected_ref.net_id).cloned() else {
            self.context_menu = None;
            return;
        };

        let theme = *ui.theme();
        let s = theme.scale;
        let screen = ui.screen_size();
        let (rect, bars_panel) = target_frame_layout(screen.x, screen.y, s);
        consume_rects.push(rect);
        let name = candidate
            .display_name
            .as_deref()
            .unwrap_or_else(|| kind_label(candidate.reference.kind));
        let (hp_displayed, hp_trail, hp_pulse, res_displayed, res_trail, res_pulse) = {
            let state = ui.state_mut();
            let anims = state
                .world_vitals
                .entry(candidate.reference.net_id.0 as u64)
                .or_default();
            anims.hp.tick(candidate.health_pct, dt);
            anims.essence.tick(candidate.resource_pct, dt);
            (
                anims.hp.displayed,
                anims.hp.trail,
                anims.hp.pulse,
                anims.essence.displayed,
                anims.essence.trail,
                anims.essence.pulse,
            )
        };
        draw_unit_frame(
            ui,
            bars_panel,
            UnitFrameData {
                name,
                detail: None,
                bars: UnitFrameBars {
                    health_displayed: hp_displayed,
                    health_trail: hp_trail,
                    health_pulse: hp_pulse,
                    resource_displayed: Some(res_displayed),
                    resource_trail: res_trail,
                    resource_pulse: res_pulse,
                },
            },
        );
        if rect.contains(ui.mouse_pos()) && ui.input().right_clicked() {
            if let Some(name) = candidate.display_name.clone() {
                if candidate.reference.kind == SelectableKind::OtherPlayer
                    && candidate.reference.relation == SelectionRelation::Friendly
                {
                    self.context_menu =
                        Some(UnitContextMenuState::friendly_target(name, ui.mouse_pos()));
                }
            }
        }

        self.draw_context_menu(ui, net, chat, consume_rects);
    }

    fn draw_context_menu(
        &mut self,
        ui: &mut Ui<'_>,
        net: &mut NetState,
        chat: &mut ChatUi,
        consume_rects: &mut Vec<Rect>,
    ) {
        let Some(menu) = self.context_menu.clone() else {
            return;
        };
        if let Some(action) = draw_unit_context_menu(
            ui,
            &menu,
            Id::root("selection::unit_context"),
            consume_rects,
        ) {
            apply_unit_context_action(action, &menu.target, ui, net, chat);
            self.context_menu = None;
        } else if unit_context_menu_should_close(ui, &menu) {
            self.context_menu = None;
        }
    }

    fn valid_target(
        &self,
        reference: SelectionRef,
        relation: SelectionRelation,
        range: f32,
        caster: Vec3,
    ) -> Option<NetId> {
        let candidate = self.by_id(reference.net_id)?;
        if candidate.reference.relation != relation
            && !(relation == SelectionRelation::Friendly
                && candidate.reference.relation == SelectionRelation::SelfUnit)
        {
            return None;
        }
        let dx = candidate.position.x - caster.x;
        let dz = candidate.position.z - caster.z;
        if dx * dx + dz * dz > range * range {
            return None;
        }
        Some(candidate.reference.net_id)
    }

    fn pick_mesh(&self, input: &Input, renderer: &Renderer) -> Option<&SelectionCandidate> {
        let Some((origin, dir)) = cursor_ray(input, renderer) else {
            return None;
        };
        let mut best: Option<(&SelectionCandidate, f32)> = None;
        for candidate in &self.candidates {
            let mut candidate_hit: Option<f32> = None;
            for &obj_idx in &candidate.object_indices {
                let Some(obj) = renderer.objects.get(obj_idx) else {
                    continue;
                };
                let center = obj.model_matrix.w_axis.truncate() + Vec3::Y * candidate.radius * 0.72;
                let radius = (candidate.radius * 0.78)
                    .max(0.35)
                    .min(obj.bounds_radius.max(0.35));
                if let Some(t) = ray_sphere(origin, dir, center, radius) {
                    candidate_hit = Some(candidate_hit.map_or(t, |best| best.min(t)));
                }
            }
            if let Some(t) = candidate_hit {
                if best.map_or(true, |(_, best_t)| t < best_t) {
                    best = Some((candidate, t));
                }
            }
        }
        best.map(|(candidate, _)| candidate)
    }

    fn by_id(&self, net_id: NetId) -> Option<&SelectionCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.reference.net_id == net_id)
    }

    fn prune_stale_refs(&mut self) {
        if self
            .selected
            .and_then(|selected| self.by_id(selected.net_id))
            .is_none()
        {
            self.selected = None;
        }
        if self
            .hovered
            .and_then(|hovered| self.by_id(hovered.net_id))
            .is_none()
        {
            self.hovered = None;
        }
    }

    fn update_outline(&mut self, renderer: &mut Renderer) {
        for obj_idx in self.outlined_objects.drain(..) {
            set_object_flag(renderer, obj_idx, SELECTED_FLAG, false);
            set_object_flag(renderer, obj_idx, HOVERED_FLAG, false);
        }
        let selected = self
            .selected
            .and_then(|reference| self.by_id(reference.net_id));
        if let Some(selected) = selected {
            if let Some(obj_idx) = selected.object_indices.first().copied() {
                set_object_flag(renderer, obj_idx, SELECTED_FLAG, true);
                self.outlined_objects.push(obj_idx);
            }
        }
        let hovered = self
            .hovered
            .and_then(|reference| self.by_id(reference.net_id));
        if let Some(hovered) = hovered {
            let is_selected = self
                .selected
                .is_some_and(|selected| selected.net_id == hovered.reference.net_id);
            if !is_selected {
                let indices = hovered.object_indices.clone();
                for obj_idx in indices {
                    set_object_flag(renderer, obj_idx, HOVERED_FLAG, true);
                    self.outlined_objects.push(obj_idx);
                }
            }
        }
    }
}

fn cursor_ray(input: &Input, renderer: &Renderer) -> Option<(Vec3, Vec3)> {
    let (screen_w, screen_h) = renderer.screen_size();
    if screen_w <= 1.0 || screen_h <= 1.0 {
        return None;
    }
    let (mx, my) = input.mouse_pos();
    let ndc_x = (mx / screen_w) * 2.0 - 1.0;
    let ndc_y = (my / screen_h) * 2.0 - 1.0;
    let inv_vp = renderer.camera.view_projection().inverse();
    let near = inv_vp.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inv_vp.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
    let dir = (far - near).normalize_or_zero();
    if dir.length_squared() <= 0.001 {
        return None;
    }
    Some((near, dir))
}

fn ray_sphere(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    if t >= 0.0 {
        Some(t)
    } else {
        None
    }
}

fn object_indices(renderable: &Renderable, attachments: Option<&SkinnedAttachments>) -> Vec<usize> {
    let mut indices = vec![renderable.object_index];
    if let Some(attachments) = attachments {
        indices.extend(attachments.pieces.iter().map(|piece| piece.object_index));
    }
    indices
}

fn set_object_flag(renderer: &mut Renderer, obj_idx: usize, flag: u32, enabled: bool) {
    if let Some(obj) = renderer.objects.get_mut(obj_idx) {
        let mut bits = obj.material_params[2].to_bits();
        if enabled {
            bits |= flag;
        } else {
            bits &= !flag;
        }
        obj.material_params[2] = f32::from_bits(bits);
    }
}

fn monster_name(role: u8) -> Option<String> {
    MonsterRole::from_wire_byte(role).map(|role| role.display_name().to_string())
}

fn target_frame_layout(screen_w: f32, screen_h: f32, s: f32) -> (Rect, Rect) {
    let plaque_w = rift_ui::hud::PLAQUE_W_BASE * s;
    let plaque_h = rift_ui::hud::PLAQUE_H_BASE * s;
    let plaque_x = (screen_w - plaque_w) * 0.5;
    let plaque_y = screen_h - rift_ui::hud::VITALS_BOTTOM_OFFSET_BASE * s - plaque_h;
    let w = (286.0 * s).min(plaque_w);
    let h = 58.0 * s;
    let gap = 54.0 * s;
    let rect = Rect::from_xywh(plaque_x + plaque_w - w, plaque_y - gap - h, w, h);
    (rect, rect)
}

fn kind_label(kind: SelectableKind) -> &'static str {
    match kind {
        SelectableKind::OwnPlayer => "You",
        SelectableKind::OtherPlayer => "Player",
        SelectableKind::Enemy => "Enemy",
        SelectableKind::Minion => "Minion",
        SelectableKind::Loot => "Loot",
        SelectableKind::Interactable => "Interactable",
    }
}
