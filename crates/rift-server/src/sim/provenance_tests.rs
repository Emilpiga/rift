//! Server-side guards on the loot provenance system. Covers:
//! `drop_for_enemy` snapshots every Sim-peer's `character_id`
//! onto the rolled `Item::provenance`; the time-bounded
//! [`super::loot::ShareWindow`] gates pickup against that
//! provenance and lifts after [`SHARE_WINDOW_TICKS`]; legacy
//! drops without provenance self-bind to the first toucher
//! (pickup or equip); player drops preserve / self-bind
//! provenance.
use super::*;
use rift_game::kinematic::Kinematic;
use rift_game::loot::{Item, LootProvenance, LootRng, Rarity, BASE_ITEMS};
use rift_net::messages::PickupRejectReason;

fn cid(raw: u64) -> rift_net::ids::ClientId {
    rift_net::ids::ClientId(raw)
}

fn rolled(base_id: &str, ilvl: u32, seed: u64) -> Item {
    let base = BASE_ITEMS
        .iter()
        .find(|b| b.id == base_id)
        .expect("base id missing from BASE_ITEMS");
    let mut rng = LootRng::new(seed);
    Item::roll(base, Rarity::Common, ilvl, &mut rng)
}

/// Stand up a non-hub Sim (floor 1), spawn `n` clients with
/// ids `1..=n`, force their character level high enough to
/// equip anything, mint each a deterministic
/// `character_id`, snap them all to the same world position
/// so [`PICKUP_RANGE`] is never the gating factor, and seed
/// the cached tick to a known value.
fn setup_party(n: u64, level: u32, tick: u32) -> (Sim, Vec<rift_net::ids::ClientId>) {
    let mut sim = Sim::new(0xFEED_FACE, 1);
    sim.current_tick = NetTick(tick);
    let mut clients = Vec::with_capacity(n as usize);
    for raw in 1..=n {
        let c = cid(raw);
        let _ = sim.spawn_player(c);
        let entity = *sim.sessions.get(&c).expect("session registered");
        let (p, kinematic) = sim
            .world
            .query_one_mut::<(&mut ServerPlayer, &mut Kinematic)>(entity)
            .unwrap();
        p.level = level;
        p.character_id = Some(char_uuid(raw));
        // Stack everyone on the floor's spawn so PICKUP_RANGE
        // is satisfied for every test pickup.
        kinematic.position = sim.floor.spawn_pos;
        clients.push(c);
    }
    (sim, clients)
}

/// Deterministic per-client UUID so the provenance set is
/// reproducible across test runs. The value itself doesn't
/// matter — only that distinct `raw` ids produce distinct
/// UUIDs and the same `raw` reproduces the same UUID.
fn char_uuid(raw: u64) -> rift_persistence::Uuid {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&raw.to_le_bytes());
    rift_persistence::Uuid::from_bytes(bytes)
}

fn loot_count(sim: &Sim) -> usize {
    sim.world.query::<&loot::ServerLoot>().iter().count()
}

fn the_loot(sim: &Sim) -> rift_net::NetId {
    let mut count = 0;
    let mut found = None;
    for (_, l) in sim.world.query::<&loot::ServerLoot>().iter() {
        found = Some(l.net_id);
        count += 1;
    }
    assert_eq!(count, 1, "expected exactly one ground loot, found {count}");
    found.unwrap()
}

/// Roll one drop on the floor at `pos` via `drop_for_enemy`
/// using the *current* sim peers as the provenance source.
/// Returns the spawned loot's net-id. Roles with empty drop
/// tables are filtered out at table_for; we use Boss to
/// guarantee at least one rolled item.
fn drop_one_via_enemy(
    sim: &mut Sim,
    pos: glam::Vec3,
    seed_tick: u32,
    share_window_ticks: u32,
) -> rift_net::NetId {
    let before = loot_count(sim);
    let mut events = Vec::new();
    loot::drop_for_enemy(
        &mut sim.world,
        &mut sim.next_loot_net_id,
        &mut events,
        NetTick(seed_tick),
        rift_net::NetId(0xDEAD_BEEF),
        rift_game::monsters::MonsterRole::Boss,
        pos,
        sim.floor_index,
        share_window_ticks,
    );
    let mut new_id = None;
    for (_, l) in sim.world.query::<&loot::ServerLoot>().iter() {
        new_id = Some(l.net_id);
    }
    assert!(
        loot_count(sim) > before,
        "drop_for_enemy must spawn at least one ServerLoot for a Boss kill",
    );
    new_id.expect("at least one ServerLoot present")
}

