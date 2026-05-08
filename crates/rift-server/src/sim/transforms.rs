//! Legendary `TransformAbility` consumption.
//!
//! Each variant of [`rift_game::loot::AbilityVariant`] is a
//! gameplay-changing override of an existing ability. Where the
//! transform fires depends on the ability's shape — projectile
//! transforms hook before dispatch, channel transforms hook on
//! channel end, hit-on-* transforms hook in the projectile-hit
//! pipeline. This module owns one `match` per hook so adding a
//! new variant is one arm in one file.
//!
//! # Adding a new transform
//!
//! 1. Add the `AbilityVariant::Foo` variant in
//!    `rift_game::loot::affixes`.
//! 2. Add the `AffixDef` row pointing at it.
//! 3. Add an arm to the appropriate hook here. If the transform
//!    fires at a *new* point in the combat pipeline, add a new
//!    hook function (one match arm per variant) and call it
//!    from the matching pipeline site.
//!
//! Hooks today:
//! * [`on_channel_end`] — runs whenever a `ServerChannel`
//!   row is removed (natural expiry, cancel-on-move, explicit
//!   `EndChannel` request, caster death). Used by
//!   `FrostRayShatter`.

use glam::Vec3;
use hecs::Entity;
use rift_game::abilities::{id as ability_id, ChannelEffect};
use rift_game::loot::AbilityVariant;
use rift_net::{messages::WorldEvent, NetId, NetTick};

use super::channel::ServerChannel;
use super::player::ServerPlayer;
use super::projectile::{ServerProjectile, Team};

/// Per-player internal cooldowns for [`AbilityVariant`]
/// transforms. Stored on [`super::player::ServerPlayer`] so a
/// transform-firing release can be rate-limited independently
/// of the underlying ability's own cooldown. Frost Ray (zero
/// cooldown, infinite channel) would otherwise let a player
/// spam-flicker the channel to spawn a `FrostRayShatter`
/// burst every tick — the ICD makes the shatter a paced
/// finisher instead. Ticked from `Sim::step` once per frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransformCds([f32; 3]);

impl TransformCds {
    fn idx(v: AbilityVariant) -> usize {
        match v {
            AbilityVariant::FireballToBeam => 0,
            AbilityVariant::FrostRayShatter => 1,
            AbilityVariant::WhirlwindVortex => 2,
        }
    }
    pub fn get(&self, v: AbilityVariant) -> f32 {
        self.0[Self::idx(v)]
    }
    pub fn set(&mut self, v: AbilityVariant, secs: f32) {
        self.0[Self::idx(v)] = secs;
    }
    /// Tick all cooldowns down by `dt`, clamped to zero.
    pub fn tick(&mut self, dt: f32) {
        for x in &mut self.0 {
            if *x > 0.0 {
                *x = (*x - dt).max(0.0);
            }
        }
    }
}

/// Internal cooldown applied after a transform fires. Zero for
/// transforms that have no abuse vector (the underlying
/// ability's own cooldown is the limiter); positive for
/// transforms attached to spammable abilities like Frost Ray.
fn transform_internal_cooldown(v: AbilityVariant) -> f32 {
    match v {
        // Frost Ray has cooldown 0 and infinite duration; the
        // ICD is the only thing keeping shatter from being
        // a per-tick burst.
        AbilityVariant::FrostRayShatter => 6.0,
        AbilityVariant::FireballToBeam => 0.0,
        AbilityVariant::WhirlwindVortex => 0.0,
    }
}

/// Minimum channel hold (seconds) before a release fires the
/// transform. Pairs with `transform_internal_cooldown` to
/// require both "commit to the channel" and "can't spam".
fn transform_min_channel_time(v: AbilityVariant) -> f32 {
    match v {
        AbilityVariant::FrostRayShatter => 0.4,
        _ => 0.0,
    }
}

