-- Community maps (docs/community-maps.md, docs/P2_DESIGN.md 2.10).
--
-- A map is data: the validated JSON of `engine_realm::mapcheck`, never code. `community_maps` is
-- the listing row, `community_map_versions` the immutable versions (an update is a new version;
-- a published version is never edited, so `cm:<id>:<version>` always names the same level).
-- The validator runs on every create, update and publish; `report` and `valid` record what it
-- said, and publishing re-runs it rather than trusting them.
--
-- Migration 0014 is reserved for the books branch; this one does not depend on it.

-- Moderation: hide and reject. Held by the admin role (grant it to one user with a
-- `user_permissions` row to make a moderator who is not an admin).
INSERT INTO auth.permissions (name, description) VALUES
    ('community.moderate', 'Hide and reject community maps, list maps in any status');

INSERT INTO auth.role_permissions (role_id, permission_id)
SELECT r.id, p.id FROM auth.roles r, auth.permissions p
WHERE r.name = 'admin' AND p.name = 'community.moderate';

CREATE TABLE game.community_maps (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    author_account  UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    -- Listing fields come from the latest version's `meta`, made safe to store even when that
    -- version does not validate.
    title           TEXT NOT NULL CHECK (char_length(title) BETWEEN 3 AND 48),
    description     TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 1000),
    -- The author's claimed difficulty (1 easy .. 5 brutal). What a run is played at is chosen
    -- at queue time (1..=10) and is not stored here.
    difficulty      SMALLINT NOT NULL DEFAULT 1 CHECK (difficulty BETWEEN 1 AND 5),
    tags            TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(tags) <= 5),
    status          TEXT NOT NULL DEFAULT 'draft'
                    CHECK (status IN ('draft', 'published', 'hidden', 'rejected')),
    current_version INT NOT NULL DEFAULT 1 CHECK (current_version >= 1),
    -- A moderator's last note (hide / reject / restore).
    moderation_note TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Runs started on the map: one per run (not per player), counted when the run row is first
    -- written, so a redelivered result cannot count twice.
    plays           BIGINT NOT NULL DEFAULT 0 CHECK (plays >= 0)
);

CREATE TABLE game.community_map_versions (
    map_id     UUID NOT NULL REFERENCES game.community_maps(id) ON DELETE CASCADE,
    version    INT  NOT NULL CHECK (version >= 1),
    -- The `{format, meta, scene}` file as uploaded (JSONB: key order is not preserved).
    data       JSONB NOT NULL,
    -- `engine_realm::community::ReportInfo`.
    report     JSONB NOT NULL,
    valid      BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (map_id, version)
);

-- Listing and sorting: the public list only ever reads published maps, so the indexes are
-- partial. Newest, most played and easiest/hardest; the author's own list; and the speed of a
-- map's best run (the "fastest" sort reads dungeon_runs).
CREATE INDEX community_maps_newest_idx
    ON game.community_maps (created_at DESC) WHERE status = 'published';
CREATE INDEX community_maps_plays_idx
    ON game.community_maps (plays DESC, created_at DESC) WHERE status = 'published';
CREATE INDEX community_maps_difficulty_idx
    ON game.community_maps (difficulty, created_at DESC) WHERE status = 'published';
CREATE INDEX community_maps_author_idx
    ON game.community_maps (author_account, updated_at DESC);
CREATE INDEX community_maps_status_idx
    ON game.community_maps (status, updated_at DESC);
CREATE INDEX dungeon_runs_speed_idx
    ON game.dungeon_runs (dungeon_id, duration_us) WHERE outcome IN ('timed', 'overtime');

-- Request ids for community writes (the account-scoped twin of `game.hub_requests`, which is
-- keyed by character). A retried create, update, publish, unpublish or moderation call returns
-- the first answer and changes nothing; a request id seen from another account or for another
-- action is a 409 and its stored response is never shown.
CREATE TABLE game.community_requests (
    request_id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    response   JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX community_requests_account_idx ON game.community_requests (account_id, created_at);
