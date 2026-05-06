-- Per-character private stash persistence.
--
-- Mirrors `inventory_items` but without the `equipped_slot` column:
-- the stash is purely storage, items there can never be active. The
-- runtime `Item` is still keyed by its stable `&'static str`
-- `BaseItem.id`, so saved stashes survive `BASE_ITEMS` /
-- `AFFIX_POOL` reordering across rebuilds.
--
-- One row per stored item. `affixes` is a JSONB array of
-- `{"id": "<affix_id>", "v": <f32>}` objects — same shape as the
-- inventory table.

CREATE TABLE IF NOT EXISTS stash_items (
    id           UUID PRIMARY KEY,
    character_id UUID NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    base_id      TEXT NOT NULL,
    rarity       SMALLINT NOT NULL,
    ilvl         INTEGER NOT NULL,
    affixes      JSONB NOT NULL DEFAULT '[]'::jsonb,
    acquired_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_stash_items_character_id
    ON stash_items (character_id);
