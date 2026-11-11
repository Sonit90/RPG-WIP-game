# P2 final report — Endgame Depth, Content Expansion & Community Systems

Branch `p2-core` (off `p1-core`). Design and audit: `docs/P2_DESIGN.md`. End-to-end verification
(brief §46, all 30 steps): `docs/P2_VERIFICATION.md`. Running handoff log: `WORK_STATE.md`.

## Implemented

| Area | What exists | Where |
|---|---|---|
| Phase-1 hardening | Instance `uid` (u128) → collision-free run/loot ids and a per-fight seed hidden from clients; request-id idempotency on every hub write (`game.hub_requests`, `store::HubTx`: lock row → replay lookup → `ensure_unlocked` → write); strict `apply_result`; versioned loadout with `migrate` (now v3) | `engine-realm/src/instance`, `backend/crates/game/src/store.rs`, `engine-realm/src/loadout.rs` |
| Eternal keys | Keys 1–99; odd levels add modifiers, even levels add HP/damage, closed-form `ModifierCatalog::row_for`; new modifiers Bolstering, Raging, Necrotic Pools, Spiteful Casters | `assets/catalog/modifiers.json`, `engine-realm/src/modifiers.rs`, `engine-game/src/sim/modifier_exec.rs` |
| Timers & outcomes | Per-dungeon `timer_seconds`, clock from barrier lift; outcomes timed / overtime / failed / abandoned; HUD timer with 60 s warning | `engine-realm/src/outcome.rs`, `ui/run_timer.rs` |
| History & leaderboards | `game.dungeon_runs` + per-player result stats; History and Boards screens; boards from `leaderboards.json` (`ScoreKind`), incl. community boards and completions | `backend/crates/game/src/history.rs`, `engine-realm/src/leaderboard.rs` |
| Dungeons as data | Scene registry generated from `dungeons.json`; 5 production dungeons: Crypt, Ashen Reliquary, Drowned Catacombs, Bone Foundry, Sanctum of Ash (each with generator test, mechanic tests, navigator walkability, realm-path clear test, boss loot table) | `assets/scenes/*.json`, `engine-game/tests/export_*.rs`, `docs/dungeon/*.md` |
| New enemy primitives | Patrols, Hymn, Undertow, Bestow, Detonate, Soak, Conveyor, Reflect, `EnemyTarget::Weakest`, `on_death`, `once_per_phase`, phase summons, boss shields; statuses Waterlogged, Warded, Stoked, Mirrored, Ashen, Branded, Shielded (appended; wire count 40) | `engine-game/src/enemy`, `sim/enemy_exec.rs`, `status.rs` |
| Sharpening (replaces tiers) | No C/B/A/S tiers or fusion anywhere. `(quality + sharpening)/100`, one formula; pre-sharpened drops on data-driven strictly-decreasing curves with key shift, tails monotone in key | `loot.json` `sharpening_drops`, `engine-game/src/sharpen_drops.rs`, `engine-realm/src/sharpen.rs` |
| Sets | 3 major 5-piece (one per class) + 3 minor 2-piece; stats + `damage_pct` / `cooldown_pct` / `resource_gen_pct` effects through the existing ability/talent path; cached per player | `assets/catalog/sets.json`, `engine-game/src/character/sets.rs`, `engine-realm/src/sets.rs` |
| Gems | 7 kinds × 5 levels, sockets on every epic (6 with two), socket / unsocket / upgrade routes (server OsRng, keep-one on fail) | `assets/catalog/gems.json`, `engine-realm/src/gems.rs` |
| Books | 3 talent books (+1 point) and 3 enhancement books × 3 options per class, as hidden talent nodes (one progression system); use / set-active routes; Books page | `talents.json`, `engine-game/src/talent`, `engine-realm/src/books.rs` |
| Reforge (gamble) | Pay gold to re-roll base quality from published bands (EV 71.5, 5 % ≥ 100); odds, EV and cost shown before confirm; atomic and idempotent | `economy.json` `reforge`, `engine-realm/src/reforge.rs` |
| Community maps | Format + validator with stable issue codes; Postgres maps/versions; API create/update/validate/publish/unpublish/list/get + moderation (hide/reject/restore, `community.moderate`); realm `community_map` table and `cm:<id>:<ver>` instances; no loot/currency/key progress (4 server layers); client browser + upload; community boards; optional timer | `engine-realm/src/mapcheck.rs`, `engine-realm/src/community.rs`, `backend/.../community*`, `ui/frontend/community.rs` |
| Editor authoring | Dungeon / Listing / Validate tabs, debounced validation with the server's own check, issue → selection, approved-library warning, export refusing on errors, starter template | `crates/engine-editor`, `assets/templates/community_blank.json` |
| Seasons | `game.seasons` data model, season 0 active | migration 0012 |
| Simulations | Gear tiers none → p2_books, loot/sharpening, gem economy, whole-economy (900 runs), eternal-key wall | `crates/engine-sim`, `docs/balance/*.md` |
| UI & audio | Every P2 action has UI and a cue (13 new procedural sounds); gem/book icons; boss immunity indicator; hub entry for community maps | `docs/audio-p2.md` |

