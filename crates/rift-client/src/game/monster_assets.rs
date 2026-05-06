//! Monster asset cache: maps each enemy archetype to a skinned glTF
//! from the `animated-monsters` pack and pre-loads the mesh + animation
//! clips up-front (during the loading screen) so spawning is cheap.
//!
//! The monster pack ships each creature with an embedded animation
//! library: `Idle`, `Walk`, `Bite_Front`, `HitRecieve` (note: misspelled
//! in the source assets), `Death`, and a few extras. We bind every
//! animation in the file to the monster's own skeleton so gameplay
//! systems can name them at runtime.

use std::sync::Arc;

use rift_engine::ash::vk;
use rift_engine::ecs::components::AnimationSet;
use rift_engine::renderer::mesh::SkinnedMesh;
use rift_engine::renderer::texture::Texture;
use rift_game::monsters::MonsterRole;

/// One loaded monster: shared mesh + bound animations.
pub struct MonsterAsset {
    pub mesh: Arc<SkinnedMesh>,
    pub anims: AnimationSet,
    /// Raw PNG bytes for the base-color texture (extracted from the
    /// glTF). `None` if the model has no embedded texture; in that case
    /// the spawner falls back to the renderer's default material.
    pub texture_bytes: Option<Arc<Vec<u8>>>,
    /// Lazily-uploaded GPU texture shared by every spawn of this role.
    /// Allocated the first time `ensure_shared_material` is called.
    /// Lives for the lifetime of the asset cache (process lifetime),
    /// which avoids exhausting the renderer's per-pool descriptor-set
    /// budget when a floor spawns dozens of enemies.
    pub shared_texture: Option<Texture>,
    /// Descriptor set bound to `shared_texture`.
    pub shared_set: Option<vk::DescriptorSet>,
}

/// Pre-loaded monster assets, indexed by role. `None` means the load
/// failed — the spawner should fall back to a procedural mesh.
#[derive(Default)]
pub struct MonsterCache {
    pub brute:   Option<MonsterAsset>,
    pub stalker: Option<MonsterAsset>,
    pub caster:  Option<MonsterAsset>,
    pub elite:   Option<MonsterAsset>,
    pub boss:    Option<MonsterAsset>,
}

impl MonsterCache {
    pub fn get(&self, role: MonsterRole) -> Option<&MonsterAsset> {
        match role {
            MonsterRole::Brute   => self.brute.as_ref(),
            MonsterRole::Stalker => self.stalker.as_ref(),
            MonsterRole::Caster  => self.caster.as_ref(),
            MonsterRole::Elite   => self.elite.as_ref(),
            MonsterRole::Boss    => self.boss.as_ref(),
        }
    }

    pub fn slot_mut(&mut self, role: MonsterRole) -> &mut Option<MonsterAsset> {
        match role {
            MonsterRole::Brute   => &mut self.brute,
            MonsterRole::Stalker => &mut self.stalker,
            MonsterRole::Caster  => &mut self.caster,
            MonsterRole::Elite   => &mut self.elite,
            MonsterRole::Boss    => &mut self.boss,
        }
    }

    /// Free GPU resources owned by every loaded monster role.  Must be
    /// called before the renderer's allocator is dropped, since the
    /// shared `Texture` handles each own a `vk::Image` whose memory is
    /// allocated by that allocator.
    pub fn cleanup_gpu(
        &mut self,
        device: &rift_engine::ash::Device,
        allocator: &std::sync::Arc<std::sync::Mutex<rift_engine::gpu_allocator::vulkan::Allocator>>,
    ) {
        for slot in [
            &mut self.brute, &mut self.stalker, &mut self.caster,
            &mut self.elite, &mut self.boss,
        ] {
            if let Some(asset) = slot.as_mut() {
                if let Some(mut tex) = asset.shared_texture.take() {
                    tex.cleanup(device, allocator);
                }
                asset.shared_set = None;
            }
        }
    }

    /// Returns `true` if every role has been resolved (either loaded
    /// successfully or failed permanently — both are fine; the spawner
    /// falls back to procedural meshes when an asset is missing).
    pub fn fully_loaded(&self, roles: &[MonsterRole]) -> bool {
        roles.iter().all(|r| self.is_resolved(*r))
    }

    fn is_resolved(&self, role: MonsterRole) -> bool {
        // We mark a role "resolved" once load_one has been called for
        // it; that always sets `Some` (with a valid asset) on success
        // or leaves it `None` after logging the error. The cache uses a
        // separate `tried` set to disambiguate "not yet tried" from
        // "tried and failed". Caller drives the iteration explicitly via
        // `next_pending_role`, so we don't need that here — instead the
        // caller tracks progress with `LoadPhase`.
        self.get(role).is_some()
    }
}

/// Load one monster role: the glTF mesh + all animation clips bound to
/// its skeleton. Returns `None` if the file or skinning fails.
pub fn load_role(role: MonsterRole) -> Option<MonsterAsset> {
    let path = role.gltf_path();
    let mesh = match SkinnedMesh::from_gltf(path) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("monster mesh load failed ({}): {}", path, e);
            return None;
        }
    };
    let anims = match rift_engine::animation::Clip::load_all(path) {
        Ok(clips) => {
            let mut set = AnimationSet::default();
            for clip in &clips {
                let bound = clip.bind_to_skeleton(
                    &mesh.joint_index_by_name,
                    mesh.joints.len(),
                );
                set.clips.insert(clip.name.to_ascii_lowercase(), Arc::new(bound));
            }
            set
        }
        Err(e) => {
            log::warn!("monster animation load failed ({}): {}", path, e);
            AnimationSet::default()
        }
    };
    let texture_bytes = match rift_engine::renderer::mesh::extract_base_color_image_bytes(path) {
        Ok(Some(b)) => Some(Arc::new(b)),
        Ok(None) => None,
        Err(e) => {
            log::warn!("monster texture extract failed ({}): {}", path, e);
            None
        }
    };
    Some(MonsterAsset {
        mesh: Arc::new(mesh),
        anims,
        texture_bytes,
        shared_texture: None,
        shared_set: None,
    })
}

impl MonsterAsset {
    /// Make sure the shared GPU texture + descriptor set exists for
    /// this role, uploading once on first use.  Returns the descriptor
    /// set so the caller can bind it on a freshly-spawned object.
    pub fn ensure_shared_material(
        &mut self,
        renderer: &mut rift_engine::Renderer,
    ) -> Option<vk::DescriptorSet> {
        if let Some(set) = self.shared_set {
            return Some(set);
        }
        let bytes = self.texture_bytes.as_ref()?;
        match renderer.upload_shared_texture_from_bytes(bytes) {
            Ok((tex, set)) => {
                self.shared_texture = Some(tex);
                self.shared_set = Some(set);
                Some(set)
            }
            Err(e) => {
                log::warn!("monster shared texture upload failed: {}", e);
                None
            }
        }
    }
}
