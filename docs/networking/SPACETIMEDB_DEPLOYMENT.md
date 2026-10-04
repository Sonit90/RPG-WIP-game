# Deploying the realm (self-hosted, Ubuntu + Docker)

One VPS runs everything behind Caddy:

```text
Internet ──443──► Caddy ──► api:8080          realm API + OIDC issuer   (API_DOMAIN)
                       └──► spacetimedb:3000  SpacetimeDB websocket/HTTP (REALM_DOMAIN)
                 api ──► postgres:5432 (internal only)
                 api ──► spacetimedb:3000 (internal: svc_* bridge calls)
                 spacetimedb ──► https://API_DOMAIN/.well-known/* (JWKS fetch)
```

Files: `deploy/docker-compose.yml`, `deploy/Caddyfile`, `deploy/api.Dockerfile`,
`deploy/.env.production.example`, `deploy/realm-client.example.json`.

## Environments

| | Development | Staging | Production |
| --- | --- | --- | --- |
| How | `tools/realm-dev.ps1 up` (native SpacetimeDB, Docker Postgres, API via cargo) | the production compose on a second VPS or a second compose project | `deploy/docker-compose.yml` |
| API / issuer | `http://127.0.0.1:8080` | `https://api.staging.example.com` | `https://api.example.com` |
| SpacetimeDB | `http://127.0.0.1:3000`, database `realm` | `https://realm.staging.example.com`, `realm` | `https://realm.example.com`, `realm` |
| Signing key | generated into `backend/data/` | its own key | its own key |

The client reads four addresses — `SPACETIMEDB_URL`, `SPACETIMEDB_DATABASE`, `AUTH_API_URL`,
`PERSISTENCE_API_URL` — from the environment or `--realm-config <file.json>`
(`deploy/realm-client.example.json`). Nothing production is compiled in. Plain `http://` to a
remote host is logged as a warning: credentials would cross the network unencrypted.

## 1. Server preparation

```bash
# Ubuntu 22.04/24.04
sudo apt update && sudo apt install -y ca-certificates curl git ufw
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER            # log out and back in
sudo ufw allow OpenSSH && sudo ufw allow 80,443/tcp && sudo ufw allow 443/udp && sudo ufw enable
```

DNS: A/AAAA records for `API_DOMAIN` and `REALM_DOMAIN` pointing at the VPS. Caddy obtains
certificates on first start.

## 2. Configuration

```bash
git clone <repo> mygame && cd mygame
cp deploy/.env.production.example deploy/.env      # fill in domains and secrets
openssl rand -base64 48                            # for POSTGRES_PASSWORD and EMAIL_CODE_SECRET
openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt \
  -out deploy/secrets/jwt_es256.pem
chmod 600 deploy/.env deploy/secrets/jwt_es256.pem
```

The signing key is the one secret that matters most: anyone holding it can mint tokens the
realm accepts. Back it up offline; never put it in the image or the repository
(`deploy/secrets/` is git-ignored).

## 3. Start

```bash
docker compose -f deploy/docker-compose.yml --env-file deploy/.env up -d --build
docker compose -f deploy/docker-compose.yml logs -f api
curl https://API_DOMAIN/health
curl https://API_DOMAIN/.well-known/openid-configuration
```

Migrations run on API start (`APP__DB__RUN_MIGRATIONS_ON_START=true` in the compose file). To
run them as a separate deploy step instead, set it to false and run
`sqlx migrate run --source backend/migrations` against the database first.

## 4. Publish the realm module

From a machine with the `spacetime` CLI and the repository:

```bash
rustup target add wasm32-unknown-unknown
cd server/realm-module && cargo build --target wasm32-unknown-unknown --release && cd ../..
spacetime server add prod --url https://REALM_DOMAIN
spacetime publish realm --server prod \
  --bin-path server/realm-module/target/wasm32-unknown-unknown/release/realm_module.wasm --yes
# The identity that publishes is the module's owner: keep that CLI login (or token) safe.
spacetime call --server prod realm admin_configure \
  '"https://API_DOMAIN"' '"mygame-realm"' '"service:realm-bridge"'
```

`admin_configure` must name exactly the issuer the API puts in tokens (`APP__HTTP__PUBLIC_URL`,
no trailing slash). Until it is called the realm accepts the development issuer only.

