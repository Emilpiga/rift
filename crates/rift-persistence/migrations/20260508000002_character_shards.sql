-- Persist a per-character "shards" currency. Shards are minted
-- by salvaging unwanted loot and spent on stash expansion (and
-- in the future: crafting, shrine donations, cosmetics).
--
-- Default 0 means "fresh character with nothing salvaged yet".
-- Stored as INT NOT NULL so the server never has to deal with
-- a NULL while spending; legitimate shard counts stay well
-- under i32::MAX.

ALTER TABLE characters
    ADD COLUMN IF NOT EXISTS shards INTEGER NOT NULL DEFAULT 0;
