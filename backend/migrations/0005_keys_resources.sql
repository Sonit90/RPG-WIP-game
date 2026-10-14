-- The P0 game loop: dungeon keys and the rewards a run pays (docs/networking/GAME_LOOP.md).
--
-- One key per character, provisioned with the character (and back-filled on list for older
-- characters). Resources live inside the character's loadout JSONB, so they need no table.

CREATE TABLE game.dungeon_keys (
    character_id UUID PRIMARY KEY REFERENCES game.characters(id) ON DELETE CASCADE,
    account_id   UUID NOT NULL,
    dungeon_id   TEXT NOT NULL,
    level        INT  NOT NULL CHECK (level >= 1),
    -- Reserved for the modifier ticket; always [] in P0.
    modifiers    JSONB NOT NULL DEFAULT '[]'::jsonb,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX dungeon_keys_account_idx ON game.dungeon_keys (account_id);

-- What the realm reported about the key and the resources, next to the experience.
ALTER TABLE game.dungeon_results
    ADD COLUMN key_level       INT     NOT NULL DEFAULT 0,
    ADD COLUMN key_owner       BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN key_level_after INT     NOT NULL DEFAULT 0,
    ADD COLUMN resources       JSONB   NOT NULL DEFAULT '[]'::jsonb;
