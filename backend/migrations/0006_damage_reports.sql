-- Per-player damage reports for finished runs, read by the operator dashboard's anomaly panel
-- (docs/networking/ADMIN.md). Nothing writes it yet: the realm does not report damage today.
-- The table exists so the reader, the thresholds and the report shape are settled before the
-- writer lands.

CREATE TABLE game.dungeon_damage_reports (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The player's dungeon result this report belongs to (`<instance id>:<account id>`).
    result_id        TEXT UNIQUE REFERENCES game.dungeon_results(result_id) ON DELETE CASCADE,
    instance_id      TEXT NOT NULL,
    dungeon_id       TEXT NOT NULL,
    key_level        INT  NOT NULL DEFAULT 0,
    character_id     UUID,
    account_id       UUID NOT NULL,
    class            TEXT NOT NULL,
    -- The party's members at the time: [{ "account_id": ..., "class": ..., "role": ... }].
    party            JSONB NOT NULL DEFAULT '[]'::jsonb,
    total_damage     BIGINT NOT NULL CHECK (total_damage >= 0),
    dps              REAL NOT NULL CHECK (dps >= 0),
    duration_seconds REAL NOT NULL CHECK (duration_seconds >= 0),
    -- Seconds from the first pull of the boss to its death; NULL when unknown.
    boss_kill_seconds REAL,
    -- Per ability: [{ "ability": ..., "damage": ..., "casts": ... }].
    abilities        JSONB NOT NULL DEFAULT '[]'::jsonb,
    completed_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX dungeon_damage_reports_dungeon_key_idx
    ON game.dungeon_damage_reports (dungeon_id, key_level);
CREATE INDEX dungeon_damage_reports_dps_idx
    ON game.dungeon_damage_reports (dps DESC);
