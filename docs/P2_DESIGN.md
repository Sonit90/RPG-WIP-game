# P2 design — Endgame depth, content expansion & community systems

Branch `p2-core` (from `p1-core` @ `464cdd8`), started 2026-10-20. This document is the audit result
and the design every P2 work stream follows. Final report goes to `docs/P2_FINAL_REPORT.md`.

## 0. Decisions taken on spec conflicts

- **No item tiers.** The brief's §9–10 (uncut) remove C/B/A/S tiers and tier fusion entirely; P1 never
  had them. §36/§44/§45 references to tiers/fusion are void. There is no fusion.
- **Sharpening = P1 enhancement, renamed.** `ItemProps.enhancement` becomes `sharpening` (serde alias
  `enhancement` keeps old loadouts loading). Max stays 10 (`economy.json`), +1 % quality per level,
  P1 break rules kept. Base quality never overwritten.
- **"Level 20+" for class books.** Character level has no gameplay effect and level 20 needs ~1 M XP
  (≈1400 runs), so book gates use **key level** (`requirement.min_key_level`, data). Default: class
  books drop from bosses at key ≥ 8, enhancement books at key ≥ 6.
- **Monsters/abilities/modifier kinds stay Rust** ("approved primitives"). Dungeons and community maps
  compose them by string id from data. New dungeons may add new archetypes; maps cannot.
- **Community maps grant no gear, currency, books, gems or key progress** (prevents farm maps). They
  record leaderboard entries only.

## 1. Audit summary (current state)

| Area | State | Where |
|---|---|---|
| Auth / sessions | JWT + refresh tokens, Google OAuth, OIDC for realm | `backend/crates/{auth,api}` |
| Character select | `GET /characters`, one character per hero per account | `routes/game.rs`, `store.rs:54` |
| Hub / queue | `POST /realm/queue` → `svc_enqueue` (bridge) | `game.rs:538`, `realm-module/service.rs:45` |
| Matchmaking / party | realm tables `queue_ticket`, `party`, `party_member`; ready check, key insert | `realm-module/{matchmaking,party}.rs` |
| Instances | `dungeon_instance` (auto_inc id), combat tick, finish → outbox | `realm-module/{instance,combat}.rs`, `engine-realm/combat/instance.rs` |
| Keys | Postgres `dungeon_keys` (1/character); progress owner +1, participant +1; CAS on level; max 20 | `engine-realm/key.rs`, `store.rs apply_result` |
| Modifiers | 2 kinds (Rust), `modifiers.json` progression 20 rows (slots, health/damage mult) | `engine-game/modifier`, `engine-realm/modifiers.rs` |
| Classes / talents | warrior, druid, firemage; `talents.json`; points = 5 + earned; server validation; per-talent Rust mapping into `TalentEffects` | `engine-game/talent`, `firemage/talent.rs` |
| Items | `ItemDef` + `ItemStack{props: ItemProps{quality 50–110, enhancement, broken}}`; one multiplier `(quality+enh)/100` → `gear_stats` → combat/tooltips | `engine-game/character/item.rs` |
| Loot | `loot.json` 24 items, boss tables, mob table, quality buckets, epic chance; per-instance seed | `engine-game/loot.rs`, `engine-realm/loot.rs` |
| Persistence | Postgres: `characters.loadout` JSONB (equipment/inventory/mail/resources), results/loot/damage reports idempotent by text PK | `backend/migrations/0001–0010`, `store.rs` |
| Recount / reports | realm meter (damage/healing), `dungeon_damage_reports` | `engine-realm/meter.rs` |
| Reconnect | grace window, Wait/Leave prompt, leaver leaves fight | `realm-module/connection.rs` |
| Admin | realm owner reducers, `/admin` dashboard, ops API | `realm-module/admin.rs`, `api/ops` |
| Editor | places objects/monsters/lights/spawns/terrain/water, packs; saves scene JSON; cannot edit dungeon block | `crates/engine-editor` |
| Asset pipeline | `asset_check` CLI + style validator | `engine-assetcheck`, `engine-assets/asset_validator` |
| Dungeons | 1 data dungeon (crypt); scene embedded by hardcoded `match` (`engine-realm/catalog.rs:744`); online client forced to crypt (`engine-app/main.rs:68`); Ashen Reliquary exists only as Rust (offline) | |
| Sim | `engine-sim` balance matrix; gear modelled as flat stat block, not `ItemStack` | `crates/engine-sim` |
| Seasons / leaderboards / history | none | |

