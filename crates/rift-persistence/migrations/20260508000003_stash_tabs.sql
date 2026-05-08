-- Tabbed stash. Adds per-tab metadata in a new `stash_tabs`
-- table and a `tab_index` column on `stash_items` so each row
-- knows which page it belongs to.
--
-- Migration is fully backward-compatible: every existing
-- `stash_items` row defaults to `tab_index = 0` (the free
-- starter tab), and `stash_tabs` is lazily seeded by the server
-- the first time a character opens their stash. Players who
-- already had a stash see all their items on tab 1 with the
-- name "Tab 1" and the default neutral color.
--
-- Tab cost is computed at purchase time on the server (see
-- `Sim::buy_stash_tab`); only the resulting tab metadata
-- persists here.

ALTER TABLE stash_items
    ADD COLUMN IF NOT EXISTS tab_index SMALLINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_stash_items_character_tab
    ON stash_items (character_id, tab_index);

CREATE TABLE IF NOT EXISTS stash_tabs (
    character_id UUID NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    tab_index    SMALLINT NOT NULL,
    name         TEXT NOT NULL,
    -- Packed `0xRRGGBB` (alpha is implicit, opaque).
    color        INTEGER NOT NULL,
    PRIMARY KEY (character_id, tab_index)
);
