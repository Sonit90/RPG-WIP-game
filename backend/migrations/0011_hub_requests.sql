-- Request ids for hub transactions (docs/P2_DESIGN.md 2.1).
--
-- Every progression-changing hub route (enhance, salvage, exchange, equipment move, taking
-- mail, key downgrade and cleanse, talents) may carry a `request_id`: the client's UUID for one
-- user action, reused if it retries. The first request records its response here in the same
-- transaction that changes the character (which holds the character row's lock); a replay
-- finds the row, returns the stored response and changes nothing - a double submit is one
-- attempt, one debit.
--
-- Scoped by account: a request id seen from another account (or for another character or
-- kind) is a 409, and the stored response is never returned to it.
CREATE TABLE game.hub_requests (
    request_id   UUID PRIMARY KEY,
    account_id   UUID NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE,
    character_id UUID NOT NULL REFERENCES game.characters(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    response     JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX hub_requests_character_idx ON game.hub_requests (character_id, created_at);
