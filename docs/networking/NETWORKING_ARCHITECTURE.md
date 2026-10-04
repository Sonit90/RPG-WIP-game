# Networking architecture

The online game is an authoritative client/server MMO: the **realm** (a SpacetimeDB module)
owns everything realtime, the **realm API** (an axum service over Postgres) owns everything
long-lived, and the **game client** owns nothing but presentation and the player's intent.

```text
                      ┌───────────────────────────────┐
                      │          Game client          │  engine-app (online.rs)
                      │  front-end · prediction · draw│  engine-net (network thread)
                      └───────┬───────────────┬───────┘
                 HTTPS (JSON) │               │ WSS (SpacetimeDB protocol, OIDC token)
                              ▼               ▼
        ┌──────────────────────────┐   ┌──────────────────────────────┐
        │        Realm API          │   │     Realm (SpacetimeDB)       │
        │ backend/ (axum + sqlx)    │──▶│ server/realm-module (wasm32)  │
        │ auth · OIDC/JWKS · chars  │   │ queue · parties · instances   │
        │ settings · queue relay    │◀──│ movement · barrier · outbox   │
        │ result bridge             │   └──────────────────────────────┘
        └────────────┬─────────────┘    svc_* calls, initiated by the API
                     ▼
              Postgres: auth.* (accounts, sessions) · game.* (characters, settings, results)
```

The realm never calls out: SpacetimeDB refuses module HTTP to private addresses, and a realm
that holds no secret and opens no connection is a smaller attack surface anyway. Every
API ↔ realm exchange is started by the API, authenticated with a service token
(`sub = service:realm-bridge`) that the realm recognises by the identity SpacetimeDB derives
from it.

## Crates and where they run

| Crate | Runs in | Owns |
| --- | --- | --- |
| `engine-realm` | realm (wasm32), client, API | The rules: catalogs, loadouts and gear score, matchmaking, party and ready-check state machines, instance lifecycle, the movement step and input budget, rate limits, progression, talent keys, API bodies. Pure; no I/O; 64 unit tests. |
| `server/realm-module` | SpacetimeDB | Tables, reducers, procedures, views, scheduled ticks. A transactional shell around `engine-realm`. |
| `engine-net` | client | The network thread: realm API client, SpacetimeDB link, reconnect, clock sync, plain-data mirror, prediction, interpolation. No window, no renderer. |
| `backend/` | API server | Auth (copied from `rust-generic-auth-handling`), ES256 OIDC tokens, characters, settings, queue relay, result bridge. |
| `engine-presentation::ui::frontend` | client | The online screens. Reads a plain `FrontEndView`; knows nothing about networking. |
| `engine-app::online` | client | The one branch point between offline and online: drives the session, mirrors the instance into the local simulation. |
| `engine-realm-testclient` | dev machine | `realm-bot`: scripted clients and the end-to-end/security scenarios. |

Dependency rules from `docs/architecture.md` still hold: `engine-game`, `engine-collision`,
`engine-environment`, `engine-core` gained no network dependency; `engine-net` has no
`windows`, renderer or UI; the renderer never learns anything is remote.

## Data ownership

| Data | Source of truth | Notes |
| --- | --- | --- |
| Accounts, credentials, sessions, refresh tokens | API (`auth` schema) | |
| Characters, level, experience, equipment, bags, talents | API (`game.characters`) | Loadout stored as stable item/slot keys |
| Gear score, derived stats | Computed by API from the loadout | Never stored, never accepted from a client |
| Dungeon completions, applied results | API (`game.*`) | Results keyed by realm idempotency key |
| Account settings (keys, audio, camera, UI, nameplates, bars) | API (`game.account_settings`) | Versioned |
| Machine settings (display, resolution, graphics) | Client `settings.cfg` | Never uploaded |
| Queue, parties, ready checks, leaders | Realm | |
| Instances, runtime positions, HP, barrier, countdown | Realm | |
| Monster runtime state | Realm | Static until server combat (Phase 9) |
| Pending results (outbox) | Realm until acknowledged | Then API |

A character enters the realm as a **snapshot** taken by the API at queue time (id, name,
definition, level, gear score). The realm never writes it back; it writes *results*, which
the API applies with its own rules.

## Authority

Clients send **intent**, never state:

