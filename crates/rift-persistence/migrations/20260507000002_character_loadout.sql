-- Add per-character ability loadout. Six ability wire ids (see
-- rift_game::abilities::id) addressing entries in REGISTRY.
-- `255` is the empty-slot sentinel (`rift_game::loadout::EMPTY_SLOT`,
-- bit-packed into i16 because postgres SMALLINT is signed).
-- Stored as a SMALLINT[] so we don't need six separate columns
-- and so a future change to slot count is one default rewrite
-- away.
--
-- Default array matches `Loadout::default_hero()` on the gameplay
-- side: a brand-new hero starts with only Steady Shot (wire id 0)
-- in slot 0 and every other slot empty (255). Bar slots and
-- abilities themselves unlock as the character levels up.
-- Existing rows from the previous migration default are not
-- rewritten — characters created before this change keep the
-- bar they had.

ALTER TABLE characters
    ADD COLUMN loadout SMALLINT[]
    NOT NULL DEFAULT '{0,255,255,255,255,255}'::SMALLINT[];
