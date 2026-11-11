-- Dungeon key modifiers, rerolls' currency and party key progression
-- (docs/dungeon/MODIFIERS.md, docs/networking/GAME_LOOP.md).
--
-- Numbered 0010: 0009 is reserved for the parallel economy branch.

-- `dungeon_keys.modifiers` (JSONB, [] until now) holds the key's rolled modifiers:
-- [{"id": "rotating_beams", "magnitude_pct": 0}, ...]. A key whose modifiers have never been
-- rolled is rolled once when it is next read (the flag tells "never rolled" from "cleansed
-- or genuinely has none"); every key created or whose level changes afterwards is rolled
-- at that moment.
ALTER TABLE game.dungeon_keys
    ADD COLUMN modifiers_rolled BOOLEAN NOT NULL DEFAULT false;

-- What the realm reported next to the experience: the currency a character spent during the
-- run (key rerolls), and whether the character's own key rose as a participant of the run.
ALTER TABLE game.dungeon_results
    ADD COLUMN currency_spent       JSONB   NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN participant_key_bump BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN own_key_level        INT     NOT NULL DEFAULT 0;