Tuning on a live realm (owner only):

```bash
spacetime call --server prod realm admin_set_matchmaking 0.10 0.20 30 60
spacetime call --server prod realm admin_set_timeouts 30 300 60
```

## 5. Client

Ship `realm.json` beside `mygame.exe` (from `deploy/realm-client.example.json`) and start with
`mygame --online --realm-config realm.json`, or set the four environment variables.

## Module versioning and migrations

* `spacetime publish` to an existing database performs a hot swap; SpacetimeDB checks the new
  schema against the old and refuses breaking changes (removed/re-typed columns). Additive
  changes — new tables, new reducers, new columns appended with `#[default(...)]` — publish in
  place, and connected clients keep their sessions.
* A breaking change needs a planned migration: add the new shape alongside the old, move data
  with an owner-only reducer, ship clients that read the new shape, then remove the old one in
  a later publish. Regenerate client bindings (`tools/realm-dev.ps1 publish` does it locally)
  whenever the module's public surface changes, and ship that client with the module.
* Postgres migrations are numbered SQL files in `backend/migrations/`, applied once by sqlx in
  order. Never edit one that has shipped; add the next number.
* Tag each release (`git tag realm-v<n>`) on the commit whose wasm and API image you deploy.

## Deploy and rollback

```bash
git pull && git checkout realm-v<n>
docker compose -f deploy/docker-compose.yml --env-file deploy/.env up -d --build api
spacetime publish realm --server prod --bin-path …/realm_module.wasm --yes
```

Rollback: check out the previous tag, rebuild/restart the API, republish the previous wasm.
A module rollback is only clean if the schema did not change incompatibly in between — which
is the reason breaking changes go through the two-step migration above. A Postgres migration
is rolled back by restoring the pre-deploy backup (take one before every deploy that adds a
migration).

## Backups

| What | How | Restore |
| --- | --- | --- |
| Postgres (accounts, characters, settings, results) | `docker compose exec -T postgres pg_dump -U realm -Fc realm > backup-$(date +%F).dump` nightly (cron), copied off the host | `pg_restore -U realm -d realm --clean backup.dump` |
| SpacetimeDB (realtime state, outbox) | stop the container, archive the `spacetimedb-data` volume (`docker run --rm -v mygame-realm_spacetimedb-data:/d -v $PWD:/b alpine tar czf /b/stdb.tgz -C /d .`), start it | restore the volume, start |
| Signing key, `.env` | offline copy | copy back |

The realm's state is mostly transient (queues and runs), so losing it costs the runs in
progress; results not yet collected are in its outbox, which is why the bridge collects every
five seconds. Postgres is the backup that matters.

## Logging and monitoring

* Every service logs to stdout; `docker compose logs -f <service>`. Configure Docker's log
  rotation (`/etc/docker/daemon.json`: `"log-opts": {"max-size": "50m", "max-file": "5"}`).
* The module logs structured `event=… key=value` lines (`spacetime logs realm --server prod -f`):
  `realm_initialized`, `connected`/`disconnected`, `queue_join`/`queue_leave`, `match_formed`,
  `ready_check_response`, `ready_check_cancelled`, `party_created`, `leader_transfer`,
  `party_kick`, `member_departed`, `party_disbanded`, `instance_created`, `instance_state`,
  `instance_start`, `player_disconnected`/`player_reconnected`, `instance_completion`,
  `instance_cleanup`, `result_persisted`, `rate_limited`, `auth_rejected`,
  `service_call_refused`, `admin_call_refused` — with account, character, party, instance and
  dungeon ids. No token or password is ever logged.
* The API logs with `tracing` (`RUST_LOG`): sign-ins, throttling, refresh-token reuse, queue
  relays, applied results, persistence failures.
* Watch: `/health` (uptime check), `realm bridge: cannot collect results` (the API cannot reach
  the realm), `persistence failure`, `refresh token reuse detected`, `rate limited` bursts,
  disk use of the two volumes.

## Security checklist

* TLS everywhere public (Caddy); Postgres not published; SpacetimeDB reachable only through
  Caddy.
* The signing key and `.env` are mode 600 and backed up offline.
* The module owner credential (the CLI identity that published) is the realm's root: keep it
  off the VPS.
* `APP_ENV=production` disables Swagger UI, permissive CORS and the development rate-limit
  scaling.
