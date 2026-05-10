-- Issuer-tagged account identity.
--
-- Adds two columns to `accounts` so the persistence layer can
-- distinguish a Steam player from a dev player who happened to
-- pick the same identity string, without colliding on the
-- existing `name` column.
--
--   * `account_key`  — `"steam:<steamid64>"` or `"dev:<identity>"`
--                      (see `rift_server::auth::AccountKey`).
--   * `display_name` — human-readable label (Steam persona name
--                      once the Web API integration lands; the
--                      dev identity string otherwise).
--
-- This migration is intentionally **additive**: existing code
-- still reads / writes by `accounts.name`, so no behaviour
-- changes today. A follow-up migration will promote
-- `account_key` to NOT NULL UNIQUE once the persistence layer
-- and server flow have been switched over to key on it.
--
-- Backfill rule: every pre-existing row was created via the
-- old free-form account-name flow, which is functionally
-- equivalent to the new `dev:` issuer. So we tag historical
-- rows as `dev:<name>` and use `name` as the human label.

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS account_key  TEXT,
    ADD COLUMN IF NOT EXISTS display_name TEXT;

UPDATE accounts
    SET account_key  = 'dev:' || name
    WHERE account_key IS NULL;

UPDATE accounts
    SET display_name = name
    WHERE display_name IS NULL;

-- Non-unique index for now so backfilled rows that share an
-- identity (shouldn't happen, but defensive) don't fail the
-- migration. The follow-up migration will replace this with a
-- unique index after the server is keying lookups on
-- `account_key` and any duplicates have been resolved.
CREATE INDEX IF NOT EXISTS accounts_account_key_idx
    ON accounts (account_key);
