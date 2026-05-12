-- Phase 5 — Rift-touched bonus line. Drops from inside a rift
-- past `RIFT_TOUCHED_MIN_FLOOR` may roll one extra "rift-touched"
-- line that lives in its own dedicated slot, scales by floor
-- depth rather than ilvl, and **survives extraction** (the line
-- is a permanent record of how deep the item came from).
--
-- Stored as three nullable columns rather than a JSON blob so a
-- migration that prunes the pool can `UPDATE … SET
-- rift_touched_id = NULL WHERE rift_touched_id NOT IN (...)`
-- without parsing JSON.
--
-- All three columns are `Some(_)` together or `NULL` together —
-- the loader treats any partial row defensively as `None`.
--
-- Existing inventory and stash rows decode with `rift_touched =
-- None`, identical to a hub drop. No backfill required.

ALTER TABLE inventory_items
    ADD COLUMN rift_touched_id    TEXT NULL,
    ADD COLUMN rift_touched_value REAL NULL,
    ADD COLUMN rift_touched_depth SMALLINT NULL;

ALTER TABLE stash_items
    ADD COLUMN rift_touched_id    TEXT NULL,
    ADD COLUMN rift_touched_value REAL NULL,
    ADD COLUMN rift_touched_depth SMALLINT NULL;
