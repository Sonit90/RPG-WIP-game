-- Switch email verification from long UUID tokens to short 6-digit codes.
--
-- The `email_verification_tokens` table is repurposed: `token_hash` now holds
-- the HMAC-SHA256 of a 6-digit code (keyed with the server secret) instead of
-- a 256-bit token hash.
--
--  * 6-digit codes are NOT unique across users, so the UNIQUE constraint on
--    `token_hash` is dropped; lookups are by `user_id` instead.
--  * `attempt_count` caps online brute-force guesses against a single code.
--  * the (user_id, created_at) index backs resend rate-limit queries and the
--    "latest active code" lookup.

ALTER TABLE auth.email_verification_tokens
    ADD COLUMN attempt_count INT NOT NULL DEFAULT 0;

-- Drop the UNIQUE constraint on token_hash (auto-named <table>_token_hash_key).
ALTER TABLE auth.email_verification_tokens
    DROP CONSTRAINT email_verification_tokens_token_hash_key;

CREATE INDEX email_verification_tokens_user_created_idx
    ON auth.email_verification_tokens(user_id, created_at);
