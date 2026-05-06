-- Per-character inventory persistence.
--
-- One row per stored item. The wire/runtime `Item` is rolled
-- against the static `BASE_ITEMS` and `AFFIX_POOL` tables in
-- `rift-game`; we persist by their stable `&'static str` ids
-- rather than the volatile pool indices used on the wire, so
-- saved inventories survive rebuilds that reorder the pools.
--
-- `affixes` is a JSONB array of `{"id": "<affix_id>", "v": <f32>}`
-- objects. JSONB keeps the schema flat (no separate child table)
-- and lets us round-trip arbitrary affix counts without a fixed
-- column ceiling.

CREATE TABLE IF NOT EXISTS inventory_items (
    id           UUID PRIMARY KEY,
    character_id UUID NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    -- Stable string id matching `BaseItem.id`. Survives pool
    -- reordering across rebuilds.
    base_id      TEXT NOT NULL,
    -- Rarity discriminant byte: Common=0, Magic=1, Rare=2, Legendary=3.
    rarity       SMALLINT NOT NULL,
    ilvl         INTEGER NOT NULL,
    -- `[{"id": "affix_str_id", "v": rolled_value}, ...]`.
    affixes      JSONB NOT NULL DEFAULT '[]'::jsonb,
    acquired_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_inventory_items_character_id
    ON inventory_items (character_id);