### Risks found (fixed in P2 Phase 1)

1. `result_id`/`loot_id` derive from realm auto_inc instance ids; realm data is wiped on schema change,
   so ids restart and `ON CONFLICT DO NOTHING` would silently drop new runs' grants.
2. Engagement lock is check-then-act (`ensure_unlocked` outside the row lock); queue snapshot read without lock.
3. `save_talents` writes without row lock.
4. `apply_result` silently skips grants when the character row is missing.
5. No request ids on hub transactions (double submit = two attempts).
6. Realm combat RNG constant-seeded except loot; loot seed predictable.
7. Offline save drops `ItemProps` (out of P2 scope beyond not making it worse).

## 2. Architecture (where P2 lives)

Rules stay in shared crates (`engine-game` = gameplay, `engine-realm` = server rules + DTOs), persisted
by the API (`backend`), run by the realm. No new frameworks.

### 2.1 Transactions & idempotency (Phase 1)

- `game.hub_requests (request_id UUID PK, account_id, character_id, kind, response JSONB, created_at)`.
  Helper `store::idempotent(tx, request_id, kind, |tx| -> Result<Response>)`: inside the character
  `FOR UPDATE` transaction, a replayed `request_id` returns the stored response and changes nothing.
  Every progression-changing hub route (sharpen, salvage, exchange, gem socket/upgrade, book use/select,
  reforge, key downgrade/cleanse, equipment move, talents) takes `request_id` (client generates a UUID
  per user action and reuses it on retry).
- Lock order: `SELECT … FOR UPDATE` on the character **then** `ensure_unlocked` **then** mutate; queue
  holds the same row lock while snapshotting and enqueueing. Serialises hub writes vs queue.
- Instance identity: `dungeon_instance.uid` = 128-bit server random (ctx rng) assigned at creation;
  `result_id = "<uid>:<account>"`, loot ids `"<uid>:<account>:loot:<n>"`. Same random seeds the whole
  instance sim (crits, AI, loot) — fixes risks 1 and 6.
- Loadout JSON gains `"version": 2`; reading is tolerant (serde defaults/aliases); migration functions
  in `engine-realm::loadout::migrate` with tests over captured P1 JSON.

### 2.2 Keys, timers, eternal (Phase 2)

- `modifiers.json`: `progression` covers levels 1–12 ("normal"); new block
  `"eternal": { "from_level": 13, "odd": {"add_modifier_slot": 1, "pool": "eternal"},
  "even": {"health_mult_step": 0.12, "damage_mult_step": 0.06} }`. Function
  `ModifierCatalog::row_for(level)` synthesises rows beyond the table (pure, tested). Key max becomes
  `null` (unbounded) / data cap 99 for safety.
- New modifier kinds (Rust primitives, data params): `bolstering` (allies of a slain enemy gain
  % hp/damage), `necrotic_pools` (dead enemies leave a damaging pool — positioning),
  `raging` (enemies enrage below 30 %), `spiteful_casters` (caster cast-time reduction — rewards
  interrupts). Eternal pool introduces combos via slot count.
- `dungeons.json` per dungeon: `timer_seconds`, `timer_per_key_seconds` (optional).
  Outcomes: `timed | overtime | failed | abandoned`. Disconnect within reconnect grace never counts as
  abandon; abandon = party left / all gone past grace.

### 2.3 History & leaderboards (Phase 2)

