-- Persist the highest rift floor each character has ever cleared
-- (boss killed). Drives the "start floor" picker in the portal
-- modal: the player can pick any floor from 1..=deepest_cleared_floor
-- to skip ahead through content they have already mastered.
--
-- Default 0 means "has never killed a boss" — UI surfaces this as
-- "must start at floor 1".

ALTER TABLE characters
    ADD COLUMN IF NOT EXISTS deepest_cleared_floor INTEGER NOT NULL DEFAULT 0;
