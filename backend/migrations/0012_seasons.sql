-- Seasons (docs/P2_DESIGN.md 2.3): the periods leaderboards and history are cut by.
--
-- `season-0` is "all time": it started before anything was recorded and never ends, so the
-- first leaderboards have a season to belong to. `ends_at` NULL means open-ended; the active
-- season is the latest one that has started and not ended (`store::active_season`).
CREATE TABLE game.seasons (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    starts_at TIMESTAMPTZ NOT NULL,
    ends_at   TIMESTAMPTZ,
    CHECK (ends_at IS NULL OR ends_at > starts_at)
);

INSERT INTO game.seasons (id, name, starts_at, ends_at)
VALUES ('season-0', 'All time', '2026-01-01T00:00:00Z', NULL);
