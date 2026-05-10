//! Spatial audio system for Rift, mirroring the architecture of
//! [`rift_engine::renderer::vfx::runtime::VfxSystem`] and the
//! dynamic point-light slot table.
//!
//! ## Design
//!
//! The runtime owns:
//!
//! * One `kira::AudioManager` (default cpal backend, symphonia
//!   decoders).
//! * One [`ListenerHandle`] driven from the player camera every
//!   frame.
//! * A [`SoundLibrary`] that lazy-loads `StaticSoundData` from
//!   disk on first reference and caches it for cheap clones —
//!   `kira` shares the underlying decoded buffer between clones.
//! * A generational [`Emitter`] table for sounds that need to
//!   live longer than a one-shot (loops, projectile trails,
//!   torches). Each emitter owns a per-emitter
//!   [`SpatialTrackHandle`] so its world position can be moved
//!   independently of every other sound.
//!
//! ## Two entry-points
//!
//! 1. [`AudioSystem::play_one_shot`] — fire-and-forget. Spawns
//!    a throwaway spatial track at `position`, plays the sound
//!    once, lets `kira` reap the track when the sound ends.
//!    Use for impacts, hits, footsteps, ability casts.
//!
//! 2. [`AudioSystem::spawn_emitter`] — returns an
//!    [`EmitterId`] you can reposition, retune, or despawn.
//!    Use for projectile trails, torch crackle loops, portal
//!    hum, anything that needs to follow an entity or have its
//!    intensity driven by another system (lights, VFX
//!    brightness curves).
//!
//! ## Listener
//!
//! [`AudioSystem::set_listener`] takes the player camera's
//! world position and orientation. Call it every frame *before*
//! ticking so distance attenuation + stereo panning use the
//! latest pose.
//!
//! ## Crate boundary
//!
//! This crate only depends on `kira` + `glam` + `log`. It has
//! zero awareness of `hecs`, `vulkan`, or any rift-specific
//! types — it's a thin, gameplay-agnostic mixer wrapper. The
//! ECS bridge (the `AudioEmitter` component, listener
//! propagation from camera) lives in `rift-client` so the
//! audio crate can be reused from tools and tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use glam::{Quat, Vec3};
use kira::{
    listener::ListenerHandle,
    sound::{
        static_sound::{StaticSoundData, StaticSoundHandle},
        PlaybackState,
    },
    track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle},
    AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Tween,
};

/// Stable handle for one live [`Emitter`]. Wraps a generational
/// index so freed slots can be reused without aliasing old ids
/// — mirrors `rift_engine::renderer::vfx::runtime::EffectId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EmitterId {
    index: u32,
    generation: u32,
}

/// Authoring-time description of a sound. Built once and reused
/// for every play. Kept deliberately simple — kira already
/// exposes a much wider parameter surface; we only thread the
/// knobs gameplay code actually drives.
#[derive(Clone, Debug)]
pub struct SoundSpec {
    /// Path under `assets/audio/`, e.g. `"vfx/fireball_loop.ogg"`.
    /// Loaded once via [`SoundLibrary`] and cached.
    pub path: PathBuf,
    /// Linear volume multiplier (1.0 = source level). Applied
    /// per-play; emitters can override it later via
    /// [`AudioSystem::set_emitter_volume`].
    pub volume: f32,
    /// Distance at which the sound is at full volume. Sounds
    /// closer than this are not boosted further; sounds
    /// between `min_distance` and `max_distance` attenuate
    /// according to kira's default rolloff curve. See
    /// [`SpatialTrackDistances`].
    pub min_distance: f32,
    /// Distance beyond which the sound is fully attenuated
    /// (silent). Tune per-effect — projectile whooshes are
    /// short-range (~30 m), boss death rumbles are long-range
    /// (~60 m+).
    pub max_distance: f32,
    /// `true` for sounds that should restart from the top
    /// when they reach the end (torch crackle, portal hum,
    /// projectile trail). One-shots leave this `false`.
    pub looping: bool,
}

