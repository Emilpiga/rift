//! Avatar head cosmetics — eyes, eyebrows, hair.
//!
//! The `Superhero_*_FullBody.gltf` base-character files ship three
//! sibling skinned meshes under one skin: the body itself, plus
//! `Eyes` and `Eyebrows`. The base-mesh loader merges every
//! primitive that uses the skin into a single buffer with one
//! material, so previously the eye / eyebrow geometry got stained
//! by whatever texture we set on the body. Hair is shipped as a
//! separate gltf rigged to the `Head` joint.
//!
//! We solve that here by:
//!   * loading the body with a name-filter that keeps only the
//!     body sibling (see `is_body_mesh_name`);
//!   * loading `Eyes`, `Eyebrows`, and the per-gender hair pick
//!     as their own `SkinnedMesh`es, remapping their joints onto
//!     the host skeleton, and pushing them as `AttachmentPiece`s
//!     so the existing skinning + render path picks them up;
//!   * forcing the eyes mesh's vertex colors to white and not
//!     setting a custom texture on its render slot, so the
//!     renderer's default 1×1 white texture leaves the eyeballs
//!     pure white;
//!   * applying the hair texture to both the eyebrow and hair
//!     pieces so they share a single hair color.
//!
//! Tag values are kept in a high range (`COSMETIC_TAG_BASE`+) so
//! they never collide with the equipment-slot tags used by
//! `equipment_visuals`. The pieces survive equip/unequip churn
//! untouched because that pass only reconciles slot tags it
//! recognises.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use hecs::Entity;

use rift_engine::ecs::components::{AttachmentPiece, Skinned, SkinnedAttachments, Transform};
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::Renderer;

use rift_game::character::{CharacterAppearance, Gender};

/// Tag values for cosmetic attachment pieces. Chosen well above
/// any plausible equipment-slot id so `apply_equipment_visuals`
/// never tries to hide / replace them. `COSMETIC_TAG_BASE` is
/// the lower bound the equipment-visual reconciler uses to
/// identify "leave this piece alone".
pub const COSMETIC_TAG_BASE: u32 = 200;
const TAG_EYES: u32 = 200;
const TAG_EYEBROWS: u32 = 201;
const TAG_HAIR: u32 = 202;

/// Path to the gendered base-character gltf that owns the
/// `Eyes` / `Eyebrows` sibling meshes.
fn base_character_path(gender: Gender) -> &'static str {
    match gender {
        Gender::Female => {
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Female_FullBody.gltf"
        }
        Gender::Male => {
            "assets/models/base-characters/Base Characters/Godot - UE/Superhero_Male_FullBody.gltf"
        }
    }
}

/// Per-gender hair pick. Files live under
/// `Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)/`. Their
/// only joint is `Head`, which is also a joint in the base
/// skeleton, so the standard remap covers them.
///
/// We deliberately avoid the hair-card assets (`Hair_Long`,
/// `Hair_SimpleParted`, the body's own `Eyebrows`) because the
/// renderer doesn't do alpha-cutout: those would draw their card
/// rectangles as opaque white planes, which looks awful. The
/// chosen meshes (`Hair_Buns`, `Hair_Buzzed`) are solid
/// geometry that reads correctly without alpha support.
fn hair_path(gender: Gender, style: u8) -> &'static str {
    match (gender, style % 3) {
        (Gender::Female, 0) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_Buns.gltf"
        ),
        (Gender::Female, 1) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_BuzzedFemale.gltf"
        ),
        (Gender::Female, _) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_Buzzed.gltf"
        ),
        (Gender::Male, 0) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_Buzzed.gltf"
        ),
        (Gender::Male, 1) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_Beard.gltf"
        ),
        (Gender::Male, _) => concat!(
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)",
            "/Hair_Buns.gltf"
        ),
    }
}

fn eyebrow_path(style: u8) -> &'static str {
    match style % 2 {
        0 => {
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)/Eyebrows_Regular.gltf"
        }
        _ => {
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)/Eyebrows_Female.gltf"
        }
    }
}

