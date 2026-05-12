-- Phase 4 — Named Legendary uniques. Items that roll a
-- `loot::uniques::UniqueDef` stamp the unique's stable string
-- id (`unique_id`) onto the persisted row so the identity
-- survives save / load round-trips independent of the in-memory
-- `UNIQUES` table ordering. Pool-roll uniques (today only
-- Mirrorglass) additionally store the sampled pool index in
-- `unique_pick` so the resolved effect is stable across loads.
--
-- Both columns default `NULL`:
--   • non-Legendary rows leave both columns NULL,
--   • procedural Legendaries (no authored match) leave both NULL,
--   • Fixed uniques fill `unique_id` only,
--   • Pool uniques fill both columns.
--
-- Existing inventory and stash rows decode as procedural
-- legendaries — no backfill required.

ALTER TABLE inventory_items
    ADD COLUMN unique_id   TEXT NULL,
    ADD COLUMN unique_pick SMALLINT NULL;

ALTER TABLE stash_items
    ADD COLUMN unique_id   TEXT NULL,
    ADD COLUMN unique_pick SMALLINT NULL;