- `game.dungeon_runs (run_uid PK, season_id, dungeon_id, key_level, modifiers, outcome, duration_us,
  timer_us, party JSONB, started_at, finished_at)` — one row per run, written by `apply_result`
  (`ON CONFLICT DO NOTHING`, all player results share `run_uid`).
- `dungeon_results` gains `run_uid`, `deaths`, `healing`, `damage_taken`, `dps`, `loot JSONB`.
  Realm meter extended with deaths and damage taken.
- `GET /characters/{id}/history?before=&limit=` (paged), `GET /runs/{uid}`.
- Leaderboards = query over `dungeon_runs` where outcome in (timed, overtime), ranked by
  `leaderboards.json` `"score": {"kind": "key_then_time"}` (deterministic: key desc, timed first,
  duration asc, finished_at asc). Endpoints: `GET /leaderboards?dungeon=&key=&season=&board=fastest|highest`.
  Index `(season_id, dungeon_id, key_level, duration_us)`. Never queried from the realm.
- `game.seasons (id, name, starts_at, ends_at)` + seed row `season-0` (all-time). Runs tagged with the
  active season. Keys/rewards remain unseasoned (data model ready, no reset logic).

### 2.4 Dungeons as data (Phase 2)

- Scene registry: `build.rs` in `engine-realm` generates the embed table from `assets/catalog/dungeons.json`
  (wasm realm cannot read files), replacing the hardcoded `match`.
- Client picks the scene from the instance's `dungeon_id` (remove `Scene::Graveyard` forcing).
- Dungeon block extended: `boss_pack`, `timer_seconds`, `loot_table` id, `objectives`.
- Patrol: optional `patrol: [[x,z],…]` waypoints on a monster placement (AI walks loop until aggro).
- Target 5 production dungeons: Graveyard Crypt (existing), Ashen Reliquary (port the Rust layout to
  scene JSON via an export test), + 3 new scenes authored as JSON (generator tool for blockout, then
  editor): **Drowned Catacombs** (water, ranged/caster heavy, patrols), **Bone Foundry** (melee
  brutes, hazards, healer packs), **Sanctum of Ash** (caster/interrupt focus, dispellable curses).
  New bosses each get one mechanic needing a distinct player decision.

### 2.5 Sharpening (Phase 3)

- Rename enhancement → sharpening across code/UI/API (`/sharpen`, old `/enhance` kept as alias one release).
- `loot.json` `"sharpening_drops"`: `default` curve + overrides keyed by `source` (mob/elite/boss/chest/special),
  optional `dungeon`, `min_key_level`; curve = `weights[]` per level, validated strictly decreasing and
  `len ≤ max_level+1`; key shift `key_bonus_per_level` moves weight mass upward but re-validates monotonicity.
  Resolution: most specific match wins, else default.
- `roll_sharpening(rng, curve)` after quality roll (independent of item identity). Loot-distribution sim test.
- Stat formula unchanged: `multiplier = (quality + sharpening)/100` (epic = different item row,
  not a multiplier) — one formula, additive, no multiplication.

### 2.6 Sets (Phase 3)

- `sets.json`: `{id, name, pieces:[item ids], bonuses:[{count, stats:{…}, effects:[{kind, ability?, value}]}]}`.
  2-piece minis and 4–5-piece majors. Items reference `set` in `loot.json`.
- `engine-game::character::sets::active_sets(equipment) -> ActiveSets` (pure). Stats folded into
  `gear_stats`; effects (`resource_gen_pct`, `cooldown_pct{ability}`, `extra_charge{ability}`,
  `damage_pct{ability}`, `talent_rank{…}`) applied at fight build through `AbilityBook::adjust`/
  `TalentEffects`. Server computes; client calls the same function for display only.

### 2.7 Gems (Phase 3)

