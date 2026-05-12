//! Brute / elite chase-and-melee behaviour.
//!
//! Pattern: walk to target, telegraph swing, land damage,
//! repeat. The same body powers `MonsterRole::Brute`,
//! `MonsterRole::Elite`, and the boss-fallback when its
//! `BossState` companion is missing. Variants share the
//! [`Spec`] shape and differ only in the static-spec instance
//! the dispatcher hands them.
//!
//! Three behaviours layered on top of the basic chase:
//! flanking offsets so packs surround instead of dogpiling,
//! A* fallback when LOS to the target is blocked, and a
//! wind-up telegraph so every basic swing is dodgeable.

use glam::Vec3;
use hecs::Entity;
use rift_dungeon::Floor;
use rift_game::kinematic::loco;

use super::{
    elite_mod, enter_windup, tick_windup, AiOutcome, AiPhase, ServerEnemy, WindupKind,
    ATTACK_ANIM_DUR, ELITE_VAMPIRE_FRAC,
};

// ---- Tuning constants ----------------------------------------

/// Brute / elite melee wind-up duration before damage lands.
/// Short enough that swings still feel tight; long enough that
/// a player can react with a dodge-roll on every basic attack
/// instead of just on dashes / bolts. Same telegraph treatment
/// as the stalker dash but tuned shorter — brutes swing twice
/// per windup of a stalker.
pub const WINDUP_DUR: f32 = 0.32;

/// Number of angular flank slots around a target. 8 = cardinals
/// + diagonals, dense enough that a pack of 4-6 brutes spreads
/// to roughly equidistant approach lanes without the visible
/// "snap to nearest slot" jitter you get with 4. Each enemy
/// claims a slot deterministically via `net_id % FLANK_SLOTS`.
pub const FLANK_SLOTS: u8 = 8;
/// Distance from target at which flanking is engaged. Inside
/// this range the brute steers toward an offset *around* the
/// target instead of straight at it; outside, it bee-lines.
/// Tuned roughly equal to one room's width so packs only fan
/// out once they've crossed open ground.
pub const FLANK_ENGAGE_DIST: f32 = 6.5;
/// Lateral offset distance baked into each flank slot — the
/// final approach point is `target + offset(slot)` where
/// offset has this magnitude. Smaller than `Spec::attack_range`
/// so the flanker still ends up in melee on the offset side.
pub const FLANK_RADIUS: f32 = 1.2;

/// How often (s) an enemy may rebuild its A* path. Caps the
/// per-tick cost: with ~30 enemies on a deep floor this caps
/// at ~100 path computes per second worst-case. Recompute is
/// also forced when the target changes tile.
pub const PATH_RECOMPUTE_INTERVAL: f32 = 0.4;

// ---- Spec -----------------------------------------------------

/// Brute / elite chase-and-melee tuning.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Distance below which the brute stops moving and starts
    /// a swing. Doubles as the melee-resolve check at the end
    /// of the wind-up (with a small tolerance for late
    /// dodge-out).
    pub attack_range: f32,
    /// Base damage per landed swing. Scaled at apply time by
    /// `damage_mult * floor.enemy_damage_mult`.
    pub attack_damage: f32,
    /// Cooldown between consecutive swings (s). Counts down on
    /// `ServerEnemy::attack_cooldown`.
    pub attack_cooldown: f32,
    /// Wind-up duration before the swing's damage lands (s).
    /// Telegraph fires once at wind-up start.
    pub windup_dur: f32,
    /// Whether to use [`FLANK_*`] offsets to surround the
    /// target instead of bee-lining. Off for boss-fallback so
    /// the boss doesn't drift sideways into a player.
    pub flank: bool,
    /// Whether to consume a cached A* path when LOS to the
    /// target is blocked. Same opt-out reasoning as `flank` —
    /// the boss arena is open enough that pathfinding is
    /// unnecessary.
    pub use_path: bool,
}

/// Default brute / elite spec — used by every non-boss
/// chase-and-melee role.
pub static SPEC: Spec = Spec {
    attack_range: 1.6,
    attack_damage: 8.0,
    attack_cooldown: 1.4,
    windup_dur: WINDUP_DUR,
    flank: true,
    use_path: true,
};

/// Boss-fallback brute spec — used when a `BOSS` row is
/// missing its `BossState` companion or when the boss is in
/// chase mode between special attacks. Same swing timing as a
/// regular brute but 1.6× damage and no flank / pathfinding.
pub static BOSS_MELEE_SPEC: Spec = Spec {
    attack_range: 1.6,
    attack_damage: 8.0 * 1.6,
    attack_cooldown: 1.4,
    windup_dur: WINDUP_DUR,
    flank: false,
    use_path: false,
};

// ---- Tick -----------------------------------------------------

