-- Character names are scoped to an account, not globally unique.
-- The original migration constrained `characters.name` UNIQUE which
-- meant two players on different accounts couldn't share a name.

ALTER TABLE characters DROP CONSTRAINT IF EXISTS characters_name_key;
DROP INDEX IF EXISTS characters_name_key;

CREATE UNIQUE INDEX IF NOT EXISTS characters_account_name_uniq
    ON characters (account_id, name);
