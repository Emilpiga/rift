//! Boss role: 3-phase HP-gated multi-ability scheduler.
//!
//! Genuinely irreducible to a tunable `Spec` — the per-phase
//! ability gating + per-ability cooldown table + slam-radius
//! enrage scaling are all bespoke, so the boss stays as its
//! own self-contained module rather than another row in a
//! registry.
//!
//! Per-attack tuning (radius, damage, count, wind-up) lives in
//! [`rift_game::abilities::REGISTRY`] entries for `GROUND_SLAM`,
//! `ARCANE_FAN`, and `SUMMON_BRUTES`; `tick` reads them by
//! `wire_id` at cast-decision time so a designer can rebalance
//! the fight by editing the registry.

use glam::Vec3;
use hecs::Entity;
use rift_game::abilities::AbilityWireId;
use rift_game::kinematic::loco;

use super::brute;
use super::{AiOutcome, ATTACK_ANIM_DUR, EnemyCast, ServerEnemy};

// ---- Phase + enrage tuning -----------------------------------

/// HP fraction at which phase 2 unlocks.
pub const PHASE_2_HP: f32 = 0.66;
/// HP fraction at which phase 3 (enrage) unlocks.
pub const PHASE_3_HP: f32 = 0.33;

/// Cooldown multiplier applied to every boss attack while
/// enraged. Smaller = more frequent.
pub const ENRAGE_CD_MULT: f32 = 0.7;
/// Speed multiplier applied while enraged.
pub const ENRAGE_SPEED_MULT: f32 = 1.3;
/// Phase-3 slam radius is bumped by this multiplier so the
/// player can't just stand at the slam edge once enrage hits.
/// Applied on top of the slam ability's authored `radius` from
/// the registry.
pub const SLAM_RADIUS_ENRAGE_MULT: f32 = 1.5;

/// Wind-up scale as a function of rift floor. Floor 1 leaves
/// wind-ups at 1.0×; deep floors compress them so the player
/// has less time to react. Bottoms out at 0.55×.
pub fn windup_scale(floor: u32) -> f32 {
    (1.0 / (1.0 + floor as f32 * 0.04)).max(0.55)
}

// ---- BossState component -------------------------------------

/// Number of cooldown slots per boss. One per possible ability
/// wire id. Wire ids are u8 so 256 is the upper bound; at this
/// size the array fits in 1 KiB and lookups are O(1).
pub const ABILITY_COOLDOWN_SLOTS: usize = 256;

/// Per-boss state. Lives as a sibling component on the boss
/// entity so regular enemies don't pay for boss bookkeeping.
///
/// Cooldowns are keyed by ability `wire_id` so adding a new
/// boss attack is a single registry-entry append + one entry
/// in the per-phase ability list inside [`tick`]. The active
/// wind-up tracks the resolving ability the same way — when it
/// expires we look up `ability_id` in the registry and dispatch
/// through the kernel pipeline (`super::super::ability::submit`
/// + `super::super::ability::dispatch`).
#[derive(Clone, Debug)]
pub struct BossState {
    /// Floor index captured at spawn — used for wind-up scaling
    /// and for sizing the brute summons (so summons keep up with
    /// the floor's HP curve).
    pub floor: u32,
    /// Per-ability cooldowns. `cooldowns[ability_id as usize]`
    /// is the seconds remaining before the boss may cast that
    /// ability again. Sized to cover the full enemy-id range
    /// (64..=255 — see `rift_game::abilities::id`); the lower
    /// half is unused.
    pub cooldowns: [f32; ABILITY_COOLDOWN_SLOTS],
    pub attack: BossAttack,
}

impl BossState {
    pub fn new(floor: u32) -> Self {
        let mut cooldowns = [0.0_f32; ABILITY_COOLDOWN_SLOTS];
        // Stagger the opening so the boss doesn't dump every
        // attack on the first frame the player walks in.
        cooldowns[rift_game::abilities::id::GROUND_SLAM.raw() as usize] = 1.5;
        cooldowns[rift_game::abilities::id::ARCANE_FAN.raw() as usize] = 3.0;
        cooldowns[rift_game::abilities::id::SUMMON_BRUTES.raw() as usize] = 6.0;
        Self {
            floor,
            cooldowns,
            attack: BossAttack::Idle,
        }
    }
}