/// Brute / elite / boss-fallback tick. See module docs for the
/// behaviour summary.
pub fn tick(
    en: &mut ServerEnemy,
    spec: &Spec,
    floor: &Floor,
    target: Option<(Entity, Vec3, f32)>,
    speed_mult: f32,
    damage_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    let Some((target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        // Drop the brute back to Idle if it was mid-windup
        // when its target died. Cancels the swing — feels
        // fairer than a phantom hit on a dead player.
        if matches!(
            en.ai_phase,
            AiPhase::Windup {
                kind: WindupKind::BruteMelee,
                ..
            }
        ) {
            en.ai_phase = AiPhase::Idle;
        }
        return;
    };
    let dist = d2.sqrt();
    let to_target_raw = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    if to_target_raw.length_squared() > 1.0e-4 {
        en.k.yaw = to_target_raw.x.atan2(to_target_raw.z);
        en.k.aim_yaw = en.k.yaw;
    }

    // Mid-windup: tick the central timer. On expiry resolve
    // the swing — only land the hit if still in melee range
    // (with a small tolerance), so a player dodging out
    // during the wind-up causes a clean whiff. That's the
    // whole point of the telegraph.
    if matches!(
        en.ai_phase,
        AiPhase::Windup {
            kind: WindupKind::BruteMelee,
            ..
        }
    ) {
        if let Some(WindupKind::BruteMelee) = tick_windup(en, dt) {
            if dist <= spec.attack_range * 1.15 {
                outcome
                    .melee_damage
                    .push(super::super::combat_ctx::PlayerHit {
                        target: target_entity,
                        attacker_kind: en.role.to_wire_byte(),
                        ability_id: rift_game::abilities::id::MELEE_ATTACK,
                        amount: spec.attack_damage * damage_mult,
                    });
                if (en.elite_mods & elite_mod::VAMPIRIC) != 0 {
                    outcome.vampiric_heals.push((
                        target_entity,
                        spec.attack_damage * damage_mult * ELITE_VAMPIRE_FRAC,
                    ));
                }
            }
        }
        return;
    }

    if dist > spec.attack_range {
        // Pick the steering direction: A* waypoint when LOS is
        // blocked (and the spec opts in), flank offset when
        // close, bee-line when far.
        let los_blocked = super::cached_los_blocked(en, floor, target_pos);
        let approach = if los_blocked && spec.use_path {
            // LOS blocked — consume / rebuild the cached path.
            let target_tile = world_to_tile(target_pos);
            let need_recompute = en.path.is_empty()
                || en.path_target_tile != Some(target_tile)
                || en.path_recompute_in <= 0.0;
            if need_recompute {
                let from = world_to_tile(en.k.position);
                en.path = floor.path(from, target_tile, 1024).unwrap_or_default();
                en.path_target_tile = Some(target_tile);
                en.path_recompute_in = PATH_RECOMPUTE_INTERVAL;
            }
            // Drop already-reached waypoints (within half a
            // tile of the enemy's centre).
            while let Some(&(wx, wz)) = en.path.first() {
                let wp = Vec3::new(wx as f32, 0.0, wz as f32);
                let dx = wp.x - en.k.position.x;
                let dz = wp.z - en.k.position.z;
                if dx * dx + dz * dz < 0.25 {
                    en.path.remove(0);
                } else {
                    break;
                }
            }
            // Steer to the next waypoint if we have one,
            // otherwise fall through to the bee-line — the path
            // either hasn't been computed yet or A* failed.
            if let Some(&(wx, wz)) = en.path.first() {
                Vec3::new(
                    wx as f32 - en.k.position.x,
                    0.0,
                    wz as f32 - en.k.position.z,
                )
                .normalize_or_zero()
            } else {
                to_target_raw.normalize_or_zero()
            }
        } else {
            // Clear LOS (or path disabled) — drop any cached
            // waypoints and bee-line / flank.
            en.path.clear();
            en.path_target_tile = None;
            if spec.flank && dist < FLANK_ENGAGE_DIST {
                let approach_pos = flank_offset_pos(target_pos, en.flank_slot, dist);
                let dir = Vec3::new(
                    approach_pos.x - en.k.position.x,
                    0.0,
                    approach_pos.z - en.k.position.z,
                )
                .normalize_or_zero();
                if dir.length_squared() > 1.0e-4 {
                    dir
                } else {
                    to_target_raw.normalize_or_zero()
                }
            } else {
                to_target_raw.normalize_or_zero()
            }
        };
        en.k.velocity = approach * en.speed * speed_mult;
        en.k.locomotion = loco::RUN;
    } else {
        // In melee range — face target, halt, kick off windup
        // when cooldown is ready.
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        if en.attack_cooldown <= 0.0 {
            en.attack_cooldown = spec.attack_cooldown;
            // attack_anim_remaining gets overwritten by
            // enter_windup; pad it post-windup to keep the
            // attack clip running through the swing follow-
            // through.
            enter_windup(en, WindupKind::BruteMelee, spec.windup_dur, outcome);
            en.attack_anim_remaining = spec.windup_dur + ATTACK_ANIM_DUR;
        }
    }
}

// ---- Helpers --------------------------------------------------

/// Translate a world XZ position to integer tile coordinates,
/// matching the `(x + 0.5).floor()` convention used throughout
/// the codebase (see [`rift_game::kinematic::tile_at`]).
pub(super) fn world_to_tile(p: Vec3) -> (i32, i32) {
    ((p.x + 0.5).floor() as i32, (p.z + 0.5).floor() as i32)
}

/// Compute the flanking-approach point around `target` for the
/// given slot. The offset shrinks linearly with distance to
/// the target so once the brute is on top of the player the
/// offset is zero and the swing lands cleanly. Outside
/// [`FLANK_ENGAGE_DIST`] this fn isn't called.
fn flank_offset_pos(target: Vec3, slot: u8, dist: f32) -> Vec3 {
    let theta = (slot as f32 / FLANK_SLOTS as f32) * std::f32::consts::TAU;
    // Shrink to zero in the last metre so the final swing
    // lines up on the target rather than on the offset.
    let radius_scale = ((dist - 1.0) / (FLANK_ENGAGE_DIST - 1.0)).clamp(0.0, 1.0);
    let r = FLANK_RADIUS * radius_scale;
    Vec3::new(target.x + theta.cos() * r, 0.0, target.z + theta.sin() * r)
}
