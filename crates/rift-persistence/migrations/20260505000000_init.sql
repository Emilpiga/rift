-- Initial persistence schema.
--
-- One row per account, one row per character. For now there is
-- a 1:1 relationship in practice (one character per account) but
-- the schema is shaped so we can add multiple characters per
-- account without a migration.

CREATE TABLE IF NOT EXISTS accounts (
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS characters (
    id          UUID PRIMARY KEY,
    account_id  UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name        TEXT NOT NULL UNIQUE,
    class_id    TEXT NOT NULL,
    gender      SMALLINT NOT NULL,
    level       INTEGER NOT NULL DEFAULT 1,
    xp          INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_characters_account_id
    ON characters (account_id);
