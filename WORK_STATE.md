# P2 work state — handoff

Updated 2026-10-20. Spec: the P2 brief (Endgame Depth, Content Expansion & Community Systems).
Design + audit: `docs/P2_DESIGN.md` (read first). P1 report: `docs/P1_FINAL_REPORT.md`.
Orchestration per CLAUDE.md (max 3 agents; implementers in `../myGame-wt-<name>` worktrees off `p2-core`).
**Token rule (user, 2026-10-20): never resume a finished agent with SendMessage for new scope — spawn a
fresh agent with a self-contained brief.** Commits carry no Co-Authored-By trailer.

## Decisions (user-confirmed or recorded in P2_DESIGN §0)

- No C/B/A/S tiers, no fusion. Sharpening = P1 enhancement renamed; wire/DB key stays `enhancement`.
- Book gates use key level (character level has no gameplay effect).
- Monsters/abilities/modifier kinds = approved Rust primitives; dungeons/maps compose them as data.
- Community maps give no loot/currency/key progress; leaderboards only. Terrain/water refused for
  community maps (open question to user), community scale ≤ 10.

## Merged into p2-core

- `p2-sim`: engine-sim uses real catalog items via `sheet_of`; tiers none/low/medium/high/p2_max;
  monotonic-DPS test. Finding: p2_max only +9–10 % DPS over high → gems/sets/books must carry endgame.
  Catalog lacks epics for most slots.
- `p2-content`: scene registry generated from dungeons.json (engine-realm build.rs); client loads scene
  by instance dungeon_id; Ashen Reliquary as JSON; `timer_seconds`; patrols (`patrol` waypoints);
  Eternal keys (`modifiers.json` rows 1–12 + `eternal` block, `ModifierCatalog::row_for`); modifiers
  bolstering(5), raging(7), necrotic_pools(11, reuses plague VFX/sound), spiteful_casters(13).
- `p2-mapcheck`: `engine-realm::mapcheck` community-map format + validator, `map_check` bin,
  `docs/community-maps.md`.
- `p2-infra`: instance `uid` (u128) → run ids + whole-fight seed (hidden from clients); `game.hub_requests`
  + `store::HubTx` request-id idempotency on all hub writes; lock row → ensure_unlocked → write;
  strict apply_result; versioned loadout + `migrate`; `game.seasons` (season-0). Migrations 0011, 0012.
- `p2-history`: outcomes timed/overtime/failed/abandoned (`engine-realm/outcome.rs`, clock from barrier
  lift); meter deaths/damage taken; `game.dungeon_runs` + result columns (0013); history API,
  `/runs/{uid}`, `/leaderboards` (`leaderboards.json`, `ScoreKind`); History/Boards screens; timer HUD;
  realm-bot `history` scenario. Live `scenario all` 13/13 passed.

Last full verification (before history merge): workspace 4438 pass, embedded-scenes 285, backend 114,
all clippy clean. History branch alone: 4463 / backend 120 / DB 30.

## In flight

- MERGED `p2-sharpen`: rename, drop curves (`loot.json` `sharpening_drops`, key_shift per_level 0.02
  max 1.4 clamped to keep curves decreasing; tails monotone in key, tested), tooltip, `--loot-sim` →
  `docs/balance/loot-sharpening.md`. Boss P(>=+8) 0.11 % key1 → 1.3 % key20.
- MERGED `p2-dungeon4`: Bone Foundry (ring layout; slag_ironclad Warded, cinder_bomber Detonate,
  slag_thrower, foundry_foreman Stoked/Bestow; foundry_overseer 19k: brand spread, Ladle soak (overflow
  1.25 so solo soaks), Conveyor lane, Overheat). Warded/Stoked not dispellable. Unverified 3D/online push.
- Known bug: an engine-editor test fails while gitignored `assets/generated/` exists (baked minimaps) — fix later.
- `p2-dungeon5` (worktree `../myGame-wt-dungeon5`): Sanctum of Ash (mini-boss + hardest final boss).
- MERGED `p2-sets`: sets.json (3 major 5-pc per class, 3 minor 2-pc), `character/sets.rs`, effects
  damage_pct/cooldown_pct/resource_gen_pct, cached per player in refresh_character; tooltip + panel.
  Warrior p2_set only +2.4 % over p2_max after Foundry ilvl13 epics (Gravewarden pieces buffed) —
  revisit in power-scaling pass. Set piece drop rates untuned.
- MERGED `p2-reforge`: `POST /characters/{id}/reforge {slot, request_id}`, bands EV 71.5, cost
  50+10·ilvl+10·sharpening, broken refused; Workshop "Reforge items" mode. No HTTP/live test yet.
- `p2-gems` (worktree `../myGame-wt-gems`): gems, sockets, routes, economy sim.
- p2-core verified after sharpen+dungeon3 merges: workspace 4457, embedded 312, backend 120, DB 30, clippy clean.
  `engine-renderer` `instanced_scene_equivalence` crashed once (0xc0000005), not reproducible — flaky GPU test.
