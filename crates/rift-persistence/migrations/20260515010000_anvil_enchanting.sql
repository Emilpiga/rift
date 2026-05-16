ALTER TABLE inventory_items
    ADD COLUMN IF NOT EXISTS enchanted_affix_index SMALLINT NULL;

ALTER TABLE stash_items
    ADD COLUMN IF NOT EXISTS enchanted_affix_index SMALLINT NULL;
