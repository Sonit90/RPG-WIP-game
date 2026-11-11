-- Persistent game data: characters, their loadouts and progression, dungeon results and
-- account settings. Lives in its own schema so the auth tables stay exactly as they were.
--
-- Source of truth (see docs/networking/PERSISTENCE.md):
--   characters, loadout (equipment + bags), talents, level/experience,
--   dungeon completions, applied results, account settings  -> this schema
--   accounts, credentials, sessions                          -> auth schema
--   queue, parties, instances, runtime state                  -> SpacetimeDB realm

CREATE SCHEMA IF NOT EXISTS game;

CREATE TABLE game.characters (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id    UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    -- The catalog row (assets/catalog/characters.json) this character was made from.
    definition_id TEXT NOT NULL,
    name          TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 32),
    level         INT NOT NULL DEFAULT 1 CHECK (level >= 1),
    experience    BIGINT NOT NULL DEFAULT 0 CHECK (experience >= 0),
    -- engine_realm::loadout::Loadout: equipment and bag, by stable item and slot keys.
    loadout       JSONB NOT NULL,
    -- Talent keys bought.
    talents       JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- One character per definition per account while every class is a starter.
    UNIQUE (account_id, definition_id)
);
CREATE INDEX characters_account_idx ON game.characters (account_id);

CREATE TABLE game.dungeon_completions (
    character_id     UUID NOT NULL REFERENCES game.characters(id) ON DELETE CASCADE,
    dungeon_id       TEXT NOT NULL,
    count            INT NOT NULL DEFAULT 0 CHECK (count >= 0),
    best_duration_us BIGINT,
    last_completed   TIMESTAMPTZ,
    PRIMARY KEY (character_id, dungeon_id)
);

-- Every result the realm reported, keyed by the realm's idempotency key: a result delivered
-- twice (the bridge crashed between persisting and acknowledging) is applied once.
CREATE TABLE game.dungeon_results (
    result_id    TEXT PRIMARY KEY,
    account_id   UUID NOT NULL,
    character_id UUID,
    dungeon_id   TEXT NOT NULL,
    success      BOOLEAN NOT NULL,
    duration_us  BIGINT NOT NULL,
    experience   INT NOT NULL CHECK (experience >= 0),
    applied_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX dungeon_results_character_idx ON game.dungeon_results (character_id);

-- Account-scoped settings (keybinds, audio, camera, interface…). Machine-scoped settings —
-- display mode, resolution, graphics quality — stay in the client's settings.cfg.
CREATE TABLE game.account_settings (
    account_id UUID PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
    version    BIGINT NOT NULL DEFAULT 1,
    settings   JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