## Deferred

- **Seasons gameplay:** only the data model exists. No season rollover, season boards or rewards.
- **Community map in the real client:** the community flow was verified through realm-bot only. There is no browser-UI automation, so playing a map in the real client was not exercised.
- **Visual and audio review:** nothing new was looked at or listened to. This covers the 3D scenes of the three new dungeons, the new panels, icons and immunity tint, and the procedural sounds. All runs were `--hidden`, and the clients ran `--no-audio`.
- **Late-game sinks:** a sink for surplus Crypt Dust and for gold after best-in-slot. Both are proposed in `docs/balance/P2_BALANCE.md` and not implemented.
- **Terrain/water in community maps:** refused by design. This is still an open question for the owner.

## Architecture

All of this builds on the P1 split; nothing was replaced.

- **Shared rules and data transfer objects** live in `engine-realm`.
- **Gameplay** lives in `engine-game`.
- **The SpacetimeDB realm module** owns live instances and fights. It writes outcomes to an outbox: `pending_result` and `pending_loot`.
- **The API bridge** polls that outbox and writes Postgres. It applies everything idempotently by text keys built from the instance uid.
- **Hub actions are HTTP routes.** Sharpen, reforge, gems, books and community each go through one `HubTx` / `community_requests` pattern.
- **Content is data:**
  - dungeons in `dungeons.json` plus scene JSON;
  - modifiers, loot, sets, gems, books, economy and leaderboards in `assets/catalog`.
- **Monsters and mechanics are approved Rust primitives.** Dungeons and community maps only compose them.

## Persistence

Migrations are additive and never edited after they are applied. Progression is never reset.

| Migration | Adds |
|---|---|
| `0011_hub_requests` | Request-id idempotency for hub writes |
| `0012_seasons` | `game.seasons` |
| `0013_run_history` | `game.dungeon_runs`; result columns for outcome, deaths and damage taken |
| `0014_books` | `characters.books` JSONB (unlocked books, active options) |
| `0015_community_maps` | `community_maps`, `community_map_versions`, `community_requests`; the `community.moderate` permission; sort indexes |

The loadout JSON is versioned: v2 → v3 adds socketed gems as item keys. Old saves migrate on load and come up with empty sockets.

## Networking

- **New HTTP routes:**
  - `/sharpen` (`/enhance` kept as an alias)
  - `/reforge`
  - `/gems/{socket,unsocket,upgrade}`
  - `/books/use`, `/books/active`
  - `/characters/{id}/history`, `/runs/{uid}`, `/leaderboards`
  - `/api/v1/community/*`
- **Retries:** every write carries a client-generated `request_id`. A retry with the same id replays the stored response.
- **Realm module:** gains the private `community_map` table and the service-only reducers `svc_community_put/remove/list`. Queue requests carry `difficulty` for community runs.
- **Client never decides outcomes:** the client sends intents. All random rolls (loot, sharpening, gems, books, reforge, gem upgrades) happen on the server with OS or fight-seeded randomness. The fight seed and instance uid are hidden from clients.

## Security

- **Anti-duplication:**
  - text primary keys on results and loot, built from the instance uid;
  - request-id replay on every hub write;
  - row lock, then engagement check, then write;
  - each write is one atomic transaction.

  §46 step 21 cut the connection mid-request for reforge, gem upgrade and sharpen, then retried with the same id. Each changed gold, dust and gems by exactly one operation.
- **Community content:**
  - Only data is published, never code.
  - Size is capped at 2 MiB before parsing. Unknown keys are rejected, and assets and monsters must come from the approved library.
  - The map is validated on every create, update and publish. Publishing re-checks the stored text, and the realm re-validates on put.
  - Writes are limited to the author, with moderation on top and per-account rate limits.
  - Isolation from progression is enforced in four places: zeroed loot rules, realm reward skip, `apply_community_result`, and an `apply_loot` refusal.

## Tests

