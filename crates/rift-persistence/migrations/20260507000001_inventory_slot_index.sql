-- Persist user-defined bag/stash ordering.
--
-- The new drag-and-drop UI lets the player reorder bag and
-- stash slots arbitrarily. Without an explicit position column,
-- the load path's `ORDER BY acquired_at, id` would shuffle a
-- reordered bag back to acquisition order on the next login.
--
-- `slot_index` is a 0-based dense index into the bag (or stash)
-- as the player last saw it. For equipped rows the column is
-- still written but ignored on load (the row is routed by
-- `equipped_slot` instead). All resets push fresh values, so
-- the column stays in sync with the in-memory `Vec<Item>`
-- positions.

ALTER TABLE inventory_items
    ADD COLUMN IF NOT EXISTS slot_index INTEGER NOT NULL DEFAULT 0;

ALTER TABLE stash_items
    ADD COLUMN IF NOT EXISTS slot_index INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_inventory_items_slot
    ON inventory_items (character_id, slot_index);

CREATE INDEX IF NOT EXISTS idx_stash_items_slot
    ON stash_items (character_id, slot_index);
