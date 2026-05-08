-- Enforce: each non-sentinel ability wire id appears in at most
-- one loadout slot per character. Mirrors the in-game invariant
-- enforced by `rift_game::loadout::Loadout::set_slot` /
-- `Loadout::normalize` so a hand-rolled DB write (or an older
-- buggy client / server) can't slip a duplicate past the
-- gameplay layer. Sentinel `255` (empty slot) is allowed to
-- repeat \u2014 a fresh hero has five empties.
--
-- Implemented as a `CHECK` invoking an immutable helper because
-- Postgres `CHECK` clauses can't contain subqueries directly.

CREATE OR REPLACE FUNCTION loadout_no_duplicate_abilities(arr SMALLINT[])
RETURNS BOOLEAN
LANGUAGE SQL
IMMUTABLE
AS $$
    SELECT COALESCE(
        (
            SELECT COUNT(*) = COUNT(DISTINCT x)
            FROM unnest(arr) AS x
            WHERE x <> 255
        ),
        TRUE
    );
$$;

-- One-time repair: collapse duplicates on existing rows so the
-- CHECK can be applied without rejecting them. For each row we
-- keep the first occurrence of every non-sentinel id (positional
-- order with `WITH ORDINALITY`) and replace later duplicates
-- with the empty sentinel. Mirrors the "first wins" pass in the
-- in-game `Loadout::normalize` (the in-game code keeps the
-- *later* occurrence to match `set_slot` semantics; for a one-shot
-- DB repair either direction is correct \u2014 we pick first-wins for
-- a stable, deterministic SQL expression).
UPDATE characters c
SET loadout = repaired.arr
FROM (
    SELECT
        c2.id,
        ARRAY(
            SELECT CASE
                WHEN x = 255 THEN 255::SMALLINT
                WHEN ord = MIN(ord) FILTER (WHERE x <> 255) OVER (PARTITION BY x)
                    THEN x
                ELSE 255::SMALLINT
            END
            FROM unnest(c2.loadout) WITH ORDINALITY AS u(x, ord)
            ORDER BY ord
        ) AS arr
    FROM characters c2
) AS repaired
WHERE c.id = repaired.id
  AND c.loadout IS DISTINCT FROM repaired.arr;

ALTER TABLE characters
    ADD CONSTRAINT loadout_no_duplicate_abilities
    CHECK (loadout_no_duplicate_abilities(loadout));
