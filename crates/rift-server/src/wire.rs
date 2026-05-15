//! Server-side adapters between authoritative game types and
//! lightweight wire/persistence shapes.

use rift_net::messages::ItemBlob;
use rift_net::Gender;

/// Decode a stored gender column back to the wire enum. Unknown
/// codes (forward-compat with future variants) fall back to
/// Female so we never panic on a malformed row.
pub(crate) fn gender_from_i16(g: i16) -> Gender {
    match g {
        x if x == gender_to_i16(Gender::Male) => Gender::Male,
        x if x == gender_to_i16(Gender::Female) => Gender::Female,
        _ => Gender::Female,
    }
}

/// Encode the wire gender into the smallint persisted by the
/// account store. Keep the explicit mapping in one place so enum
/// discriminants never become part of the storage contract by accident.
pub(crate) const fn gender_to_i16(g: Gender) -> i16 {
    match g {
        Gender::Male => rift_game::character::gender_byte::MALE as i16,
        Gender::Female => rift_game::character::gender_byte::FEMALE as i16,
    }
}

pub(crate) fn wire_gender_to_game(g: Gender) -> rift_game::character::Gender {
    match g {
        Gender::Male => rift_game::character::Gender::Male,
        Gender::Female => rift_game::character::Gender::Female,
    }
}

#[allow(dead_code)]
pub(crate) fn game_gender_to_wire(g: rift_game::character::Gender) -> Gender {
    match g {
        rift_game::character::Gender::Male => Gender::Male,
        rift_game::character::Gender::Female => Gender::Female,
    }
}

/// Coerce a persisted `[i16; 6]` ability loadout to the `u8` wire
/// shape. Out-of-range entries fall back to the empty-slot
/// sentinel (`u8::MAX`) so a malformed row leaves the slot
/// blank instead of accidentally re-binding Steady Shot.
pub(crate) fn loadout_to_u8(loadout: [i16; 6]) -> [u8; 6] {
    let mut out = [rift_game::loadout::EMPTY_SLOT.raw(); 6];
    for (i, &slot) in loadout.iter().enumerate() {
        out[i] = u8::try_from(slot).unwrap_or(rift_game::loadout::EMPTY_SLOT.raw());
    }
    out
}

/// Convert an authoritative `rift_game::loot::Item` into the
/// `ItemBlob` shape that ships over the wire. Centralised so all
/// inventory, equipment, loot-drop and snapshot builders agree on
/// the field layout.
pub(crate) fn item_to_blob(item: &rift_game::loot::Item) -> ItemBlob {
    let (base_id, rarity, ilvl, affixes, anchored, unique_id, unique_pick) = item.to_wire();
    ItemBlob {
        base_id,
        rarity,
        ilvl,
        affixes,
        anchored,
        unstable: item.unstable,
        provenance: provenance_to_wire(item),
        unique_id: unique_id.map(|s| s.to_string()),
        unique_pick,
        rift_touched: item.rift_touched_to_wire(),
    }
}

/// Inverse of [`item_to_blob`]. Kept beside the encoder so tests and
/// future receive paths validate the full carrier shape, not just the
/// legacy `Item::to_wire` tuple.
pub(crate) fn item_from_blob(blob: &ItemBlob) -> Option<rift_game::loot::Item> {
    let unique_id = blob
        .unique_id
        .as_deref()
        .and_then(|id| rift_game::loot::uniques::find(id).map(|u| u.id));
    let mut item = rift_game::loot::Item::from_wire(
        blob.base_id,
        blob.rarity,
        blob.ilvl,
        &blob.affixes,
        blob.anchored,
        provenance_from_wire(blob.provenance.clone()),
        unique_id,
        blob.unique_pick,
    )?;
    item.unstable = blob.unstable;
    item.rift_touched = rift_game::loot::Item::rift_touched_from_wire(blob.rift_touched);
    Some(item)
}

/// Convert an [`Item`]'s in-memory [`rift_game::loot::LootProvenance`]
/// into the `Option<Vec<[u8; 16]>>` shape used on the wire and
/// in [`rift_net::messages::ItemBlob`]. Returns `None` for
/// legacy / unprovenanced items so the receiver can route them
/// to the self-bind-on-touch path.
pub(crate) fn provenance_to_wire(item: &rift_game::loot::Item) -> Option<Vec<[u8; 16]>> {
    item.provenance.as_ref().map(|p| p.eligible.clone())
}

/// Convert an [`Item`]'s provenance into the
/// `Option<Vec<rift_persistence::Uuid>>` shape stored as
/// `provenance UUID[]` in the database. Same `None` semantics
/// as [`provenance_to_wire`].
pub(crate) fn provenance_to_persisted(
    item: &rift_game::loot::Item,
) -> Option<Vec<rift_persistence::Uuid>> {
    item.provenance.as_ref().map(|p| {
        p.eligible
            .iter()
            .map(|bytes| rift_persistence::Uuid::from_bytes(*bytes))
            .collect()
    })
}

