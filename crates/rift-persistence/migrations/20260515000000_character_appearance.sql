-- Persist character creator cosmetic choices.
--
-- Layout: [skin_tone, hair_style, eyebrow_style, hair_color,
-- eyebrow_color, chest_size].
ALTER TABLE characters
    ADD COLUMN IF NOT EXISTS appearance SMALLINT[] NOT NULL DEFAULT ARRAY[0, 0, 0, 16, 16, 128]::SMALLINT[];
