# P2 final verification (brief section 46)

Run 2026-10-21 on branch `p2-core` against the live local stack (`tools/realm-dev.ps1`: Postgres in
Docker, SpacetimeDB and the API as native processes). Tooling: `realm-bot scenario p2_full`,
`p2_after`, `community`, `all` (crates/engine-realm-testclient), the real client `mygame.exe`
(`--online --hidden --no-audio --dev-login --dev-autoplay`), `realm-dev.ps1 restart`, direct Postgres
queries, and an ignored editor test (`export_a_map_for_the_live_verification`).

"Client" = the real `mygame.exe` binary (three processes, logs kept). "Bot" = realm-bot, which uses the same
engine-net session code. Run completion uses the owner-only `admin_clear_instance` (kills the monsters, the
realm's own tick then completes the run with its real chest) in both cases.

| # | Step | Method | Evidence | Result |
|---|---|---|---|---|
| 1 | Clean build | build | `cargo build --release` (client, realm-bot), `backend cargo build --release`, module build + publish by `realm-dev up`. Incremental from the current tree, no `cargo clean` | PASS |
| 2 | Start server | script | `realm-dev up`: Postgres, SpacetimeDB, publish, API; `status` all up | PASS |
| 3 | Start client | client | 3 x `mygame.exe --online --hidden`: "online: started", "realm connected" | PASS |
| 4 | Create / log in | client + bot | client: `--dev-login` registered `p2c-*` accounts ("auth: signed in"); bot: register-then-login | PASS |
| 5 | Select character | client + bot | `--dev-autoplay warrior:tank` etc.: "WARRIOR dressed from the persistent character"; bots pick by class | PASS |
| 6 | Enter hub | client + bot | "online: the hub stands"; bots: connected, 3 starters, no ticket/party/instance | PASS |
| 7 | Queue | client + bot | party formed, key inserted (other two get "InsertKey refused: A key is already inserted", expected), "entering instance 42" | PASS |
| 8 | Complete dungeon | client + bot | client: instances 42 and 43 completed, clients returned to the hub ("left the instance"); DB `dungeon_runs` timed, 3 results each. Bot: 3 runs at keys 1, 2, 3 | PASS |
| 9 | Loot | bot + DB | each player: 2 chest equipment (plus sometimes a gem) delivered exactly once, gold equals the summary, quality and sharpening as rolled; real-client characters hold 6-8 mail items and 238-253 gold | PASS |
| 10 | Progression | bot | talent points 5 -> 9 over 3 runs, level 1 -> 2, experience 350 | PASS |
| 11 | Key increase | bot + client | key 1 -> 2 -> 3 -> 4 (owner); client run owners' keys 1 -> 2 in the DB | PASS |
| 12 | Higher key run | bot | run 2 at key 2, run 3 at key 3 | PASS |
| 13 | Modifiers | bot | key 3 run carried `rotating_beams`, seen by every member before the start, recorded in history; `scenario modifiers` (reroll, bonus, refusals) PASS | PASS |
| 14 | History | bot | 3 timed runs, keys [3,2,1], party of 3, modifiers recorded, run opens for a member; stranger refusal in the `history` scenario | PASS |
| 15 | Leaderboard | bot | 3 runs on the `highest` board, each once on `fastest` of its key | PASS |
| 16 | Sharpening + pre-sharpened drops | bot + DB + sim | sharpen +0 -> +1, replay free, persisted. Delivered gear in the DB across all runs: +0 119, +1 15, +2 7, +3 2, +4 2, +5 1; chest lines carried sharpening and arrived identically. `engine-sim --loot-sim` reproduces `docs/balance/loot-sharpening.md` | PASS |
| 17 | Gems | bot | socket (stamina rose), unsocket (gold charged once, gem back), upgrade (consumed/returned/gold add up); each replay with the same request id changed nothing | PASS |
| 18 | Sets | bot | 5 Emberwake pieces worn: server haste = bare + pieces + 4 (2pc) + 2 (4pc), crit +6 | PASS |
| 19 | Books | bot | talent book: learned, +1 talent point; duplicate refused and kept; enhancement book learned, bad option refused, option set and switched | PASS |
| 20 | Gambling (reforge) | bot | quality 90 -> 72 for 310 gold, sharpening kept, replay free | PASS |
| 21 | Disconnect mid-transaction | bot (raw socket hang-up) + API log | reforge cut at once and after 40 ms, gem upgrade and sharpen cut after 10 ms, each retried twice with the same request_id: gold/dust/gems moved by exactly one operation. API log: the cut-at-once request never executed (the retry did, once), the cut-after-40 ms one did (retries logged "request replayed") | PASS |
| 22 | Reconnect | client + bot | client: process killed mid-run 43, restarted with the same flags, "entering instance 43", run completed. Bot: dropped healer seen as disconnected, resumed the same instance at the same position | PASS |
| 23 | Restart server | script | `realm-dev restart`: Postgres container restarted (Up 6 s), SpacetimeDB and API restarted, nothing republished | PASS |
| 24 | Persistence | bot + DB | `p2_after`: all 3 characters of 3 accounts equal the snapshot as full JSON (equipment, bag, mail, resources, talents, books, key, stats), history uids equal, boards list the runs. `state.sql` before/after diff: only a community run's results (delivered by the bridge after the restart) differ | PASS |
| 25 | Create community map | editor test + bot | `Editor::export_community` on the template plus a spawn and patrol, written to a file (ignored test), uploaded by the scenario | PASS |
| 26 | Validate | bot | upload report has no errors | PASS |
| 27 | Publish | bot | `cm:<id>:1`; the map survives the restart (API detail, listing, realm `community_map` row) | PASS |
| 28 | Play it | bot | party at difficulty 3, completed, summary shows no xp/loot/currency/key; holdings unchanged | PASS |
| 29 | Community boards | bot | plays 1, board 1 row, completions board 3, history lists the run | PASS |
| 30 | Malformed content | bot | oversized (413 on create, update, validate); unknown keys (top, scene, object), loot, terrain: `unsupported_feature` / `loot_not_allowed`; timers 5 and 100000 `bad_timer`, -60 `schema_error`; bad asset `missing_asset`; unknown monster `unknown_monster`; broken drafts refuse publish (422); non-author update/unpublish 404, hide/reject/restore 403 | PASS |

`realm-bot scenario all`: 14/14 PASS after the restart (slice, isolation, decline, reconnect, reconnect_party,
security, results, combat, loop, chat, loot, modifiers, history, community).

## Bugs found

1. Fixed (9c2d510): the `loot` scenario asserted exactly two chest lines, but a chest can also roll a gem or
   a book, so it failed whenever one dropped.
2. Fixed (8aab22e): `cargo fmt -p engine-net` (the documented binding step) reformatted `api.rs` and
   `session.rs` every time; they are now rustfmt-clean.
3. Not fixed, observation: the first scenario after a fresh `up` once failed with WebSocket 401 for about 30 s
   and passed on rerun (probably the realm's OIDC discovery of the API issuer cached before the API was up).
   Not reproduced.
4. Environment: the SpacetimeDB left running from an older session used a worktree data dir
   (`C:\Projects\myGame-wt-history`); stopping it lost the realm database, so I restarted via the script's own
   data dir and republished once before the restart test. That directory can be deleted.

No game-code defects were found in steps 1-30.

## Tooling added

`realm-dev.ps1 restart`; `realm-bot scenario p2_full | p2_after` (outside `all`: they seed Postgres via
`docker exec psql` and write a snapshot); community scenario hostile-content checks, `P2_COMMUNITY_MAP`,
`P2_KEEP_PUBLISHED`; ignored editor export test.

## Test totals

| Suite | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --exclude engine-platform` | 4712 passed, 0 failed |
| `cargo test -p engine-realm --features embedded-scenes` | 384 passed |
| `server/realm-module` clippy | clean |
| `backend` clippy, `cargo test --workspace` | clean, 127 passed (64 ignored DB tests) |
| `backend` DB tests (`-p realm-game -- --ignored`) | 62 passed |

## Not verified

The community map was not played through the real client (no UI automation for the browser). The real
clients ran with `--no-audio`, and run completion used the admin clear rather than real combat (autoplay does
not fight). No `cargo clean` rebuild was done for step 1.