/// In-flight boss attack. The `Windup` variant carries the
/// resolving ability's wire id + remaining timer + (for
/// projectiles) the locked aim direction. On expiry the caller
/// looks the ability up in [`rift_game::abilities::REGISTRY`]
/// and dispatches through the kernel pipeline.
#[derive(Clone, Copy, Debug)]
pub enum BossAttack {
    Idle,
    Windup {
        ability_id: AbilityWireId,
        remaining: f32,
        /// Aim direction snapshotted at wind-up start (so the
        /// player can side-step a fan after the telegraph).
        /// `Vec3::ZERO` for self-centred attacks (slam, summon).
        aim: Vec3,
        /// Ability-specific scalar baked at wind-up start —
        /// passed straight through to
        /// `EnemyCast::Resolve.param_a`. Used by the slam to
        /// preserve the enrage-scaled radius across the
        /// wind-up regardless of any phase change mid-cast.
        param_a: f32,
    },
}

// ---- Tick -----------------------------------------------------

/// Boss behaviour. Runs a 3-phase fight gated on HP fraction:
///
/// * **Phase 1** (HP > 66 %): chase + melee + Slam.
/// * **Phase 2** (HP 33-66 %): adds Fan (5-bolt arc).
/// * **Phase 3** / enrage (HP < 33 %): adds Summons; all
///   cooldowns multiplied by [`ENRAGE_CD_MULT`] and speed by
///   [`ENRAGE_SPEED_MULT`].
///
/// Between attacks the boss chases its target and swings in
/// melee using [`brute::BOSS_MELEE_SPEC`] (1.6× damage, no
/// flanking, no pathfinding). Active attacks (Slam / Fan /
/// Summons) take precedence over chase movement and freeze the
/// boss for the duration of the wind-up; that freeze + the
/// attack-anim flag is the visual telegraph the player reads.
///
/// `players` carries every player position so the slam pulse
/// can hit the whole arena radius, not just the locked target.
pub fn tick(
    en: &mut ServerEnemy,
    boss: &mut BossState,
    target: Option<(Entity, Vec3, f32)>,
    _players: &[(Entity, Vec3)],
    speed_mult: f32,
    damage_mult: f32,
    dt: f32,
    outcome: &mut AiOutcome,
) {
    use rift_game::abilities::{id as ab_id, lookup, AbilityKind};

    // Phase + per-phase modifiers.
    let hp_frac = if en.hp_max > 0.0 { en.hp / en.hp_max } else { 0.0 };
    let phase: u8 = if hp_frac > PHASE_2_HP {
        1
    } else if hp_frac > PHASE_3_HP {
        2
    } else {
        3
    };
    let enraged = phase == 3;
    let cd_mult = if enraged { ENRAGE_CD_MULT } else { 1.0 };
    let move_mult = if enraged { ENRAGE_SPEED_MULT } else { 1.0 };
    let windup_scale = windup_scale(boss.floor);

    // Tick every per-ability cooldown. Independent of the
    // shared `attack_cooldown` clock so basic-melee swings
    // stay on their own rhythm during chase.
    for cd in boss.cooldowns.iter_mut() {
        if *cd > 0.0 {
            *cd = (*cd - dt).max(0.0);
        }
    }

    // 1. Resolve any in-flight wind-up before deciding what to
    //    do this tick. Wind-ups freeze movement; on expiry we
    //    emit an `EnemyCast::Resolve` and let the central
    //    dispatcher in the kernel pipeline actually apply the
    //    effect (spawn projectiles / damage / queue summons).
    if let BossAttack::Windup { ability_id, remaining, aim, param_a } = boss.attack {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        // For projectile fans, lock the boss's facing to the
        // captured aim so the cone reads correctly through the
        // wind-up.
        if aim.length_squared() > 1.0e-4 {
            en.k.yaw = aim.x.atan2(aim.z);
            en.k.aim_yaw = en.k.yaw;
        }
        let next = remaining - dt;
        if next <= 0.0 {
            outcome.casts.push(EnemyCast::Resolve {
                owner: en.net_id,
                attacker_kind: en.role.to_wire_byte(),
                ability_id,
                origin: en.k.position,
                aim,
                damage_mult,
                crit_chance: en.crit_chance,
                crit_damage: en.crit_damage,
                param_a,
            });
            // Re-arm the per-ability cooldown from the
            // registry, scaled by phase modifier.
            let base_cd = lookup(ability_id).map(|a| a.cooldown).unwrap_or(0.0);
            boss.cooldowns[ability_id.raw() as usize] = base_cd * cd_mult;
            boss.attack = BossAttack::Idle;
        } else {
            boss.attack = BossAttack::Windup {
                ability_id,
                remaining: next,
                aim,
                param_a,
            };
        }
        return;
    }

    // 2. No active wind-up — chase the target and decide
    //    whether to commit to a new attack this tick.
    let Some((target_entity, target_pos, d2)) = target else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        return;
    };
    let dist = d2.sqrt();
    let to_target = Vec3::new(
        target_pos.x - en.k.position.x,
        0.0,
        target_pos.z - en.k.position.z,
    );
    if to_target.length_squared() > 1.0e-4 {
        en.k.yaw = to_target.x.atan2(to_target.z);
        en.k.aim_yaw = en.k.yaw;
    }

    // Attack selection priority: summons (only enrage) > slam >
    // fan (phase 2+) > melee. Tuning (cooldown, wind-up,
    // radius, projectile count) all comes from the shared
    // [`rift_game::abilities::REGISTRY`]; this body only owns
    // the *selection* logic.
    if enraged && boss.cooldowns[ab_id::SUMMON_BRUTES.raw() as usize] <= 0.0 {
        let summon = lookup(ab_id::SUMMON_BRUTES)
            .expect("REGISTRY missing SUMMON_BRUTES");
        let windup = match summon.kind {
            AbilityKind::Summon { windup, .. } => windup,
            _ => 1.2,
        } * windup_scale;
        boss.attack = BossAttack::Windup {
            ability_id: ab_id::SUMMON_BRUTES,
            remaining: windup,
            aim: Vec3::ZERO,
            param_a: 0.0,
        };
        en.attack_anim_remaining = windup;
        return;
    }
    if boss.cooldowns[ab_id::GROUND_SLAM.raw() as usize] <= 0.0 {
        let slam = lookup(ab_id::GROUND_SLAM).expect("REGISTRY missing GROUND_SLAM");
        let (slam_radius, slam_windup_base) = match slam.kind {
            AbilityKind::DelayedAoe { radius, windup } => (radius, windup),
            _ => (4.0, 1.0),
        };
        // Only commit to slam if the player is within
        // `radius * 1.4` — leaves headroom for the player to
        // dance around the edge of the danger zone.
        if dist <= slam_radius * 1.4 {
            let windup = slam_windup_base * windup_scale;
            // Phase-3 slam radius is bumped so the player can't
            // outrange it by 0.5 m. Visual telegraph carries
            // the same scaled radius so the ring the player
            // sees is the danger circle. The same scaled
            // radius rides through the wind-up via `param_a`
            // so resolve damage uses it too.
            let radius = slam_radius
                * if enraged { SLAM_RADIUS_ENRAGE_MULT } else { 1.0 };
            boss.attack = BossAttack::Windup {
                ability_id: ab_id::GROUND_SLAM,
                remaining: windup,
                aim: Vec3::ZERO,
                param_a: radius,
            };
            en.attack_anim_remaining = windup;
            // Sustained ground-ring telegraph for the wind-up
            // duration. Side-channels through the visual-only
            // GROUND_SLAM_WINDUP wire id; the impact event is
            // emitted by the kernel pipeline on resolve.
            outcome.casts.push(EnemyCast::Start {
                owner: en.net_id,
                ability_id: ab_id::GROUND_SLAM_WINDUP,
                origin: en.k.position,
                target: en.k.position,
                dir_x: radius,
                dir_y: windup,
            });
            return;
        }
    }
    if phase >= 2 && boss.cooldowns[ab_id::ARCANE_FAN.raw() as usize] <= 0.0 {
        let fan = lookup(ab_id::ARCANE_FAN).expect("REGISTRY missing ARCANE_FAN");
        let fan_windup = match fan.kind {
            AbilityKind::EnemyProjectiles { windup, .. } => windup,
            _ => 0.8,
        } * windup_scale;
        let aim = to_target.normalize_or_zero();
        boss.attack = BossAttack::Windup {
            ability_id: ab_id::ARCANE_FAN,
            remaining: fan_windup,
            aim,
            param_a: 0.0,
        };
        en.attack_anim_remaining = fan_windup;
        return;
    }

    // No special attack ready — chase + melee using the
    // boss-fallback brute spec (1.6× damage, no flank, no
    // pathfinding). No wind-up telegraph here: the boss's
    // SLAM / FAN / SUMMON wind-ups already serve as the
    // big-attack reads, and a third telegraph layer on the
    // basic swing felt noisy in playtest.
    let melee = &brute::BOSS_MELEE_SPEC;
    if dist > melee.attack_range {
        let dir = to_target.normalize_or_zero();
        en.k.velocity = dir * en.speed * speed_mult * move_mult;
        en.k.locomotion = loco::RUN;
    } else {
        en.k.velocity = Vec3::ZERO;
        en.k.locomotion = loco::IDLE;
        if en.attack_cooldown <= 0.0 {
            en.attack_cooldown = melee.attack_cooldown;
            en.attack_anim_remaining = ATTACK_ANIM_DUR;
            outcome.melee_damage.push(super::super::combat_ctx::PlayerHit {
                target: target_entity,
                attacker_kind: en.role.to_wire_byte(),
                ability_id: rift_game::abilities::id::MELEE_ATTACK,
                amount: melee.attack_damage * damage_mult,
            });
        }
    }
}