| A client may send | The server decides |
| --- | --- |
| "queue character C as role R" (via the API) | ownership, role legality, level/gear admission, gear score |
| "accept / decline the ready check" | outcome, re-queue, leader (random, server-side) |
| "start", "kick X", "make X leader", "leave" | authorization via `engine_realm::party::authorize` |
| input frames: axes, camera yaw, jump, sequence number | position, velocity, collision, barrier, speed |
| "what time is it" | the server clock |

There is no message that carries a position, a health value, a gear score, a reward or a
completion from a client. `realm-bot scenario security` attempts each of those and checks
the server refuses.

## Realtime state flow

```text
reducer / scheduled tick ─► private tables ─► caller-scoped views (my_*) ─► subscription
     ─► engine-net callbacks (network thread) ─► RealmUpdate ─► channel ─► RealmMirror
     ─► FrontEndView / OnlineSession::sync_world (game thread) ─► Simulation, extractor, UI
```

* **Subscriptions** are to views only (`my_account`, `my_ticket`, `my_party`,
  `my_party_members`, `my_instance`, `my_instance_players`, `my_instance_monsters`,
  `my_notices`, `queue_population`). Each view starts from `ctx.sender()`, so a client
  receives its own slice and nothing else; there is no argument to point it elsewhere. All
  tables are private.
* **The game thread never blocks.** Commands go into a channel; events are drained with
  `try_iter` once a frame.

## Movement

```text
client tick (60 Hz)  intent ─► Simulation (predicted) ─► InputFrame{seq} ─► outbox (~30 Hz send)
realm move_input     drop stale/malformed ─► InputBudget (60/s of server time, burst 12)
                     ─► engine_realm::movement::apply_input × N ─► row {pos, vel, last_seq}
client               row ─► Prediction::reconcile (rewind, replay unacked) ─► body
others               rows ─► SnapshotBuffer ─► drawn 100 ms behind, interpolated
```

Server and client integrate with the same function (`engine_game::movement::step_collided`
via `apply_input`) against the same collision (`LevelGeometry::build`, the same
`engine_game::arena` builders the offline game uses, plus barrier boxes). A speed hack —
oversized axes or frames faster than real time — is clamped and budgeted; the security
scenario floods 16-frame messages with axis 50 and checks the distance stays within real
time.

## Offline mode

`mygame` without flags is the offline game, unchanged. `--network-mode online` (or
`--online`) creates an `OnlineSession`; everything else in the app branches on one
`Option`. See [RECONNECTION.md](RECONNECTION.md), [MATCHMAKING.md](MATCHMAKING.md),
[PARTY_SYSTEM.md](PARTY_SYSTEM.md), [DUNGEON_INSTANCES.md](DUNGEON_INSTANCES.md),
[AUTHENTICATION.md](AUTHENTICATION.md), [PERSISTENCE.md](PERSISTENCE.md),
[SPACETIMEDB_DEPLOYMENT.md](SPACETIMEDB_DEPLOYMENT.md).

## Development workflow

```powershell
.\tools\realm-dev.ps1 up          # Postgres, SpacetimeDB, module publish + bindings, API
.\tools\realm-it.ps1              # every end-to-end scenario
.\tools\run-3-clients.ps1 -Autoplay   # tank, healer, dps clients that play to the start
cargo run --release --bin mygame -- --online --profile me
cargo run --bin realm-bot -- bot --character druid --role healer --walk --lead
```

`--profile` keeps each local client's remembered session apart. `--dev-login` and
`--dev-autoplay` exist for unattended local testing and are refused against a remote realm.

## Testing

| Layer | Where | What |
| --- | --- | --- |
| Rules | `cargo test -p engine-realm` | matchmaking (1/1/1, ±10 %, widening, 20 % cap, multi-dungeon, determinism…), party FSM and authorization, ready check, lifecycle, movement budget, gear score, catalog |
| Client net | `cargo test -p engine-net` | prediction/reconciliation, interpolation, clock, config, mirror, back-off |
| UI | `cargo test -p engine-presentation frontend` | click-what-was-drawn, leader-only controls, roles, every screen draws |
| API | `cd backend && cargo test` | ES256/JWKS, forged/expired/tampered tokens, service tokens, rate limits, result decoding |
| End to end | `tools/realm-it.ps1` | slice, isolation, decline, reconnect, security, results |
