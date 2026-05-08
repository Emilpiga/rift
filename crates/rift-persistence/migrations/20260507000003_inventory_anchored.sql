-- Anchored items survive death-wipes. Tagged at roll time
-- (only Legendaries can be Anchored, ~1/5000) and tracked
-- per row so the rare drops persist across runs.
ALTER TABLE inventory_items
    ADD COLUMN IF NOT EXISTS anchored BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE stash_items
    ADD COLUMN IF NOT EXISTS anchored BOOLEAN NOT NULL DEFAULT FALSE;
