-- Add per-character talent investment + unspent point pool.
--
-- `talents` is a flat SMALLINT[] of (id, rank) pairs — even-length,
-- where slot 2k is the `TalentId` and slot 2k+1 is the invested
-- rank. Nodes at rank 0 are omitted so the row size scales with
-- the player's actual investment, not the tree size. See
-- `rift_game::talents::TalentTree`.
--
-- `talent_unspent` mirrors `TalentTree::unspent_points`. Per
-- `TALENT_TREE.md` §6 the design grants 1 starter point at level
-- 1 + 1 per level thereafter; existing rows are back-filled to
-- `GREATEST(level, 1)` so legacy characters don't show up at zero
-- points after the gate enables.

ALTER TABLE characters
    ADD COLUMN talents SMALLINT[] NOT NULL DEFAULT '{}';

ALTER TABLE characters
    ADD COLUMN talent_unspent INTEGER NOT NULL DEFAULT 1;

UPDATE characters SET talent_unspent = GREATEST(level, 1);
