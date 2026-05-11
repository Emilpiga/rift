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

use rift_game::character::Gender;

/// Tag values for cosmetic attachment pieces. Chosen well above
/// any plausible equipment-slot id so `apply_equipment_visuals`
/// never tries to hide / replace them. `COSMETIC_TAG_BASE` is
/// the lower bound the equipment-visual reconciler uses to
/// identify "leave this piece alone".
pub const COSMETIC_TAG_BASE: u32 = 200;
const TAG_EYES: u32 = 200;
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
fn hair_path(gender: Gender) -> &'static str {
    match gender {
        Gender::Female => {
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)/Hair_Buns.gltf"
        }
        Gender::Male => {
            "assets/models/base-characters/Hairstyles/Rigged to Head Bone/glTF (Godot -Unreal)/Hair_Buzzed.gltf"
        }
    }
}

/// Solid hair tint per gender. The hair gltfs ship with hair-card
/// textures whose alpha channel encodes strand cutout — without
/// alpha-cutout in the shader, sampling that texture stains
/// every hair vertex bright white. Tinting the vertex color and
/// not binding the texture at all gives clean solid-color hair
/// that matches what these solid-mesh styles want anyway.
fn hair_tint(gender: Gender) -> Vec3 {
    match gender {
        // Warm chestnut for female buns.
        Gender::Female => Vec3::new(0.32, 0.18, 0.10),
        // Cool dark brown for male buzz cut.
        Gender::Male => Vec3::new(0.18, 0.12, 0.09),
    }
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
    let hair_p = hair_path(gender);
    let hair_color = hair_tint(gender);

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