- `gems.json`: kinds (strength, intellect, stamina, critical, haste, versatility, armor), per-level stat
  values, `max_level` 5, drop sources/chances, upgrade rule `{consume: 2 same gems of level L (+ dust/gold), success_chance[L], on_fail: "keep_one"}`.
- Gems are inventory items (registered `gem_<kind>_<level>` ItemDefs, family `Gem`, stackable).
  Items have `sockets` (0–2) in `loot.json`; `ItemProps.gems: [u16; 2]` (0 = empty; stays `Copy`).
- Routes: `/gems/socket`, `/gems/unsocket` (returns gem, gold cost), `/gems/upgrade`. Target ~100 runs to
  a max gem (economy sim asserts 60–150).

### 2.8 Books (Phase 4)

- `talents.json` per hero: `books: [{id, kind: "talent_point"}…]` and
  `enhancement_books: [{id, name, options:[{id, name, effect…}] ×3}] ×3`.
- Book items (family `Book`, not stackable) drop from boss chests per `loot.json` rules.
  `POST /books/use` consumes the item and unlocks (duplicate → refused with 409, item kept).
- `characters.books JSONB {unlocked:[ids], active:{book_id: option_id}}`; total talent points =
  starting + earned + unlocked talent books. Options are implemented as hidden talent nodes so the same
  `TalentEffects` path applies them (one progression system). `PUT /books/active`, locked while engaged.

### 2.9 Reforge (controlled gamble, Phase 5)

- `economy.json` `reforge`: cost, outcome table over new base quality (e.g. bands with probabilities),
  "keep best" not offered → real risk. UI shows exact odds + expected value + current quality before
  confirm. Server OsRng, idempotent request, atomic.

### 2.10 Community maps (Phase 6)

- Format = existing `SceneDocument` + `dungeon` block + `community` metadata; one pipeline:
  `parse → validate → runtime` shared with official dungeons (`engine-realm::mapcheck`).
- Validation: schema, size limits (objects ≤ 4000, monsters ≤ 200), asset ids ∈ approved library,
  monster ids ∈ approved archetypes (no `boss`-only restrictions beyond 1 boss pack), spawn points on
  walkable ground, boss pack exists, completion reachable (Σ completion ≥ required), loot = none,
  unsupported fields rejected (`deny_unknown_fields` in the community view).
- Postgres `game.community_maps (id, author_account, title, description, status
  draft|published|hidden|rejected, current_version, created_at, updated_at, plays)` +
  `community_map_versions (map_id, version, data JSONB, report JSONB, created_at)`.
- API: create / update (new version) / validate / publish / unpublish / list (search, sort newest,
  most played, fastest, difficulty, author) / get.
- Play: on publish the bridge pushes the validated JSON to realm table `community_map` (service
  reducer); instance build resolves `dungeon_id = "cm:<id>:<ver>"` from it. Data only — no code path.
- Editor: dungeon-block inspector (boss pack, gate, required completion, timer) and metadata panel;
  upload from the client map browser ("Upload level file").
- Leaderboards reuse `dungeon_runs` (dungeon_id = `cm:<id>`), boards: fastest, highest difficulty, completions.

## 3. Streams and order

| Stream | Phases | Files (mostly disjoint) |
|---|---|---|
| A — server/economy | 1 infra → 2 history/leaderboards/seasons/timers → 3 sharpening/sets/gems → 4 books → 5 reforge | `backend/`, `engine-realm/{loadout,enhance,api,key,modifiers}`, `realm-module` |
| B — content | 2 scene registry, client scene select, patrols, new modifiers, 4 dungeons + bosses | `engine-game/enemy`, `engine-game/modifier`, `assets/scenes`, `engine-environment` |
| C — client UI/audio | after each A/B API lands | `engine-presentation/ui`, `engine-app/online.rs`, `engine-audio` |
| D — community | 6 after A2/B2 | `engine-realm/mapcheck`, `backend` maps, editor |
| E — balance | 7 sim with real `ItemStack` gear, economy sim | `engine-sim` |