#[test]
fn drop_for_enemy_stamps_provenance_with_all_sim_peers() {
    let (mut sim, clients) = setup_party(3, 50, 100);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 100, SHARE_WINDOW_TICKS);
    // Every dropped item should carry a provenance covering
    // all three party members.
    let mut found = None;
    for (_, l) in sim.world.query::<&loot::ServerLoot>().iter() {
        if l.net_id == net_id {
            found = Some(l.item.provenance.clone());
            break;
        }
    }
    let prov = found
        .flatten()
        .expect("provenance must be set on enemy drop");
    for c in &clients {
        assert!(
            prov.allows(&char_uuid(c.0).into_bytes()),
            "provenance should include party member {c:?}",
        );
    }
}

#[test]
fn drop_for_enemy_share_window_expires_at_expected_tick() {
    let (mut sim, _clients) = setup_party(2, 50, 100);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 100, SHARE_WINDOW_TICKS);
    let mut share = None;
    for (_, l) in sim.world.query::<&loot::ServerLoot>().iter() {
        if l.net_id == net_id {
            share = l.share.clone();
            break;
        }
    }
    let share = share.expect("enemy drop must carry a share window");
    assert_eq!(
        share.expires_at_tick.0,
        100u32 + SHARE_WINDOW_TICKS,
        "expiry must equal kill_tick + SHARE_WINDOW_TICKS",
    );
}

#[test]
fn eligible_peer_can_pick_up_within_window() {
    let (mut sim, clients) = setup_party(2, 50, 0);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 0, SHARE_WINDOW_TICKS);
    // clients[1] is on the provenance snapshot \u2192 pickup
    // should succeed.
    let result = sim.try_pickup_loot(clients[1], net_id);
    assert!(result.is_ok(), "eligible peer should succeed: {result:?}");
    let still_present = sim
        .world
        .query::<&loot::ServerLoot>()
        .iter()
        .any(|(_, l)| l.net_id == net_id);
    assert!(!still_present, "picked-up loot must be despawned");
}

#[test]
fn ineligible_peer_blocked_during_window() {
    // Drop with only client 1 in the Sim, then add a
    // latecomer whose character_id was not on the snapshot.
    let (mut sim, _clients) = setup_party(1, 50, 0);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 0, SHARE_WINDOW_TICKS);

    let latecomer = cid(99);
    let _ = sim.spawn_player(latecomer);
    {
        let entity = *sim.sessions.get(&latecomer).unwrap();
        let (p, kinematic) = sim
            .world
            .query_one_mut::<(&mut ServerPlayer, &mut Kinematic)>(entity)
            .unwrap();
        p.level = 50;
        p.character_id = Some(char_uuid(99));
        kinematic.position = sim.floor.spawn_pos;
    }

    let result = sim.try_pickup_loot(latecomer, net_id);
    assert!(
        matches!(result, Err(Some(PickupRejectReason::NotEligible))),
        "latecomer must be NotEligible, got {result:?}",
    );
    let still_present = sim
        .world
        .query::<&loot::ServerLoot>()
        .iter()
        .any(|(_, l)| l.net_id == net_id);
    assert!(still_present, "rejected pickup leaves loot on ground");
}

#[test]
fn ineligible_peer_can_pick_up_after_window_expires() {
    let (mut sim, _clients) = setup_party(1, 50, 0);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 0, SHARE_WINDOW_TICKS);

    let latecomer = cid(99);
    let _ = sim.spawn_player(latecomer);
    {
        let entity = *sim.sessions.get(&latecomer).unwrap();
        let (p, kinematic) = sim
            .world
            .query_one_mut::<(&mut ServerPlayer, &mut Kinematic)>(entity)
            .unwrap();
        p.level = 50;
        p.character_id = Some(char_uuid(99));
        kinematic.position = sim.floor.spawn_pos;
    }

    sim.current_tick = NetTick(SHARE_WINDOW_TICKS + 1);
    let result = sim.try_pickup_loot(latecomer, net_id);
    assert!(result.is_ok(), "gate must lift after window: {result:?}");
}

