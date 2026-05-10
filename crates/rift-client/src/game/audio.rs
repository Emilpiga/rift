//! ECS bridge for [`rift_audio`].
//!
//! Two patterns supported:
//!
//! * **Entity-bound emitters.** Attach an [`AudioEmitter`]
//!   component to any entity that already has a `Transform`.
//!   Each frame, [`tick_audio_emitters`] walks the world and
//!   pushes the entity's position into the audio system, so
//!   the sound follows the entity automatically. Drop the
//!   entity → the component is dropped → on the next tick
//!   [`tick_audio_emitters`] notices the orphaned emitter and
//!   despawns it.
//!
//!   Mirror of the dynamic-light pattern in
//!   [`rift_client::game::torches`]: a component holds the
//!   per-emitter handle, and a system reads it every frame.
//!
//! * **Manual handles.** Game systems that own non-ECS
//!   resources (the torch table, the portal system, the
//!   projectile-trail map) can call
//!   [`rift_audio::AudioSystem::spawn_emitter`] directly and
//!   keep the [`rift_audio::EmitterId`] in their own struct.
//!   This is the pattern used by the existing VFX `EffectId`
//!   handles \u2014 see `torches.rs::Torch::vfx`.

use glam::Vec3;
use rift_audio::{AudioSystem, EmitterId};
use rift_engine::ecs::components::Transform;

/// Component: this entity has a sound attached. The sound
/// follows the entity's [`Transform`] every frame; the audio
/// system is updated by [`tick_audio_emitters`].
///
/// `intensity` mirrors the dynamic-light intensity pattern
/// \u2014 game code can write to it each frame (e.g. driven by a
/// VFX brightness curve, a charge-up timer, a flame flicker)
/// and the next tick will push it into the emitter's volume.
/// Set to `1.0` for "play at the spec's authored level".
#[derive(Clone, Copy, Debug)]
pub struct AudioEmitter {
    pub id: EmitterId,
    pub intensity: f32,
}

impl AudioEmitter {
    pub fn new(id: EmitterId) -> Self {
        Self { id, intensity: 1.0 }
    }
}

/// Per-frame system: push every [`AudioEmitter`]'s entity
/// position into the audio system, plus its current
/// `intensity` as the emitter's volume. Cheap; one query and
/// one set per emitter.
pub fn tick_audio_emitters(world: &hecs::World, audio: &mut AudioSystem) {
    for (_e, (tr, em)) in world.query::<(&Transform, &AudioEmitter)>().iter() {
        if !audio.is_alive(em.id) {
            // Dead emitters can happen if a one-shot finished
            // naturally before the entity was despawned. Skip
            // silently; the next ECS despawn cleans up the
            // component.
            continue;
        }
        audio.set_emitter_position(em.id, tr.position);
        audio.set_emitter_volume(em.id, em.intensity);
    }
}

/// Convenience: spawn an emitter and attach it to `entity`.
/// Returns the emitter id (also stored on the new component)
/// or `None` if the audio system rejected the spawn (missing
/// asset, capacity, etc.).
pub fn attach_emitter(
    world: &mut hecs::World,
    audio: &mut AudioSystem,
    entity: hecs::Entity,
    spec: &rift_audio::SoundSpec,
    initial_position: Vec3,
) -> Option<EmitterId> {
    let id = audio.spawn_emitter(spec, initial_position)?;
    if world.insert_one(entity, AudioEmitter::new(id)).is_err() {
        // Entity disappeared between spawn and insert \u2014 reap.
        audio.despawn_emitter(id);
        return None;
    }
    Some(id)
}
