-- Promote `accounts.account_key` to the primary identity column.
--
-- Phase 4 of the Steam-auth migration was additive: it added
-- `account_key` + `display_name` alongside the legacy
-- `accounts.name`, backfilled them from `name`, and left the
-- old column as the lookup key. This migration completes the
-- swap:
--
--   * `account_key` becomes `NOT NULL UNIQUE` (issuer-tagged
--     storage form, e.g. `dev:alice` or `steam:76561198000000000`).
--   * `accounts.name` becomes nullable + non-unique. The column
--     remains for now so old data isn't lost and rollbacks are
--     trivial; a later cleanup migration can drop it once
--     nothing reads it.
--
-- Defensive backfill: any row that somehow still lacks an
-- `account_key` (shouldn't happen post-Phase-4, but a partial
-- migration on a developer DB could land us here) gets the
-- same `dev:<name>` tag the previous migration used.

UPDATE accounts
    SET account_key = 'dev:' || name
    WHERE account_key IS NULL AND name IS NOT NULL;

-- Replace the non-unique scout index with a real uniqueness
-- constraint. We use a unique index rather than `ADD CONSTRAINT`
-- so it can be created `IF NOT EXISTS` and so a future migration
-- can drop / rebuild it without touching the table definition.
DROP INDEX IF EXISTS accounts_account_key_idx;
CREATE UNIQUE INDEX IF NOT EXISTS accounts_account_key_uniq
    ON accounts (account_key);

ALTER TABLE accounts
    ALTER COLUMN account_key SET NOT NULL;

-- Drop the legacy uniqueness + NOT NULL on `name`. The column
-- stays for now (so a rollback only needs to flip these
-- constraints back), but the persistence layer no longer reads
-- or writes it for new accounts.
ALTER TABLE accounts
    DROP CONSTRAINT IF EXISTS accounts_name_key;
DROP INDEX IF EXISTS accounts_name_key;
ALTER TABLE accounts
    ALTER COLUMN name DROP NOT NULL;