/// Solid hair tint. The hair gltfs ship with hair-card
/// textures whose alpha channel encodes strand cutout — without
/// alpha-cutout in the shader, sampling that texture stains
/// every hair vertex bright white. Tinting the vertex color and
/// not binding the texture at all gives clean solid-color hair
/// that matches what these solid-mesh styles want anyway.
fn hair_tint(color: u8) -> Vec3 {
    hsv_to_rgb(color as f32 / 255.0, 0.72, 0.78)
}

pub fn skin_tint(style: u8) -> Vec3 {
    match style % 10 {
        0 => Vec3::new(1.08, 0.96, 0.86),
        1 => Vec3::new(1.00, 0.86, 0.72),
        2 => Vec3::new(0.94, 0.74, 0.56),
        3 => Vec3::new(0.84, 0.62, 0.44),
        4 => Vec3::new(0.72, 0.50, 0.34),
        5 => Vec3::new(0.60, 0.40, 0.27),
        6 => Vec3::new(0.48, 0.31, 0.22),
        7 => Vec3::new(1.04, 0.90, 0.74),
        8 => Vec3::new(0.90, 0.67, 0.50),
        _ => Vec3::new(0.38, 0.24, 0.18),
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Vec3 {
    let h = h.fract() * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 {
        0 => Vec3::new(v, t, p),
        1 => Vec3::new(q, v, p),
        2 => Vec3::new(p, v, t),
        3 => Vec3::new(p, q, v),
        4 => Vec3::new(t, p, v),
        _ => Vec3::new(v, p, q),
    }
}

pub fn apply_body_shape(mesh: &mut SkinnedMesh, gender: Gender, appearance: CharacterAppearance) {
    if gender != Gender::Female {
        return;
    }
    let amount = (appearance.chest_size as f32 / 255.0 - 0.5) * 0.76;
    if amount.abs() < 0.01 || mesh.bind_vertices.is_empty() {
        return;
    }

    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for v in &mesh.bind_vertices {
        mn = mn.min(v.position);
        mx = mx.max(v.position);
    }
    let height = (mx.y - mn.y).max(0.001);
    let half_width = ((mx.x - mn.x) * 0.5).max(0.001);
    let depth = (mx.z - mn.z).max(0.001);
    let mid_z = (mn.z + mx.z) * 0.5;

    for v in &mut mesh.bind_vertices {
        let p = v.position;
        let y_t = (p.y - mn.y) / height;
        let chest_y = smoothstep(0.58, 0.66, y_t) * (1.0 - smoothstep(0.70, 0.79, y_t));
        let front = smoothstep(mid_z + depth * 0.10, mx.z, p.z);
        let side = (1.0 - (p.x / half_width).abs().powf(1.7)).clamp(0.0, 1.0);
        let w = chest_y * front * side;
        if w <= 0.0 {
            continue;
        }
        v.position.z += amount * w;
        v.position.x *= 1.0 + amount.max(0.0) * w * 0.08;
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(0.001)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// True when a glTF primitive belongs to the body sibling (and
/// *not* the `Eyes` / `Eyebrows` siblings) inside one of the
/// base-character glTFs. Receives both the node and mesh name
/// because exporters disagree about which one carries the
/// meaningful label (see `SkinnedMesh::from_gltf_filtered`).
/// Excludes anything containing "eye" or "brow" in *either*
/// name so eyes/eyebrows aren't pulled into the body load on
/// the male asset (where the meshes are named
/// `Face`/`Face.001`).
pub fn is_body_mesh_name(node: &str, mesh: &str) -> bool {
    let n = node.to_ascii_lowercase();
    let m = mesh.to_ascii_lowercase();
    !n.contains("eye") && !n.contains("brow") && !m.contains("eye") && !m.contains("brow")
}

/// Process-wide cache of remapped cosmetic meshes. Keyed by
/// `(path, mesh_name_filter, host_joint_count)` so the body
/// gltf can produce two distinct entries (Eyes, Eyebrows) and
/// repeat avatars share a single Arc.
#[derive(Default)]
pub struct AvatarCosmeticsCache {
    meshes: HashMap<(String, String, usize), Arc<SkinnedMesh>>,
    /// Negative cache: entries we've already tried and failed
    /// to load. Avoids per-frame warn-spam (and per-frame gltf
    /// re-parsing) for character-select avatars whose assets
    /// don't include an `Eyes` / `Eyebrows` sub-mesh.
    failed: std::collections::HashSet<(String, String, usize)>,
}

impl AvatarCosmeticsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Specialised eye-mesh loader: pulls just the `Eyes`
    /// sub-mesh out of the base-character gltf, force-tints
    /// every vertex pure white, then caches the result under a
    /// synthetic key so later avatars of the same skeleton
    /// reuse the Arc.
    fn fetch_eyes_white(
        &mut self,
        body_path: &str,
        host_joint_names: &HashMap<String, u16>,
        host_joint_count: usize,
    ) -> Option<Arc<SkinnedMesh>> {
        let key = (
            body_path.to_string(),
            "Eyes#white".to_string(),
            host_joint_count,
        );
        if let Some(m) = self.meshes.get(&key) {
            return Some(m.clone());
        }
        if self.failed.contains(&key) {
            return None;
        }
        let mut mesh = match SkinnedMesh::from_gltf_filtered(body_path, |node, mesh| {
            node == "Eyes" || mesh == "Eyes"
        }) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("avatar eyes load failed for {:?}: {}", body_path, e);
                self.failed.insert(key);
                return None;
            }
        };
        if !mesh.remap_joint_indices_to(host_joint_names) {
            log::warn!("avatar eyes joint remap failed for {:?}", body_path);
            self.failed.insert(key);
            return None;
        }
        mesh.override_vertex_colors(Vec3::ONE);
        let arc = Arc::new(mesh);
        self.meshes.insert(key, arc.clone());
        Some(arc)
    }

    /// Load a hair-style gltf and replace its vertex colors with
    /// `tint`, so the renderer reads a clean solid color (the
    /// renderer can't do alpha-cutout, so binding the original
    /// hair-card texture would just stain the geometry white).
    fn fetch_hair_tinted(
        &mut self,
        path: &str,
        tint: Vec3,
        host_joint_names: &HashMap<String, u16>,
        host_joint_count: usize,
    ) -> Option<Arc<SkinnedMesh>> {
        // Tint encoded in the cache key so a re-tint (different
        // gender visiting same path) doesn't collide.
        let key_name = format!("tint:{:.3},{:.3},{:.3}", tint.x, tint.y, tint.z);
        let key = (path.to_string(), key_name, host_joint_count);
        if let Some(m) = self.meshes.get(&key) {
            return Some(m.clone());
        }
        if self.failed.contains(&key) {
            return None;
        }
        let mut mesh = match SkinnedMesh::from_gltf(path) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("avatar hair load failed for {:?}: {}", path, e);
                self.failed.insert(key);
                return None;
            }
        };
        if !mesh.remap_joint_indices_to(host_joint_names) {
            log::warn!("avatar hair joint remap failed for {:?}", path);
            self.failed.insert(key);
            return None;
        }
        mesh.override_vertex_colors(tint);
        let arc = Arc::new(mesh);
        self.meshes.insert(key, arc.clone());
        Some(arc)
    }
}