impl SoundSpec {
    /// Convenience builder for a one-shot at typical
    /// short-range falloff (1 m full, 25 m silent).
    pub fn one_shot(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            volume: 1.0,
            min_distance: 1.0,
            max_distance: 25.0,
            looping: false,
        }
    }

    /// Convenience builder for an entity-attached loop at
    /// typical short-range falloff. Caller usually wants to
    /// override `max_distance` to taste.
    pub fn looping(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            volume: 1.0,
            min_distance: 1.0,
            max_distance: 20.0,
            looping: true,
        }
    }
}

/// Path-keyed cache of decoded `StaticSoundData`. `kira`'s
/// `StaticSoundData` is reference-counted internally — cloning
/// an entry is free (Arc bump). We hand out clones rather than
/// references so callers don't have to juggle the cache's
/// lifetime.
#[derive(Default)]
struct SoundLibrary {
    /// Root for all paths. Resolved against the working dir
    /// (typically the repo root) at startup.
    root: PathBuf,
    cache: HashMap<PathBuf, StaticSoundData>,
    /// Paths that failed to load once, cached so we don't
    /// re-hit the filesystem every frame for missing assets.
    misses: HashMap<PathBuf, ()>,
}

impl SoundLibrary {
    fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache: HashMap::new(),
            misses: HashMap::new(),
        }
    }

    /// Return a clone of the cached `StaticSoundData` for
    /// `path`, loading it from disk on miss. `None` for
    /// missing / unreadable files; we log once per path to
    /// avoid spamming the console.
    fn get(&mut self, path: &Path) -> Option<StaticSoundData> {
        if let Some(hit) = self.cache.get(path) {
            return Some(hit.clone());
        }
        if self.misses.contains_key(path) {
            return None;
        }
        let abs = self.root.join(path);
        match StaticSoundData::from_file(&abs) {
            Ok(data) => {
                self.cache.insert(path.to_path_buf(), data.clone());
                Some(data)
            }
            Err(e) => {
                log::warn!(
                    "audio: failed to load {} ({e}); subsequent plays will be silent",
                    abs.display()
                );
                self.misses.insert(path.to_path_buf(), ());
                None
            }
        }
    }
}

/// One live emitter: spatial track + the sound currently
/// playing on it. Stored in [`AudioSystem`]'s slot table; the
/// public handle is [`EmitterId`].
struct Emitter {
    track: SpatialTrackHandle,
    sound: Option<StaticSoundHandle>,
    generation: u32,
    /// `true` for emitters whose sound is set to loop. Used by
    /// [`AudioSystem::tick`] to know which slots to recycle
    /// when their sound naturally finishes (one-shots) vs
    /// which to keep alive forever (loops, despawned only on
    /// explicit [`AudioSystem::despawn_emitter`]).
    looping: bool,
}

/// Slot in the emitter table. `Free` slots track a generation
/// so reused indices can't be confused with their previous
/// occupant.
enum Slot {
    Live(Emitter),
    Free { next_generation: u32 },
}

/// Default tween for volume / position changes. ~30 ms is
/// short enough to feel responsive, long enough to dodge
/// zipper artefacts when gameplay code yanks a value every
/// frame.
fn smooth_tween() -> Tween {
    Tween {
        duration: Duration::from_millis(30),
        ..Default::default()
    }
}

/// One linear-volume `f32` mapped to kira's `Decibels`. Kira
/// works in decibels internally; we expose linear so gameplay
/// code stays in the same units it uses for VFX brightness /
/// light intensity.
fn linear_to_db(volume: f32) -> Decibels {
    if volume <= 0.0001 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * volume.log10())
    }
}

/// Top-level audio runtime. Owned by the client app, ticked
/// once per frame.
pub struct AudioSystem {
    manager: AudioManager<DefaultBackend>,
    listener: ListenerHandle,
    library: SoundLibrary,
    emitters: Vec<Slot>,
    /// One-shots: spatial tracks dedicated to a single sound
    /// that should auto-recycle when the sound finishes. We
    /// keep them in a separate table from [`Self::emitters`]
    /// because they have no public handle — the only
    /// per-frame work is checking `sound.state()` and
    /// dropping finished entries.
    one_shots: Vec<OneShot>,
}

