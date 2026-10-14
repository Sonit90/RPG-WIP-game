# Operator dashboard

A read-only web dashboard for developers and operators, served by the realm API. Not for players.

## Access

* Open `https://API_DOMAIN/admin` (dev: `http://127.0.0.1:8080/admin`). The page is static and
  public; every data call carries your token.
* Sign in with email and password (the normal login endpoint), or paste an access token. The
  token lives in `sessionStorage` only.
* The account needs the `ops.view` permission. Migration `0007_ops_permission.sql` grants it to
  the `admin` role. For a single user, insert a `user_permissions` row for `ops.view`.
  `users.manage` alone is not enough.
* Create an admin: `generic-auth-api create-admin --email ... --password ...`.

## Endpoints (`/api/v1/admin/ops/*`, bearer token, `ops.view`)

| Route | What |
| --- | --- |
| `GET /ops/status` | API version/uptime, DB ping, realm reachability and snapshot latency, process and system CPU/RAM (sysinfo), result-bridge state (last success, pending results/loot, failures), last 50 errors |
| `GET /ops/realm` | Realm snapshot: online accounts, queue per dungeon/role, parties by state, instances with players, monsters, kill progress, fight-clock health. Cached for 1 s |
| `GET /ops/instances/{id}` | One instance, with worn items resolved to names/stats and talents read from Postgres |
| `GET /ops/anomalies` | Fastest/slowest completed run per dungeon and key level, runs that are too fast, damage outliers |

Errors: 401 no/invalid token, 403 missing `ops.view`, 404 unknown instance, 503 realm unreachable.

## How it works

The API calls the realm procedure `svc_admin_snapshot` (service token, `require_service`) on
demand; nothing runs in the realm while nobody looks. The document is built by
`engine_realm::ops::build_snapshot` (a pure function) and shared by both ends as
`AdminSnapshot`. The instance `modifiers` field is always empty until key modifiers exist.
"Ticks behind" is how many 60 Hz fight ticks are due but not run.

Recent errors: a tracing layer copies ERROR events into a 50-entry ring buffer (repeats are
folded); the bridge adds failed collection and acknowledgement passes. In memory only.

## Anomaly thresholds (`backend/config/default.toml`, `[ops.anomaly]`)

Override with `APP__OPS__ANOMALY__<KEY>`.

| Key | Meaning | Default |
| --- | --- | --- |
| `min_completion_seconds` | completed run faster than this is flagged (0 = off) | 60 |
| `relative_fast_percent` | ...or faster than this % of the dungeon's median | 50 |
| `relative_min_samples` | completed runs a dungeon needs for the relative check | 5 |
| `max_dps` | DPS above this is flagged | 5000 |
| `max_total_damage` | one player's damage in a run above this is flagged | 5,000,000 |
| `min_boss_kill_seconds` | boss killed faster than this is flagged | 10 |
| `list_limit` | rows per list | 20 |

Damage checks read `game.dungeon_damage_reports` (migration `0006`). Nothing writes it yet
(the realm does not report damage), so that panel is empty until a writer lands.

## Deploying

Publish the module (new procedure; additive) and regenerate bindings
(`tools/realm-dev.ps1 publish`), then deploy the API. Migrations run on API start.