/// Snapshot of a channel at end-of-life that transform hooks
/// can read without re-borrowing the world. Built by the
/// channel system before stripping the `ServerChannel` row;
/// passed by reference into [`on_channel_end`].
#[derive(Clone, Debug)]
pub struct ChannelEndSnapshot {
    pub ability_id: u8,
    pub team: Team,
    pub effect: ChannelEffect,
    pub crit_chance: f32,
    pub crit_damage: f32,
    pub apply_debuff: Option<u8>,
    pub aim: Vec3,
    pub caster_pos: Vec3,
    pub caster_net_id: NetId,
    /// ECS entity of the caster, so transform hooks can read /
    /// mutate per-player state (e.g. [`TransformCds`]).
    pub caster_entity: Entity,
    /// Wall-clock seconds the channel was live. Used by the
    /// min-channel-time gate in [`on_channel_end`].
    pub elapsed: f32,
    pub transform: Option<AbilityVariant>,
}

impl ChannelEndSnapshot {
    /// Convenience: build a snapshot from a live `ServerChannel`
    /// + the caster fields that aren't on the row.
    pub fn from_channel(
        c: &ServerChannel,
        caster_entity: Entity,
        caster_pos: Vec3,
        caster_net_id: NetId,
    ) -> Self {
        Self {
            ability_id: c.ability_id,
            team: c.team,
            effect: c.effect,
            crit_chance: c.crit_chance,
            crit_damage: c.crit_damage,
            apply_debuff: c.apply_debuff,
            aim: c.aim,
            caster_pos,
            caster_net_id,
            caster_entity,
            elapsed: c.elapsed,
            transform: c.transform,
        }
    }
}

/// Channel-end transform dispatch. Called from
/// [`super::channel::tick`] (natural expiry / cancel-on-move)
/// and [`super::channel::cancel`] (explicit `EndChannel`
/// request, key-release) so a transform fires on every shape
/// of channel end. Returns silently when the channel had no
/// transform attached.
///
/// Takes the minimum surface (`events` sink and the
/// projectile net-id allocator) instead of a full
/// `CombatCtx` so `cancel()` can dispatch transforms without
/// constructing a step-scoped context.
pub fn on_channel_end(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    next_projectile_net_id: &mut u32,
    snap: &ChannelEndSnapshot,
) {
    // Only player-team transforms make gameplay sense today.
    // Enemy channels don't carry equipment-derived transforms
    // (no enemy AbilityMods source), but gating here is the
    // tidy place to enforce that invariant.
    if snap.team != Team::Player {
        return;
    }
    let Some(variant) = snap.transform else { return };

    // Gate 1 — minimum hold time. Releasing an ability
    // immediately after starting it shouldn't trigger a
    // big finisher; the player has to actually commit to
    // the channel.
    if snap.elapsed < transform_min_channel_time(variant) {
        return;
    }

    // Gate 2 — internal cooldown. Read + set on the caster's
    // `ServerPlayer` so spamming release can't bypass the
    // base ability's own cooldown (Frost Ray has none).
    let icd = transform_internal_cooldown(variant);
    if icd > 0.0 {
        let on_cd = world
            .get::<&ServerPlayer>(snap.caster_entity)
            .ok()
            .map(|p| p.transform_cds.get(variant) > 0.0)
            .unwrap_or(true);
        if on_cd {
            return;
        }
        if let Ok(mut p) = world.get::<&mut ServerPlayer>(snap.caster_entity) {
            p.transform_cds.set(variant, icd);
        }
    }

    match variant {
        AbilityVariant::FrostRayShatter => {
            fire_frost_shatter(world, events, next_projectile_net_id, snap)
        }
        AbilityVariant::FireballToBeam => {
            // Stub: Fireball-to-beam should intercept *cast*
            // dispatch, not channel end. Left here as a marker
            // so a reader scanning the file sees the variant
            // is known but not yet wired.
        }
        AbilityVariant::WhirlwindVortex => {
            // Stub: vortex-on-end behavior TBD.
        }
    }
}

