-- Run history and leaderboards (docs/P2_DESIGN.md 2.3).
--
-- `dungeon_runs` is one row per run, written by `store::apply_result` from whichever of the run's
-- results arrives first (every result of a run carries the party; a member who left the run
-- before it ended carries none and never writes it). `ON CONFLICT (run_uid) DO NOTHING` makes
-- the other players' results no-ops here. The leaderboards are a query over this table and
-- nothing else: the realm is never asked.
--
-- `outcome` is how the realm classified the run (timed / overtime / failed / abandoned,
-- `engine_realm::outcome`). Only 'timed' and 'overtime' rows are ever ranked.
CREATE TABLE game.dungeon_runs (
    run_uid     TEXT PRIMARY KEY,
    season_id   TEXT NOT NULL REFERENCES game.seasons(id),
    dungeon_id  TEXT NOT NULL,
    key_level   INT  NOT NULL CHECK (key_level >= 0),
    -- [{ "id": ..., "magnitude_pct": ... }], as the key had them during the run.
    modifiers   JSONB NOT NULL DEFAULT '[]'::jsonb,
    outcome     TEXT NOT NULL CHECK (outcome IN ('timed', 'overtime', 'failed', 'abandoned')),
    duration_us BIGINT NOT NULL CHECK (duration_us >= 0),
    -- The dungeon's timer for the run; 0 when it had none.
    timer_us    BIGINT NOT NULL DEFAULT 0 CHECK (timer_us >= 0),
    -- [{ "character_id": ..., "name": ..., "hero": ..., "role": ... }]
    party       JSONB NOT NULL DEFAULT '[]'::jsonb,
    finished_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Serves both boards: a dungeon in a season, by key and speed.
CREATE INDEX dungeon_runs_board_idx
    ON game.dungeon_runs (season_id, dungeon_id, key_level DESC, duration_us);

-- What each player's result says about their own part, next to the experience.
-- `run_uid` is NULL for a result recorded before runs were; `outcome` is the player's own (a
-- member who left a run that went on is 'abandoned' while the run itself may have been timed).
ALTER TABLE game.dungeon_results
    ADD COLUMN run_uid      TEXT,
    ADD COLUMN outcome      TEXT CHECK (outcome IN ('timed', 'overtime', 'failed', 'abandoned')),
    ADD COLUMN deaths       INT    NOT NULL DEFAULT 0 CHECK (deaths >= 0),
    ADD COLUMN damage_taken BIGINT NOT NULL DEFAULT 0 CHECK (damage_taken >= 0),
    ADD COLUMN healing      BIGINT NOT NULL DEFAULT 0 CHECK (healing >= 0),
    ADD COLUMN dps          REAL   NOT NULL DEFAULT 0,
    -- What the character received in the run, filled as the loot is delivered:
    -- [{ "item": ..., "quality": ..., "sharpening": ..., "broken": ... }]
    ADD COLUMN loot         JSONB  NOT NULL DEFAULT '[]'::jsonb;

-- A character's history, newest first.
CREATE INDEX dungeon_results_history_idx
    ON game.dungeon_results (character_id, applied_at DESC);
-- "A run I took part in": the lookup behind GET /runs/{uid}.
CREATE INDEX dungeon_results_run_idx
    ON game.dungeon_results (run_uid) WHERE run_uid IS NOT NULL;

-- Delivered items keep the properties they were rolled with, so a result's loot can be listed
-- whichever of the result and the delivery is applied first.
ALTER TABLE game.loot_deliveries
    ADD COLUMN quality    INT     NOT NULL DEFAULT 100,
    ADD COLUMN sharpening INT     NOT NULL DEFAULT 0,
    ADD COLUMN broken     BOOLEAN NOT NULL DEFAULT false;
