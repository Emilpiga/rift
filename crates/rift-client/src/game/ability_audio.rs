//! Client-side audio adaptation for ability cues.
//!
//! The per-ability sound recipe (cast / travel / impact paths)
//! now lives next to the rest of the ability data in
//! [`rift_game::abilities::Ability::audio`], so this module is
//! a thin client-side adapter: it looks the recipe up by wire
//! id and wraps the `&'static str` asset paths in
//! [`SoundSpec`] values with the right falloff / volume /
//! looping flags for cast / travel / impact roles.
//!
//! Returning a silent default means an ability with no `audio`
//! authored simply ships muted — every call site falls through
//! cleanly.

use rift_audio::SoundSpec;
pub use rift_game::abilities::AbilityAudio;

/// Look up the audio recipe for `wire_id`. Falls back to a
/// silent default for abilities the registry doesn't know
/// about (synthetic transforms, defensive guards).
pub fn audio_for(wire_id: rift_game::abilities::AbilityWireId) -> AbilityAudio {
    rift_game::abilities::lookup(wire_id)
        .map(|a| a.audio)
        .unwrap_or(AbilityAudio::SILENT)
}

/// Volume + falloff used for a one-shot cast cue. Loud and
/// wide so the player hears their own cast clearly through
/// the third-person camera, with enough min_distance to
/// cover the full camera-sit-back range.
pub fn cast_spec(path: &str) -> SoundSpec {
    SoundSpec {
        path: path.into(),
        volume: 1.0,
        min_distance: 6.0,
        max_distance: 25.0,
        looping: false,
        pitch: 1.0,
    }
}

/// Volume + falloff for the per-projectile travel loop. The
/// emitter is re-anchored to the projectile every frame so
/// spatialisation tracks the flight path; falloff is tight
/// enough that distant projectiles don't crowd the mix.
pub fn travel_spec(path: &str) -> SoundSpec {
    SoundSpec {
        path: path.into(),
        volume: 0.8,
        min_distance: 3.0,
        max_distance: 18.0,
        looping: true,
        pitch: 1.0,
    }
}

/// Volume + falloff for the impact / detonation one-shot.
/// Slightly louder and wider than travel so a fireball going
/// off across a room still reads as an event.
pub fn impact_spec(path: &str) -> SoundSpec {
    SoundSpec {
        path: path.into(),
        volume: 1.0,
        min_distance: 5.0,
        max_distance: 30.0,
        looping: false,
        pitch: 1.0,
    }
}

/// Apply subtle per-play variation to a one-shot spec so
/// repeated casts / impacts don't sound like the same file
/// stamped over itself. We jitter playback rate (pitch) and
/// linear volume independently with small ranges — large
/// enough to be perceptible, small enough that the cue still
/// reads as "the same sound, different take" rather than a
/// chipmunked or muffled version.
///
/// Looping specs are intentionally left untouched by callers
/// (the travel loop would chirp if we de-tuned it per spawn
/// while keeping per-frame anchor updates).
pub fn jitter_one_shot(spec: &mut SoundSpec) {
    // Cheap, non-deterministic seed: nanosecond-resolution
    // monotonic clock xored with a thread-local counter is
    // overkill for audio variation, so we just take low bits
    // of `Instant::now()` and run it through a single
    // xorshift step to decorrelate the two outputs.
    use std::time::Instant;
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    let mut s = epoch.elapsed().as_nanos() as u64 ^ 0x9E37_79B9_7F4A_7C15;
    // xorshift64
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    // Two independent [-1.0, 1.0] samples from the 64-bit
    // state by splitting into high/low halves.
    let to_unit = |x: u32| (x as f32 / u32::MAX as f32) * 2.0 - 1.0;
    let r_pitch = to_unit((s >> 32) as u32);
    let r_vol = to_unit(s as u32);
    // ~\u00b1 a semitone (2^(1/12) \u2248 1.0595) and \u00b110% volume.
    spec.pitch *= 1.0 + r_pitch * 0.06;
    spec.volume *= 1.0 + r_vol * 0.10;
}