// ── FrostRayShatter ────────────────────────────────────────────

/// Number of shards a `FrostRayShatter` proc emits. Tuned so
/// the burst reads clearly without overlapping pierce hits on
/// a single dense pack.
const FROST_SHATTER_SHARDS: u32 = 8;
/// Per-shard horizontal speed (m/s).
const FROST_SHATTER_SPEED: f32 = 14.0;
/// Per-shard time-to-live (s). Combined with `SPEED` this
/// gives each shard ~6 m of reach from the terminus — enough
/// to threaten a back rank that the beam just barely missed.
const FROST_SHATTER_TTL: f32 = 0.45;
/// Per-shard damage = `damage_per_tick * MULT`. Tuned so the
/// burst is a meaningful "send-off" without making FrostRay's
/// shatter the dominant DPS source on its own.
const FROST_SHATTER_DAMAGE_MULT: f32 = 2.0;

fn fire_frost_shatter(
    world: &mut hecs::World,
    events: &mut Vec<WorldEvent>,
    next_projectile_net_id: &mut u32,
    snap: &ChannelEndSnapshot,
) {
    use std::f32::consts::TAU;
    let ChannelEffect::Beam { range, damage_per_tick, .. } = snap.effect else {
        // FrostRayShatter only meaningful on Beam channels.
        return;
    };
    let aim = if snap.aim.length_squared() > 1.0e-4 {
        snap.aim.normalize()
    } else {
        Vec3::Z
    };
    let terminus = snap.caster_pos + aim * range + Vec3::Y * 1.0;
    let damage = damage_per_tick * FROST_SHATTER_DAMAGE_MULT;
    for i in 0..FROST_SHATTER_SHARDS {
        let theta = (i as f32 / FROST_SHATTER_SHARDS as f32) * TAU;
        let radial = Vec3::new(theta.cos(), 0.0, theta.sin());
        // 70 % radial + 30 % forward keeps the burst readable
        // as "shatter" rather than "ring".
        let dir = (radial * 0.7 + aim * 0.3).normalize_or_zero();
        if dir.length_squared() < 1.0e-4 {
            continue;
        }
        let net_id = NetId(*next_projectile_net_id);
        *next_projectile_net_id = next_projectile_net_id
            .wrapping_add(1)
            .max(0x4000_0000);
        // Use `FROST_SHATTER_SHARD` as the wire ability id so
        // the client projectile-spawn pipeline picks up the
        // dedicated `ShapeVisuals::Projectile` recipe (Frost
        // Ray itself is a Beam). Meter attribution will bucket
        // these as "Frost Shard" rather than under the parent
        // "Frost Ray" — UX is actually clearer that way.
        world.spawn((ServerProjectile {
            net_id,
            ability_id: ability_id::FROST_SHATTER_SHARD,
            owner: snap.caster_net_id,
            team: Team::Player,
            attacker_kind: super::meters::ATTACKER_KIND_OTHER,
            position: terminus,
            velocity: dir * FROST_SHATTER_SPEED,
            ttl: FROST_SHATTER_TTL,
            damage,
            crit_chance: snap.crit_chance,
            crit_damage: snap.crit_damage,
            pierce_remaining: 1,
            size: 0.5,
            apply_debuff: snap.apply_debuff,
        },));
    }
    // One-shot burst event at the terminus so clients have a
    // hook for the visual flourish (until a dedicated
    // `WorldEvent::Vfx` variant lands, this re-uses the
    // `ChannelTick` shape — the client renderer treats a
    // post-`ChannelEnd` tick at the terminus as the shatter
    // cue).
    events.push(WorldEvent::ChannelTick {
        caster: snap.caster_net_id,
        ability: snap.ability_id as u16,
        position: terminus.to_array(),
        dir: [aim.x, aim.z],
        tick: NetTick(0),
    });
}
