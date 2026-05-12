//! Server-side ground-loot entities.
//!
//! When an enemy dies, [`super::projectile::apply_hits_to_enemies`]
//! consults [`rift_game::loot::drops::table_for`] to roll one or
//! more [`Item`]s, spawns a [`ServerLoot`] component for each at
//! the corpse position, and pushes a [`WorldEvent::LootDropped`]
//! event so clients can light up the loot beam without waiting for
//! the next snapshot.
//!
//! Loot is replicated as a normal snapshot row
//! ([`EntityKind::Loot`]) so a freshly-joined client also sees
//! drops that are already on the floor. A pickup pass (Phase 6)
//! consumes the entity and dispatches it to the picker's inventory.

use glam::Vec3;
use hecs::Entity;
use rift_game::loot::{drops, CharacterIdBytes, Item, LootProvenance, LootRng};
use rift_game::monsters::MonsterRole;
use rift_net::{
    messages::{ItemBlob, WorldEvent},
    NetId, NetTick,
};

use super::combat_ctx::{CombatCtx, KillInfo};
use super::enemies::ServerEnemy;
use super::player::ServerPlayer;

/// Finalise a batch of kills queued by a damage subsystem:
/// 1. Read each dead enemy's role + position out of the ECS.
/// 2. Push a [`WorldEvent::Death`].
/// 3. Roll the [`drops::table_for`] table, spawn [`ServerLoot`]
///    entities, push [`WorldEvent::LootDropped`] per drop.
/// 4. Mark the corpse with `dying_remaining = DEATH_FADE_DUR` so
///    the snapshot keeps shipping it for the death-anim window;
///    [`super::enemies::tick_dying`] does the actual despawn once
///    the timer runs out.
pub fn finalise_kills(
    world: &mut hecs::World,
    ctx: &mut CombatCtx<'_>,
    dead: Vec<(Entity, NetId, Vec3)>,
) {
    for (entity, net_id, hit_dir) in dead {
        // Snapshot the corpse before flipping it into dying mode \u2014
        // the loot drop needs role + position. Pull `elite_mods`
        // too so the death-effect pass can read EXPLODER without
        // re-borrowing the row.
        let info = world
            .get::<&ServerEnemy>(entity)
            .ok()
            .map(|en| (en.role, en.k.position, en.elite_mods));
        ctx.events.push(WorldEvent::Death {
            entity: net_id,
            killer: None,
            hit_dir: hit_dir.to_array(),
        });
        if let Some((role, pos, elite_mods)) = info {
            ctx.kills.push(KillInfo { role });
            drop_for_enemy(
                world,
                ctx.next_loot_net_id,
                ctx.events,
                ctx.tick,
                net_id,
                role,
                pos,
                ctx.floor_index,
                ctx.share_window_ticks,
            );
            // Elite EXPLODER mod: spawn an enemy-team AoE zone
            // at the corpse so anyone standing on top of a fresh
            // kill takes a delayed pop. Tick interval matches
            // duration so it fires exactly once — reads as a
            // single "pop" rather than a sustained pool. Routed
            // through the same zone pool the AbilityKind path
            // uses so the existing tick / replication code
            // handles it without special casing.
            if (elite_mods & super::enemies::elite_mod::EXPLODER) != 0 {
                let zone_net_id = rift_net::NetId(*ctx.next_projectile_net_id);
                *ctx.next_projectile_net_id = ctx.next_projectile_net_id.wrapping_add(1).max(1);
                ctx.death_aoe_zones.push(super::projectile::ServerAoeZone {
                    owner: zone_net_id,
                    ability_id: super::meters::ABILITY_ID_OTHER,
                    attacker_kind: role.to_wire_byte(),
                    team: super::projectile::Team::Enemy,
                    position: pos,
                    radius: super::enemies::ELITE_EXPLODER_RADIUS,
                    damage_per_tick: super::enemies::ELITE_EXPLODER_DAMAGE,
                    crit_chance: 0.0,
                    crit_damage: 0.0,
                    tick_interval: 0.55,
                    duration: 0.55,
                    elapsed: 0.0,
                    tick_timer: 0.55,
                    apply_debuff: None,
                });
            }
        }
        if let Ok(mut en) = world.get::<&mut ServerEnemy>(entity) {
            en.dying_remaining = super::enemies::DEATH_FADE_DUR;
            en.k.velocity = glam::Vec3::ZERO;
            en.attack_anim_remaining = 0.0;
        }
    }
}

/// One unclaimed item resting on the floor.
#[derive(Clone, Debug)]
pub struct ServerLoot {
    pub net_id: NetId,
    pub position: Vec3,
    pub item: Item,
    /// Time-bounded share gate. `Some` means the underlying
    /// [`Item::provenance`] is still being enforced for
    /// pickup; once `current_tick >= expires_at_tick` the
    /// gate is lifted and any Sim-peer can claim the drop.
    /// `None` is reserved for un-windowed legacy spawns
    /// (e.g. dev-only debug seeding) — they behave as
    /// instantly-free-for-all regardless of provenance.
    pub share: Option<ShareWindow>,
}

/// Time-only pickup window. Eligibility itself lives on
/// [`Item::provenance`]; this struct just tells the pickup
/// path when to stop enforcing it. Decoupled from any live
/// party / instance state so that someone leaving the run
/// mid-window doesn't retroactively change who can claim
/// the loot.
#[derive(Clone, Debug)]
pub struct ShareWindow {
    /// Server tick after which the eligibility check is
    /// skipped. Computed at drop time as
    /// `current_tick + SHARE_WINDOW_TICKS` so we never need to
    /// know wall-clock time to evaluate the gate.
    pub expires_at_tick: NetTick,
}