/// Decode the wire / persisted byte vector back into a runtime
/// [`rift_game::loot::LootProvenance`]. `None` round-trips to
/// `None`. Centralised so the inverse of [`provenance_to_wire`] /
/// [`provenance_to_persisted`] is always evaluated the same way.
#[allow(dead_code)]
pub(crate) fn provenance_from_wire(
    bytes: Option<Vec<[u8; 16]>>,
) -> Option<rift_game::loot::LootProvenance> {
    bytes.map(|eligible| rift_game::loot::LootProvenance::from_ids(eligible))
}

/// Decode a persisted `Vec<Uuid>` back into a runtime
/// [`rift_game::loot::LootProvenance`]. Mirror of
/// [`provenance_from_wire`].
pub(crate) fn provenance_from_persisted(
    uuids: Option<Vec<rift_persistence::Uuid>>,
) -> Option<rift_game::loot::LootProvenance> {
    uuids.map(|v| rift_game::loot::LootProvenance::from_ids(v.into_iter().map(|u| u.into_bytes())))
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use rift_game::abilities::{self, AbilityWireId};
    use rift_game::loot::{
        EquipSlot, Item, LootProvenance, LootRng, Rarity, RolledRiftTouched, BASE_ITEMS,
        RIFT_TOUCHED_POOL,
    };
    use rift_game::monsters::{MonsterRole, ALL_ROLES};
    use rift_ui_types::inventory::EquipSlotIdx;
    use std::collections::HashSet;

    fn base(id: &str) -> &'static rift_game::loot::BaseItem {
        BASE_ITEMS
            .iter()
            .find(|b| b.id == id)
            .expect("test base item exists")
    }

    fn rolled(base_id: &str, rarity: Rarity, seed: u64) -> Item {
        let mut rng = LootRng::new(seed);
        Item::roll(base(base_id), rarity, 25, &mut rng)
    }

    fn assert_same_item(expected: &Item, actual: &Item) {
        assert_eq!(expected.base.id, actual.base.id);
        assert_eq!(expected.rarity as u8, actual.rarity as u8);
        assert_eq!(expected.ilvl, actual.ilvl);
        assert_eq!(expected.anchored, actual.anchored);
        assert_eq!(expected.unstable, actual.unstable);
        assert_eq!(expected.provenance, actual.provenance);
        assert_eq!(expected.unique_id, actual.unique_id);
        assert_eq!(expected.unique_pick, actual.unique_pick);
        assert_eq!(expected.affixes.len(), actual.affixes.len());
        for (left, right) in expected.affixes.iter().zip(&actual.affixes) {
            assert_eq!(left.def.id, right.def.id);
            assert_eq!(left.value, right.value);
        }
        assert_eq!(
            expected.rift_touched_to_wire(),
            actual.rift_touched_to_wire()
        );
    }

    #[test]
    fn equipment_slot_wire_ids_match_ui_mirror() {
        assert_eq!(EquipSlot::COUNT, EquipSlotIdx::COUNT);

        for (idx, slot) in EquipSlot::ALL.iter().copied().enumerate() {
            let wire = slot.to_u8();
            assert_eq!(wire as usize, idx);
            assert_eq!(EquipSlot::from_u8(wire), Some(slot));
            assert_ne!(EquipSlotIdx(wire).label(), "?");
        }

        assert_eq!(EquipSlot::from_u8(EquipSlot::COUNT as u8), None);
    }

    #[test]
    fn gender_wire_and_storage_mappings_round_trip() {
        for wire in [Gender::Male, Gender::Female] {
            let game = wire_gender_to_game(wire);
            assert_eq!(game_gender_to_wire(game), wire);
            assert_eq!(gender_from_i16(gender_to_i16(wire)), wire);
            assert_eq!(
                game.to_wire_byte() as i16,
                gender_to_i16(wire),
                "net gender storage byte must mirror rift-game gender byte"
            );
        }
    }

    #[test]
    fn ability_wire_ids_are_unique_and_resolve() {
        let mut seen = HashSet::new();
        for ability in abilities::REGISTRY {
            assert!(
                seen.insert(ability.wire_id.raw()),
                "duplicate ability wire id {}",
                ability.wire_id
            );
            assert_eq!(
                abilities::lookup(AbilityWireId::new(ability.wire_id.raw())).map(|a| a.id),
                Some(ability.id)
            );
        }
    }

    #[test]
    fn monster_role_wire_bytes_are_unique_and_resolve() {
        let mut seen = HashSet::new();
        for role in ALL_ROLES {
            let wire = role.to_wire_byte();
            assert!(seen.insert(wire), "duplicate monster role byte {wire}");
            assert_eq!(MonsterRole::from_wire_byte(wire), Some(role));
        }
    }

    #[test]
    fn item_blob_round_trips_full_carrier_fields() {
        let mut items = vec![
            rolled("staff_basic", Rarity::Rare, 7),
            rolled("amulet_basic", Rarity::Legendary, 11),
        ];

        items[0].anchored = true;
        items[0].unstable = true;
        items[0].provenance = Some(LootProvenance::from_ids([[1; 16], [2; 16]]));
        items[0].rift_touched = Some(RolledRiftTouched {
            def: &RIFT_TOUCHED_POOL[0],
            value: 12.5,
            depth: 42,
        });

        for item in items {
            let blob = item_to_blob(&item);
            let decoded = item_from_blob(&blob).expect("blob decodes");
            assert_same_item(&item, &decoded);
        }
    }
}