- **Unit and integration:**
  - workspace 4712 passed, 0 failed;
  - `engine-realm` with embedded scenes 384;
  - backend 127;
  - Postgres integration (books, gems, history, keys and results, reforge, community) 62.

  Clippy `-D warnings` is clean in the root, the realm module and the backend.
- **Transactions:** each hub route is covered for replay, insufficient funds, engaged refusal, another account, and concurrent races. See the DB tests.
- **Persistence:**
  - loadout migration tests;
  - live restart of Postgres, SpacetimeDB and the API, then a full-JSON comparison of characters, history and boards before and after (§46 steps 23–24).
- **Live end to end:**
  - `realm-bot scenario all` passes 14/14;
  - `p2_full` and `p2_after` pass;
  - the real `mygame.exe` was run `--hidden`. Three clients registered, entered the hub, queued and completed runs, and one survived a kill and relaunch mid-run.

**Simulations** (`docs/balance/`):

| Simulation | Result |
|---|---|
| DPS by gear tier | Rises at every tier, with no survivability regressions. `p2_books` is 1.42–1.48× `high` for every class (within 2 %). |
| Median runs to a goal | First max gem 88–95; full major set 71–93; all books 142–153 |
| Gold | 8–10 runs of income left unspent at run 300 |
| Eternal-key limit | A fully geared player stalls at key 24 (Warrior) or 20 (Druid, Mage) |
| Sharpened drops | Odds of each bonus level fall as the level rises, and the odds of a high bonus never drop as the key rises |

## Performance

Measured on an RTX 5080 at 1080p, `--hidden`, shadows high, 3 runs each, median.

| Scene | CPU p50 | Avg FPS | 1% low |
|---|---|---|---|
| Ashen Reliquary (baseline) | 0.93 ms | 1023 | 698 |
| Drowned Catacombs | 1.19 ms | 784 | 521 |
| Bone Foundry | 1.22 ms | 695 | 329 |
| Sanctum of Ash | 1.16 ms | 833 | 600 |

- **New dungeons:** they cost about 0.25 ms of CPU per frame more than the baseline, which is acceptable.
- **Caveat:** the logs don't confirm the autocast fight ran, so these may be idle-scene numbers.
- **No per-frame database or network work was added.** Validation in the editor is debounced, and set effects are cached per player.

## Known issues

- **Dust and gold sinks:** surplus Crypt Dust has no use, because sharpening past +4 breaks the item ≥ 95 % of the time. Gold also builds up after best-in-slot, about 171 runs of income at run 900. Quality 100+ is a lottery.
- **Forced displacement online:** Undertow (Drowned Catacombs) and the conveyor (Bone Foundry) move players server-side. Under client prediction this may rubber-band; it was not tested online.
- **Untuned numbers:** boss HP, damage and `timer_seconds` for the three new dungeons are simulation estimates, not from played runs.
- **Missing indicators:** Warded and Stoked are not dispellable, and there is no visual telegraph for them beyond their aura icons.
- **Editor:** text fields accept letters, digits, space, `-` and `.` only. Autosave doesn't write `.meta.json`.
- **Community:**
  - A client that reconnects after a map is unpublished mid-run cannot fetch the map data.
  - Title search is a sequential scan.
- **Personal best** is detected client-side from the history page; the backend has no personal-best concept.
- **Flaky checks, not reproduced:** `instanced_scene_equivalence` once crashed with an access violation (GPU test). A WebSocket 401 appeared once for about 30 s after a fresh `realm-dev up`.
- **Set-bonus chime** plays only online.
- **Gear tiers in the sim** (`p2_*`) assume quality 110 +10, so real players hit the eternal-key wall a few keys earlier.

## Recommended next steps (P3)

1. **Play and look at everything:**
   - a visible playtest of the three new dungeons, online with 2–5 real clients, focused on Undertow and conveyor feel;
   - tune boss numbers and timers from real clear times;
   - an art and audio pass on the new icons and sounds.
2. **Economy sinks:** use Crypt Dust in gem upgrades or a cosmetic or re-roll sink, and add a late gold sink. The simulation is already in place to tune them.
3. **Seasons:** rollover, seasonal leaderboards, reward tracks, and resetting eternal keys per season. The data model exists.
4. **Community:**
   - real-client automation for the browser;
   - a trigram title index;
   - a moderation UI;
   - map ratings and reports;
   - a decision on terrain/water support.
5. **Server-authoritative movement reconciliation** for forced displacement, before adding more pull or push mechanics.
6. **Merge `p2-core` → `master`** after the playtest, since `master` is behind all P1/P2 work.