/// Roll the drop table for the killed enemy and spawn the resulting
/// [`ServerLoot`] entities. Pushes a [`WorldEvent::LootDropped`]
/// per drop. Idempotent on `Vec` \u2014 caller batches multiple kills
/// per tick.
///
/// Each rolled [`Item`] is stamped with a [`LootProvenance`]
/// snapshot of every [`ServerPlayer::character_id`] currently
/// present in the world (i.e. the participants of the rift run
/// at the moment of the kill). The accompanying [`ShareWindow`]
/// expires `share_window_ticks` later, after which the gate
/// lifts and any Sim-peer can claim the drop. Players whose
/// session hasn't bound a `character_id` yet (very first hello
/// tick) are skipped — the resulting empty provenance is treated
/// by the pickup path as "self-bind on first interaction".
///
/// `tick` + `enemy_net_id` together seed the [`LootRng`] so all
/// observers can re-derive the same drop offline if needed (e.g. a
/// future replay tool); in the live game we simply trust the
/// authoritative wire payload.
pub fn drop_for_enemy(
    world: &mut hecs::World,
    next_loot_net_id: &mut u32,
    events: &mut Vec<WorldEvent>,
    tick: NetTick,
    enemy_net_id: NetId,
    role: MonsterRole,
    enemy_pos: Vec3,
    floor_index: u32,
    share_window_ticks: u32,
) {
    let table = drops::table_for(role);
    // Seed: floor pollutes the seed so re-entering a floor produces
    // different drops; net_id keeps drops within a tick distinct.
    let seed = (tick.0 as u64)
        ^ (enemy_net_id.0 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ ((floor_index as u64) << 48);
    let mut rng = LootRng::new(seed);
    // Item-level scales with floor depth. Clamp to >=1.
    let ilvl = (floor_index + 1).max(1);
    let drops_rolled = table.roll(&mut rng, ilvl);

    // Snapshot every Sim-peer's character UUID into a single
    // `LootProvenance` shared across this kill's drops. Cloned
    // per item so each rolled stack carries its own owned copy
    // (cheap \u2014 party sizes are tiny). Players whose Hello hasn't
    // bound a `character_id` yet are skipped; if nobody has one
    // we leave provenance `None` and the pickup path will
    // self-bind to the first picker.
    let provenance = collect_world_provenance(world);
    let expires_at_tick = NetTick(tick.0.wrapping_add(share_window_ticks));

    for mut item in drops_rolled {
        let net_id = NetId(*next_loot_net_id);
        // Loot id range is 0x2000_0000..0x4000_0000 \u2014 see `Sim::new`.
        *next_loot_net_id = next_loot_net_id.wrapping_add(1);
        if *next_loot_net_id >= 0x4000_0000 {
            *next_loot_net_id = 0x2000_0000;
        }

        item.provenance = provenance.clone();

        // Phase 5: rift-touched bonus line. Gated by floor index
        // + an independent per-drop chance gate (both live in
        // `rift_game::loot::affixes`). Hub kills are filtered
        // out by `RIFT_TOUCHED_MIN_FLOOR`; the constant is the
        // single configurable knob if we ever want to push
        // rift-touched deeper into the run.
        item.rift_touched = rift_game::loot::roll_rift_touched(&mut rng, floor_index);

        let (base_id, rarity, ilvl_w, affixes, anchored, unique_id, unique_pick) = item.to_wire();
        let provenance_wire = item.provenance.as_ref().map(|p| p.eligible.clone());
        let blob = ItemBlob {
            base_id,
            rarity,
            ilvl: ilvl_w,
            affixes,
            anchored,
            // Ground loot inherits whatever the in-memory item
            // says — fresh kills always produce stable items;
            // the unstable flag is only set at pickup-in-rift.
            unstable: item.unstable,
            provenance: provenance_wire,
            unique_id: unique_id.map(|s| s.to_string()),
            unique_pick,
            rift_touched: item.rift_touched_to_wire(),
        };

        let loot = ServerLoot {
            net_id,
            position: enemy_pos,
            item,
            // Time-bounded share gate; eligibility lives on
            // the `Item::provenance` field above.
            share: Some(ShareWindow { expires_at_tick }),
        };
        let _ = world.spawn((loot,));
        events.push(WorldEvent::LootDropped {
            loot: net_id,
            item: blob,
            position: enemy_pos.to_array(),
        });
    }
}

/// Collect every [`ServerPlayer::character_id`] currently in
/// `world` into a fresh [`LootProvenance`], or `None` if not a
/// single resident has bound their persistent UUID yet (in which
/// case the pickup path will self-bind to the first toucher).
pub fn collect_world_provenance(world: &hecs::World) -> Option<LootProvenance> {
    let ids: Vec<CharacterIdBytes> = world
        .query::<&ServerPlayer>()
        .iter()
        .filter_map(|(_, p)| p.character_id.map(|u| u.into_bytes()))
        .collect();
    if ids.is_empty() {
        None
    } else {
        Some(LootProvenance::from_ids(ids))
    }
}

/// Despawn every loot entity in the world. Called on floor change.
pub fn despawn_all(world: &mut hecs::World) {
    let stale: Vec<Entity> = world
        .query::<&ServerLoot>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in stale {
        let _ = world.despawn(e);
    }
}