struct OneShot {
    /// Holding the track alive keeps the spatial routing in
    /// place until the sound finishes. Dropped together.
    _track: SpatialTrackHandle,
    sound: StaticSoundHandle,
}

impl AudioSystem {
    /// Build the audio manager, register a listener at the
    /// origin, and stake out the asset root. `assets_root`
    /// should point at the workspace `assets/` directory; all
    /// [`SoundSpec::path`] values are resolved as
    /// `assets_root / "audio" / path`.
    pub fn new(assets_root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())?;
        // Listener starts at the origin facing -Z (matches the
        // engine's default camera convention). The client
        // calls `set_listener` every frame before tick, so the
        // initial value only matters for the first sub-frame.
        let listener = manager.add_listener(Vec3::ZERO, Quat::IDENTITY)?;
        let root = assets_root.into().join("audio");
        Ok(Self {
            manager,
            listener,
            library: SoundLibrary::new(root),
            emitters: Vec::new(),
            one_shots: Vec::new(),
        })
    }

    /// Update the listener pose. Call once per frame from the
    /// camera step, *before* [`Self::tick`].
    pub fn set_listener(&mut self, position: Vec3, orientation: Quat) {
        self.listener.set_position(position, smooth_tween());
        self.listener.set_orientation(orientation, smooth_tween());
    }

    /// Reap finished one-shots and dead loop emitters. Cheap;
    /// just walks two short vectors.
    pub fn tick(&mut self) {
        // Reap finished one-shots. `kira` keeps the sound
        // handle alive after `Stopped`, so we explicitly
        // check and drop.
        self.one_shots
            .retain(|o| !matches!(o.sound.state(), PlaybackState::Stopped));

        // Reap non-looping emitters whose sound naturally
        // ended (e.g. caller spawned an emitter for a
        // long-but-finite cast). Looping emitters stay alive
        // until `despawn_emitter`.
        for slot in self.emitters.iter_mut() {
            if let Slot::Live(em) = slot {
                if !em.looping {
                    let done = em
                        .sound
                        .as_ref()
                        .map(|s| matches!(s.state(), PlaybackState::Stopped))
                        .unwrap_or(true);
                    if done {
                        let next = em.generation.wrapping_add(1);
                        *slot = Slot::Free {
                            next_generation: next,
                        };
                    }
                }
            }
        }
    }

    /// Fire a sound at `position` and forget about it. The
    /// spatial track is created on the spot and dropped when
    /// the sound finishes.
    pub fn play_one_shot(&mut self, spec: &SoundSpec, position: Vec3) {
        let Some(data) = self.library.get(&spec.path) else {
            return;
        };
        let track = match self.manager.add_spatial_sub_track(
            &self.listener,
            position,
            SpatialTrackBuilder::new()
                .distances(SpatialTrackDistances {
                    min_distance: spec.min_distance,
                    max_distance: spec.max_distance,
                })
                .volume(linear_to_db(spec.volume)),
        ) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("audio: failed to create one-shot track: {e}");
                return;
            }
        };
        let mut track = track;
        let sound = match track.play(data) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("audio: failed to play one-shot {:?}: {e}", spec.path);
                return;
            }
        };
        self.one_shots.push(OneShot {
            _track: track,
            sound,
        });
    }

    /// Spawn a long-lived emitter. Returned handle stays
    /// valid until [`Self::despawn_emitter`] (or, for
    /// non-looping specs, until the sound finishes naturally).
    pub fn spawn_emitter(&mut self, spec: &SoundSpec, position: Vec3) -> Option<EmitterId> {
        let data = self.library.get(&spec.path)?;
        let mut track = match self.manager.add_spatial_sub_track(
            &self.listener,
            position,
            SpatialTrackBuilder::new()
                .distances(SpatialTrackDistances {
                    min_distance: spec.min_distance,
                    max_distance: spec.max_distance,
                })
                .volume(linear_to_db(spec.volume)),
        ) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("audio: failed to create emitter track: {e}");
                return None;
            }
        };

        // Looping is a sound-level setting in `kira` 0.12: we
        // configure it via `loop_region` on the source data
        // before handing it to the track. `..` = loop the
        // whole sound forever.
        let data_to_play = if spec.looping {
            data.loop_region(..)
        } else {
            data
        };
        let sound = match track.play(data_to_play) {
            Ok(s) => Some(s),
            Err(e) => {
                log::warn!("audio: failed to play emitter {:?}: {e}", spec.path);
                None
            }
        };

        // Generational slot allocation: prefer recycling the
        // first free slot so the emitter table stays compact.
        for (i, slot) in self.emitters.iter_mut().enumerate() {
            if let Slot::Free { next_generation } = *slot {
                *slot = Slot::Live(Emitter {
                    track,
                    sound,
                    generation: next_generation,
                    looping: spec.looping,
                });
                return Some(EmitterId {
                    index: i as u32,
                    generation: next_generation,
                });
            }
        }
        let index = self.emitters.len() as u32;
        self.emitters.push(Slot::Live(Emitter {
            track,
            sound,
            generation: 0,
            looping: spec.looping,
        }));
        Some(EmitterId {
            index,
            generation: 0,
        })
    }

    /// Move `id`'s spatial track to `position`. Tweened over
    /// ~30 ms so per-frame updates from a moving entity (a
    /// projectile) don't zipper-clip the panning.
    pub fn set_emitter_position(&mut self, id: EmitterId, position: Vec3) {
        if let Some(em) = self.live_mut(id) {
            em.track.set_position(position, smooth_tween());
        }
    }

    /// Set the linear volume multiplier on `id`'s track. This
    /// is the equivalent of [`rift_engine`'s] per-light
    /// `intensity` knob — call it every frame from the
    /// flicker / pulse / charge-up driver to drive the sound
    /// in lockstep with its visual sibling.
    pub fn set_emitter_volume(&mut self, id: EmitterId, volume: f32) {
        if let Some(em) = self.live_mut(id) {
            em.track.set_volume(linear_to_db(volume), smooth_tween());
        }
    }

    /// Stop `id` immediately and free its slot. Idempotent.
    pub fn despawn_emitter(&mut self, id: EmitterId) {
        if let Some(slot) = self.emitters.get_mut(id.index as usize) {
            if let Slot::Live(em) = slot {
                if em.generation == id.generation {
                    if let Some(s) = em.sound.as_mut() {
                        s.stop(smooth_tween());
                    }
                    let next = em.generation.wrapping_add(1);
                    *slot = Slot::Free {
                        next_generation: next,
                    };
                }
            }
        }
    }

    /// `true` while `id` still refers to a live emitter.
    pub fn is_alive(&self, id: EmitterId) -> bool {
        matches!(
            self.emitters.get(id.index as usize),
            Some(Slot::Live(em)) if em.generation == id.generation
        )
    }

    fn live_mut(&mut self, id: EmitterId) -> Option<&mut Emitter> {
        match self.emitters.get_mut(id.index as usize)? {
            Slot::Live(em) if em.generation == id.generation => Some(em),
            _ => None,
        }
    }
}

/// Re-export of the underlying math types so downstream crates
/// don't need to depend on `glam` directly through us. Kept
/// minimal — anything more exotic should be imported from
/// `glam` directly.
pub mod math {
    pub use glam::{Quat, Vec3};
}

/// Light wrapper newtype so the `Arc<SoundSpec>` pattern stays
/// readable in callers that share specs across many emitters
/// (e.g. one canonical `FIREBALL_TRAIL` spec used by every
/// fireball projectile).
pub type SharedSpec = Arc<SoundSpec>;
