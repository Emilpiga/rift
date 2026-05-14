//! Server-side guards on [`Sim::equip_from_bag`]. The roll-time
//! `required_level` formula is covered by `rift_game::loot::item`
//! tests; here we verify that the server actually consults it
//! and that a rejection leaves the bag unchanged.
use super::*;
use rift_game::loot::{Item, LootRng, Rarity, BASE_ITEMS};

fn cid(raw: u64) -> rift_net::ids::ClientId {
    rift_net::ids::ClientId(raw)
}

/// Roll a fresh item directly from the static base table at a
/// caller-chosen ilvl + seed so each test gets a deterministic
/// drop without going through the full loot-spawn pipeline.
fn rolled(base_id: &str, ilvl: u32, seed: u64) -> Item {
    let base = BASE_ITEMS
        .iter()
        .find(|b| b.id == base_id)
        .expect("base id missing from BASE_ITEMS");
    let mut rng = LootRng::new(seed);
    Item::roll(base, Rarity::Common, ilvl, &mut rng)
}

/// Stand up a hub Sim, register one client session, push a
/// rolled item into their bag, set `level`, and return the
/// pieces the test needs.
fn setup(client_level: u32, item: Item) -> (Sim, rift_net::ids::ClientId) {
    let mut sim = Sim::new(0xCAFE_BABE, 0);
    let client = cid(1);
    let _ = sim.spawn_player(client);
    let entity = *sim.sessions.get(&client).expect("session registered");
    {
        let mut p = sim
            .world
            .get::<&mut ServerPlayer>(entity)
            .expect("ServerPlayer present");
        p.level = client_level;
        p.inventory.push(Some(item));
    }
    (sim, client)
}

#[test]
fn equip_succeeds_when_player_meets_requirement() {
    let item = rolled("staff_basic", 5, 1);
    let req = item.required_level();
    let (mut sim, client) = setup(req, item);
    assert!(
        sim.equip_from_bag(client, 0),
        "equip should succeed when level >= required",
    );
    // Bag entry consumed (no displaced item, slot was empty).
    let entity = *sim.sessions.get(&client).unwrap();
    let p = sim.world.get::<&ServerPlayer>(entity).unwrap();
    assert!(
        p.inventory.iter().all(|s| s.is_none()),
        "bag should be empty after a clean equip",
    );
    assert!(
        p.equipment
            .get(rift_game::loot::EquipSlot::Weapon)
            .is_some(),
        "weapon slot should be filled after a clean equip",
    );
}

#[test]
fn equip_rejected_when_player_under_level() {
    // Roll an item at ilvl 30 — required_level() will be >= 30.
    let item = rolled("staff_basic", 30, 2);
    let req = item.required_level();
    assert!(req >= 2, "test invariant: ilvl-30 item asks for >=2");
    let (mut sim, client) = setup(1, item);
    assert!(
        !sim.equip_from_bag(client, 0),
        "equip should reject when player level < required",
    );
    // Item must still be in the bag, weapon slot empty.
    let entity = *sim.sessions.get(&client).unwrap();
    let p = sim.world.get::<&ServerPlayer>(entity).unwrap();
    assert!(
        matches!(p.inventory.get(0), Some(Some(_))),
        "bag entry should be restored after a rejected equip",
    );
    assert!(
        p.equipment
            .get(rift_game::loot::EquipSlot::Weapon)
            .is_none(),
        "weapon slot must remain empty after a rejected equip",
    );
}

#[test]
fn equip_at_exact_required_level_succeeds() {
    // Boundary: player level == required_level should pass.
    let item = rolled("staff_basic", 12, 3);
    let req = item.required_level();
    let (mut sim, client) = setup(req, item);
    assert!(
        sim.equip_from_bag(client, 0),
        "boundary req == level should equip"
    );
}

#[test]
fn equip_one_under_required_level_fails() {
    // Boundary: player level == required_level - 1 should fail.
    let item = rolled("staff_basic", 12, 4);
    let req = item.required_level();
    assert!(req >= 2, "test needs req >= 2 to subtract 1");
    let (mut sim, client) = setup(req - 1, item);
    assert!(
        !sim.equip_from_bag(client, 0),
        "boundary req - 1 should reject",
    );
}

#[test]
fn occupied_ring2_drop_replaces_ring2_not_ring1() {
    let equipped_ring = rolled("ring_basic", 5, 10);
    let bag_ring = rolled("ring_basic", 8, 11);
    let (mut sim, client) = setup(30, bag_ring);
    let entity = *sim.sessions.get(&client).unwrap();
    {
        let mut p = sim.world.get::<&mut ServerPlayer>(entity).unwrap();
        p.equipment
            .set(rift_game::loot::EquipSlot::Ring2, Some(equipped_ring));
    }

    assert!(sim.unequip_to_bag_slot(client, rift_game::loot::EquipSlot::Ring2, 0));

    let p = sim.world.get::<&ServerPlayer>(entity).unwrap();
    assert!(
        p.equipment.get(rift_game::loot::EquipSlot::Ring1).is_none(),
        "ring 1 should not be touched when replacing ring 2",
    );
    assert_eq!(
        p.equipment
            .get(rift_game::loot::EquipSlot::Ring2)
            .map(|item| item.ilvl),
        Some(8),
        "bag ring should land in ring 2",
    );
    assert_eq!(
        p.inventory
            .get(0)
            .and_then(|slot| slot.as_ref())
            .map(|item| item.ilvl),
        Some(5),
        "old ring 2 item should land back in the original bag cell",
    );
}
