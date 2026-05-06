-- Track which inventory rows are equipped vs sitting in the bag.
--
-- A NULL `equipped_slot` means the row lives in the bag (default
-- for fresh pickups). A non-null value is the byte from
-- `rift_game::loot::EquipSlot::to_u8` (0 = Weapon, ..., 8 = Amulet),
-- so the same column round-trips wholesale through
-- `PersistedItem::equipped_slot`.
--
-- Equipment toggles go through a "rewrite the character's bag" op
-- (`PersistenceMsg::ResetCharacterInventory`) rather than tracking
-- per-item ids, which keeps the in-memory `Item` allocation-free
-- of an extra UUID. Inventories are tiny (<<100 rows) so the
-- DELETE+INSERT cost is negligible.

ALTER TABLE inventory_items
    ADD COLUMN IF NOT EXISTS equipped_slot SMALLINT NULL;

CREATE INDEX IF NOT EXISTS idx_inventory_items_equipped
    ON inventory_items (character_id, equipped_slot)
    WHERE equipped_slot IS NOT NULL;
