-- Pickup-eligibility lineage for rolled items. `NULL` is the
-- legacy state for rows that pre-date the provenance system; the
-- runtime self-binds NULL to the holding character on first
-- interaction (equip / drop / pickup), so no SQL backfill is
-- required to close the legacy loophole.
--
-- Stored as `UUID[]` (Postgres array of UUIDs). The set is small
-- (party-bounded), order is not significant, and existing
-- `inventory_items` / `stash_items` queries are extended in
-- lockstep so each row continues to round-trip in a single SELECT.
ALTER TABLE inventory_items
    ADD COLUMN provenance UUID[] NULL;

ALTER TABLE stash_items
    ADD COLUMN provenance UUID[] NULL;