#[test]
fn solo_rift_drop_binds_only_the_soloer() {
    // 1 player in the Sim → provenance is exactly that
    // player's character_id; no one else can pick it up
    // during the window.
    let (mut sim, clients) = setup_party(1, 50, 0);
    let pos = sim.floor.spawn_pos;
    let net_id = drop_one_via_enemy(&mut sim, pos, 0, SHARE_WINDOW_TICKS);
    let prov = sim
        .world
        .query::<&loot::ServerLoot>()
        .iter()
        .find(|(_, l)| l.net_id == net_id)
        .and_then(|(_, l)| l.item.provenance.clone())
        .expect("provenance set");
    assert_eq!(prov.eligible.len(), 1, "solo drop must bind exactly one id");
    assert!(prov.allows(&char_uuid(clients[0].0).into_bytes()));
}

#[test]
fn equip_self_binds_legacy_bag_item() {
    // Legacy item already in the bag (e.g. loaded from a
    // pre-migration row). Equipping it must bind the
    // equipper's character_id onto provenance.
    let (mut sim, clients) = setup_party(1, 50, 0);
    let mut item = rolled("staff_basic", 1, 8);
    item.provenance = None;
    let owner = clients[0];
    let entity = *sim.sessions.get(&owner).unwrap();
    {
        let mut p = sim.world.get::<&mut ServerPlayer>(entity).unwrap();
        p.inventory.push(Some(item));
    }
    assert!(sim.equip_from_bag(owner, 0, None), "equip should succeed");
    let p = sim.world.get::<&ServerPlayer>(entity).unwrap();
    let equipped = p
        .equipment
        .get(rift_game::loot::EquipSlot::Weapon)
        .expect("weapon equipped");
    let prov = equipped.provenance.as_ref().expect("self-bound on equip");
    assert!(prov.allows(&char_uuid(owner.0).into_bytes()));
}

#[test]
fn player_drop_preserves_existing_provenance() {
    // An item whose provenance was already stamped (drop
    // → pickup → drop again) must not have its provenance
    // overwritten; the dropper isn't necessarily the only
    // eligible holder.
    let (mut sim, clients) = setup_party(2, 50, 0);
    let mut item = rolled("staff_basic", 1, 9);
    item.provenance = Some(LootProvenance::from_ids([
        char_uuid(clients[0].0).into_bytes(),
        char_uuid(clients[1].0).into_bytes(),
    ]));
    let dropper = clients[0];
    let (popped, pos) = sim.pop_inventory_item_from_seed(dropper, item);
    sim.spawn_player_drop(popped, pos, dropper);
    let prov = sim
        .world
        .query::<&loot::ServerLoot>()
        .iter()
        .next()
        .and_then(|(_, l)| l.item.provenance.clone())
        .expect("provenance preserved");
    assert_eq!(prov.eligible.len(), 2, "both original ids must survive");
}

#[test]
fn player_drop_self_binds_legacy_item() {
    // Dropping a `provenance: None` bag item must self-bind
    // to the dropper rather than persist the legacy state.
    let (mut sim, clients) = setup_party(1, 50, 0);
    let mut item = rolled("staff_basic", 1, 10);
    item.provenance = None;
    let dropper = clients[0];
    let (popped, pos) = sim.pop_inventory_item_from_seed(dropper, item);
    sim.spawn_player_drop(popped, pos, dropper);
    let prov = sim
        .world
        .query::<&loot::ServerLoot>()
        .iter()
        .next()
        .and_then(|(_, l)| l.item.provenance.clone())
        .expect("self-bound on drop");
    assert_eq!(prov.eligible.len(), 1);
    assert!(prov.allows(&char_uuid(dropper.0).into_bytes()));
}

#[test]
fn is_hub_distinguishes_floor_zero_from_rifts() {
    let hub = Sim::new(0, 0);
    assert!(hub.is_hub(), "floor 0 must be the hub");
    let rift = Sim::new(0, 1);
    assert!(!rift.is_hub(), "floor 1 must not be the hub");
}