- `p2-sets` (worktree `../myGame-wt-sets`): sets.json, active_sets, tooltip/panel UI, sim.
- MERGED `p2-dungeon3`: Drowned Catacombs (generator test `export_drowned_catacombs`), roster
  tidecaller/cultist(Waterlogged)/lurker/bilge_archer, boss drowned_abbess (Undertow, Hymn, adds);
  5 epics ilvl 12. Not visually checked in 3D; Undertow online rubber-band untested.
- Leftover dir `C:\Projects\myGame-wt-history` (locked file, worktree already pruned) — delete later.

## Remaining P2 work (order)

1. (done) Sets. 2. Gems (in flight). 4. (done) Reforge. 5. Dungeon 4 done, dungeon 5 in flight.
3. Books — in flight (`p2-books`, worktree `../myGame-wt-books`, migration 0014_books).
   Gems + books may both bump loadout CURRENT_VERSION → resolve at merge.
MERGED `p2-dungeon5`: Sanctum of Ash (3 shrines, all required; mini-boss censer_warden 11k Elite
with own table, boss ashen_pontiff 22k: Branded stacks, LOS Rite behind pillars, pylon immunity, Last
Rite enrage). Statuses Mirrored/Ashen/Branded (count 39), Reflect modifier, EnemyTarget::Weakest,
on_death. No pylon-immunity visual indicator yet (UI polish). 5 dungeons done.
`p2-community` (worktree `../myGame-wt-community`): Postgres+API+realm cm: instances+browser+realm-bot,
migration 0015. ONLY this agent may use the realm stack. Editor dungeon-block inspector = separate task after.
MERGED `p2-gems`: gems.json 7 kinds × 5 levels, every epic socketed (6 with 2), routes
socket/unsocket/upgrade, loadout v3 (gems as item keys), median 108 runs to max gem at key 10,
p2_gems ≈ +9.5 % DPS over p2_set. Gems borrow the trinket icon. `--gem-sim` → docs/balance/gem-economy.md.
MERGED `p2-books`: 3 talent books + 3×3 enhancement books per class as hidden talent nodes (TalentGrid
u64 bitset + granted mask), `characters.books` (0014), /books/use, /books/active, Books page toggle,
book_drops (gate on drop: talent key 8, enhancement key 6). Tiers now …p2_set, p2_gems, p2_books.
Verified on merge branch: workspace 4627, embedded 364, backend 124, DB 53.
MERGED `p2-community`: migration 0015 (community_maps, versions, community_requests, perm
community.moderate→admin), /api/v1/community/*, realm private `community_map` table + svc reducers +
30 s reconciler, `cm:<id>:<ver>` instances, 4-layer no-loot isolation, client browser (via Dungeon
Finder), realm-bot `community`; live `scenario all` 14/14. Dev DB `realm` had 0015 before 0014 —
applied 0014 manually with sqlx checksum. Verified: workspace 4645, embedded 375, backend 127, DB 62.
`p2-polish` (worktree `../myGame-wt-polish`): audio audit, pylon-immunity indicator, gem/book icons,
hub entry for community, community timer_seconds, terminology sweep.
`p2-balance` (worktree `../myGame-wt-balance`): economy sim, set/druid survivability fixes, eternal wall, P2_BALANCE.md.
`p2-editor` (worktree `../myGame-wt-editor`): dungeon-block inspector, community metadata, live mapcheck, export, template.
p2-core verified after sets+reforge: workspace 4527, embedded 331, backend 121, DB 36, clippy clean.
6. Community publishing: Postgres tables, API, realm `community_map` table, `cm:<id>:<ver>` instances,
   editor dungeon-block inspector, client map browser, community boards — §2.10.
7. Economy sim, power-scaling sim with gems/sets/books, audio cues, UI polish, perf check.
8. Final verification (brief §46) incl. live stack + real clients; `docs/P2_FINAL_REPORT.md`.

## Operational notes

- Docker running (user started). Stack: `tools\realm-dev.ps1 up|publish|down|status` (start `up`
  detached). Only ONE agent at a time may use/publish the realm. Postgres: 
  `postgres://realm:realm-dev-password@127.0.0.1:5434/realm`; DB tests from backend\
  with env `DATABASE_URL` set to that URL: `cargo test -p realm-game -- --ignored`.
- realm-bot: `SPACETIME_CLI=%LOCALAPPDATA%\SpacetimeDB\spacetime.exe`. Publishing changes the schema →
  realm data wiped. Bindings: `spacetime generate`, then `cargo fmt -p engine-net`.
- Don't `cargo fmt` whole backend files (not rustfmt-clean). Don't edit applied migration SQL (sqlx checksums).
- GPU is RTX 5080; perf runs headless (`--hidden`).