/// Description of a cosmetic piece we want attached, resolved
/// to a remapped mesh and the texture (if any) to apply once the
/// dynamic-mesh slot is registered.
struct CosmeticPiece {
    tag: u32,
    mesh: Arc<SkinnedMesh>,
    /// `None` => leave the renderer's default 1×1 white texture
    /// in place. Used for the eyeballs.
    texture: Option<&'static str>,
}

/// Load the head-cosmetic and hair pieces for `gender` and push
/// them onto `entity`'s `SkinnedAttachments`. Idempotent — a
/// second call with the same gender does nothing because each
/// cosmetic tag is matched against the existing pieces, and a
/// second arc-equal mesh would short-circuit anyway. Safe to
/// call after the entity has equipment-visual pieces attached;
/// the cosmetic tag namespace is disjoint.
pub fn apply_avatar_cosmetics(
    world: &mut hecs::World,
    renderer: &mut Renderer,
    cache: &mut AvatarCosmeticsCache,
    entity: Entity,
    gender: Gender,
    appearance: CharacterAppearance,
) {
    // Pull host skeleton info up front so we can drop the borrow
    // before mutating attachments / the renderer.
    let (host_joint_names, host_joint_count) = match world.get::<&Skinned>(entity) {
        Ok(s) => (s.mesh.joint_index_by_name.clone(), s.mesh.joints.len()),
        Err(_) => return,
    };
    let host_xform = world
        .get::<&Transform>(entity)
        .map(|t| t.matrix())
        .unwrap_or(Mat4::IDENTITY);

    let body_path = base_character_path(gender);
    let hair_p = hair_path(gender, appearance.hair_style);
    let eyebrow_p = eyebrow_path(appearance.eyebrow_style);
    let hair_color = hair_tint(appearance.hair_color);
    let eyebrow_color = hair_tint(appearance.eyebrow_color);

    let mut pieces: Vec<CosmeticPiece> = Vec::with_capacity(2);
    if let Some(eyes) = cache.fetch_eyes_white(body_path, &host_joint_names, host_joint_count) {
        pieces.push(CosmeticPiece {
            tag: TAG_EYES,
            mesh: eyes,
            texture: None,
        });
    }
    if let Some(hair) =
        cache.fetch_hair_tinted(hair_p, hair_color, &host_joint_names, host_joint_count)
    {
        pieces.push(CosmeticPiece {
            tag: TAG_HAIR,
            mesh: hair,
            texture: None,
        });
    }
    if let Some(eyebrows) = cache.fetch_hair_tinted(
        eyebrow_p,
        eyebrow_color,
        &host_joint_names,
        host_joint_count,
    ) {
        pieces.push(CosmeticPiece {
            tag: TAG_EYEBROWS,
            mesh: eyebrows,
            texture: None,
        });
    }

    // Ensure the entity has a SkinnedAttachments component.
    if world.get::<&SkinnedAttachments>(entity).is_err() {
        let _ = world.insert_one(entity, SkinnedAttachments::default());
    }

    // Phase 1: short-circuit pieces that already exist with the
    // same Arc<SkinnedMesh>; collect the rest for registration.
    let mut to_add: Vec<CosmeticPiece> = Vec::new();
    {
        let Ok(mut atts) = world.get::<&mut SkinnedAttachments>(entity) else {
            return;
        };
        for piece in pieces {
            let want_ptr = Arc::as_ptr(&piece.mesh);
            let mut found = false;
            for existing in atts.pieces.iter_mut() {
                if existing.tag == piece.tag {
                    if Arc::as_ptr(&existing.mesh) == want_ptr {
                        existing.visible = true;
                        found = true;
                    } else {
                        existing.visible = false;
                    }
                }
            }
            if !found {
                to_add.push(piece);
            }
        }
    }

    // Phase 2: register the new dynamic meshes and push.
    for piece in to_add {
        let object_index = match renderer.add_skinned_mesh(
            &piece.mesh.bind_vertices,
            &piece.mesh.vertex_skin,
            &piece.mesh.indices,
            host_xform,
            0.0,
        ) {
            Ok(idx) => idx,
            Err(e) => {
                log::warn!("avatar cosmetic: add_skinned_mesh failed: {}", e);
                continue;
            }
        };
        if let Some(tex) = piece.texture {
            if let Err(e) = renderer.set_object_texture(
                object_index,
                rift_engine::TextureSource::File(std::path::Path::new(tex)),
            ) {
                log::warn!("avatar cosmetic texture {:?} failed: {}", tex, e);
            }
        }
        if let Ok(mut atts) = world.get::<&mut SkinnedAttachments>(entity) {
            atts.pieces.push(AttachmentPiece {
                mesh: piece.mesh,
                object_index,
                scratch: Vec::new(),
                visible: true,
                tag: piece.tag,
                // Cosmetic geometry (eyeballs, hair cards)
                // must NOT be inflated along normals: doing
                // so would puff the eyes outward and split
                // hair cards along their opposing normals,
                // which is exactly what made the female
                // model's eyes pop out and her hair look
                // half-missing.
                inflate: false,
            });
        }
    }
}
