# Authentication

The account system is the existing JWT service from `rust-generic-auth-handling`, copied into
`backend/` and extended so the **same access token** authenticates a player to the realm API
*and* to SpacetimeDB.

## Why the original token could not be used as-is

SpacetimeDB accepts third-party OIDC tokens: it reads `iss`, fetches
`{iss}/.well-known/openid-configuration`, follows `jwks_uri`, verifies the signature against
the published key, and derives the player's **Identity** from `(iss, sub)`. The original
service signed HS256 with a shared secret, had no `aud`, no `kid` and a non-URL issuer, and
published no keys — so it could not be verified by anyone without the secret.

## What changed

| | Before | Now |
| --- | --- | --- |
| Algorithm | HS256, shared secret | **ES256** (P-256), private key only in the API |
| Issuer | `generic-auth` | the API's public URL (`http.public_url`) |
| Audience | none | `mygame-realm`, checked by API and realm |
| Key id | none | `kid` in every header, published in the JWKS |
| Discovery | none | `GET /.well-known/openid-configuration`, `GET /.well-known/jwks.json` |
| Display name | — | `preferred_username` claim (the realm shows it to other players) |
| Refresh | rotation only | rotation **+ reuse detection** (a spent refresh token revokes every session) and an `is_active` check |
| Throttling | none on login | per address and per email on login, per address on register and refresh |

The HMAC secret (`APP__AUTH__JWT_SECRET`) remains, but only for email verification codes.

## Flow

```text
client ── POST /api/v1/auth/login {email, password} ──► API ── Argon2 verify ──► TokenPair
          (access: ES256 JWT, 30 min · refresh: opaque, 14 days, rotating, stored hashed)
client ── WSS connect, Authorization: Bearer <access> ──► SpacetimeDB
          SpacetimeDB ── GET {iss}/.well-known/openid-configuration ──► API
          SpacetimeDB ── GET {iss}/.well-known/jwks.json ──► API      (cached)
          verifies signature ─► Identity = f(iss, sub) ─► client_connected
          realm checks iss == configured issuer, aud contains "mygame-realm"
client ── (every ~28 min) POST /api/v1/auth/refresh ──► new pair; reconnects use the new token
```

## Identity

* **Account id** = the token's `sub` = the user's UUID in `auth.users`.
* **SpacetimeDB Identity** = a hash of `(iss, sub)`. The same account always has the same
  Identity — on another machine, after a reconnect, with two connections open — and a
  connection id is never mistaken for an account. The realm keys accounts, party members,
  tickets and instance players by Identity and keeps a separate `connection` table.
* **The bridge** signs its own short-lived token with `sub = service:realm-bridge`. The realm
  computes the bridge's Identity from the configured issuer and that subject; `svc_*`
  reducers accept that Identity and nobody else. `JwtCodec::verify` refuses service tokens on
  player endpoints.

## Client session

* The login screen (email, password, remember me, create account) runs on the network thread;
  the game thread never waits for it.
* The password is sent once over TLS, never stored, never logged; the field is cleared after
  every attempt.
* The access token lives in memory only. With **remember me**, the refresh token is sealed
  with **DPAPI** for the current Windows user (`%APPDATA%\mygame\session[-profile].bin`,
  `engine-platform::secure_store`) — unreadable by another user, another machine or a copy of
  the file.
* Logout revokes the refresh token on the server and deletes the local file.
* Token-bearing types redact themselves in `Debug` output (`TokenPair`, `StoredSession`,
  `TextField` with a mask), so a log line cannot leak a credential.

## Endpoints

| Route | Notes |
| --- | --- |
| `POST /api/v1/auth/register` | 201 + `TokenPair`; throttled per address |
| `POST /api/v1/auth/login` | throttled per address and per email |
| `POST /api/v1/auth/refresh` | rotates; reuse revokes all sessions |
| `POST /api/v1/auth/logout` | revokes the refresh token |
| `GET /api/v1/users/me` | profile |
| `GET /.well-known/openid-configuration` · `GET /.well-known/jwks.json` | public by design |
| verify-email, resend, forgot/reset/change password, Google OAuth | unchanged from the original service |

## Keys

Development creates `backend/data/jwt_es256.pem` on first start. A deployment generates one
key and keeps it (see [SPACETIMEDB_DEPLOYMENT.md](SPACETIMEDB_DEPLOYMENT.md)); rotating it
means publishing the new key under a new `kid` and waiting out the access-token lifetime.

## Tests

`backend/crates/auth/src/jwt.rs`: round trip with issuer/audience/kid, service tokens refused
as players, tokens from another key refused, tampered payloads refused, expired tokens
refused, JWKS verifies issued tokens. `realm-bot scenario security`: a forged token is
rejected by both the API and the realm.
