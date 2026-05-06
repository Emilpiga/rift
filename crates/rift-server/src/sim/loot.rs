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
use rift_game::loot::{drops, Item, LootRng};
use rift_game::monsters::MonsterRole;
use rift_net::{
    messages::{ItemBlob, WorldEvent},
    NetId, NetTick,
};

use super::enemy::ServerEnemy;

/// Context the death-finalisation path needs to roll loot. Passed
/// down from [`super::Sim::step`] into the projectile / debuff
/// damage helpers so each subsystem can finalise kills uniformly.
pub struct DeathCtx<'a> {
    pub events: &'a mut Vec<WorldEvent>,
    pub next_loot_net_id: &'a mut u32,
    pub tick: NetTick,
    pub floor_index: u32,
}

/// Finalise a batch of kills queued by a damage subsystem:
/// 1. Read each dead enemy's role + position out of the ECS.
/// 2. Push a [`WorldEvent::Death`].
/// 3. Roll the [`drops::table_for`] table, spawn [`ServerLoot`]
///    entities, push [`WorldEvent::LootDropped`] per drop.
/// 4. Mark the corpse with `dying_remaining = DEATH_FADE_DUR` so
///    the snapshot keeps shipping it for the death-anim window;
///    [`super::enemy::tick_dying`] does the actual despawn once
///    the timer runs out.
pub fn finalise_kills(
    world: &mut hecs::World,
    ctx: &mut DeathCtx<'_>,
    dead: Vec<(Entity, NetId)>,
) {
    for (entity, net_id) in dead {
        // Snapshot the corpse before flipping it into dying mode \u2014
        // the loot drop needs role + position.
        let info = world
            .get::<&ServerEnemy>(entity)
            .ok()
            .map(|en| (en.role, en.k.position));
        ctx.events.push(WorldEvent::Death {
            entity: net_id,
            killer: None,
        });
        if let Some((role, pos)) = info {
            drop_for_enemy(
                world,
                ctx.next_loot_net_id,
                ctx.events,
                ctx.tick,
                net_id,
                role,
                pos,
                ctx.floor_index,
            );
        }
        if let Ok(mut en) = world.get::<&mut ServerEnemy>(entity) {
            en.dying_remaining = super::enemy::DEATH_FADE_DUR;
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
}

/// Roll the drop table for the killed enemy and spawn the resulting
/// [`ServerLoot`] entities. Pushes a [`WorldEvent::LootDropped`]
/// per drop. Idempotent on `Vec` — caller batches multiple kills
/// per tick.
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
    enemy_role: u8,
    enemy_pos: Vec3,
    floor_index: u32,
) {    let Some(role) = MonsterRole::from_wire_byte(enemy_role) else {
        return;
    };
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

    for item in drops_rolled {
        let net_id = NetId(*next_loot_net_id);
        // Loot id range is 0x2000_0000..0x4000_0000 — see `Sim::new`.
        *next_loot_net_id = next_loot_net_id.wrapping_add(1);
        if *next_loot_net_id >= 0x4000_0000 {
            *next_loot_net_id = 0x2000_0000;
        }

        let (base_id, rarity, ilvl_w, affixes) = item.to_wire();
        let blob = ItemBlob {
            base_id,
            rarity,
            ilvl: ilvl_w,
            affixes,
        };

        let loot = ServerLoot {
            net_id,
            position: enemy_pos,
            item,
        };
        let _ = world.spawn((loot,));
        events.push(WorldEvent::LootDropped {
            loot: net_id,
            item: blob,
            position: enemy_pos.to_array(),
        });
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
