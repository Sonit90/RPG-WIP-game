# P1 work state — handoff

Updated 2026-10-15. Spec: the P1 brief (Core Gameplay, Progression & Production Readiness).
Final report: `docs/P1_FINAL_REPORT.md` (read it first: status, verified items, known bugs, remaining work).
Orchestration per CLAUDE.md: Opus main session designs/reviews/merges; `implementer` agents work in
git worktrees; `runner` verifies; max 3 agents at once.

## Branches

- **`p1-core`** (main checkout `C:\Projects\myGame`) — integration branch, everything merged. Not pushed.
  Commits carry no Co-Authored-By trailer (user rule).
- `p1-enemies`, `p1-ui`, `p1-audio` merged (`3127156`, `36a6750`, `a20750b`); their worktrees and
  branches are deleted. No P1 worktrees remain. (`.claude/worktrees/agent-a6dc99e9390f24845` is an
  older, unrelated agent worktree — not touched.)
- `dcd9e81` fix(realm): per-instance loot seed (every run used to roll the same chest).

## Last verification (after `dcd9e81`)

clippy `-D warnings` clean (root + realm-module); workspace tests 4372 passed / 0 failed / 3 ignored;
backend 111 passed + 18 DB tests (before the fix, backend unchanged); realm-bot `scenario all` 12/12.
`realm-module` has no native `cargo test` (wasm-only link) — not a regression.
Sim report regenerated (byte-identical).

## §49 end-to-end with real clients (done 2026-10-15)

Verified: fresh accounts → characters → hub → queue → match → accept → key → start; `/admin/ops/realm`
shows runs live; disconnect/reconnect of a client (presence restored, same instance); real combat
damage persisted in damage reports with per-ability breakdown; Recount window in the client;
completion + 2 chest items + talent point persisted; key progression (owner +1, lower-key participant +1,
modifier rolled at key 3); return to hub and requeue. Details in the final report §8.

Not verified with real clients (bots/tests only): modifiers active in a client run, talents/trinkets
used in combat, players killing the boss, opening the chest/quality UI, enhancement from the client,
restore during a boss fight. Needs a human play pass.

## Decisions made with the user

- Valor stacks from War Stomp, Colossus Strike, Shield Charge; 5 → Battle Fervor (+10 % dmg, −3 % taken, 30 s, max 3).
- Every participant whose key is lower than the used key gets +1 on success.
- Enhancement: early levels safe; last six levels break 95/96/97/98/99/99.5 % (config in economy.json).
- Work on branch `p1-core`, commit per milestone; local Docker/SpacetimeDB stack may be used.
- A local admin account may be created for /admin checks.

## Operational notes

- **Hardware changed 2026-10-15: GPU is now NVIDIA RTX 5080 (was AMD RX 6700 XT).** All older perf
  numbers in `docs/` are RX 6700 XT — re-baseline before comparing.
- Stack: `tools\realm-dev.ps1 up|publish|down|status`. `up` can block the shell — start it detached.
  `publish` regenerates bindings; run `cargo fmt -p engine-net` after. Schema changes clear realm data.
- Local admin for ops: `ops@local.mygame.test` / `local-password-1` (local DB only; promoted to the
  admin role and email-verified by SQL). Ops API: `GET /api/v1/admin/ops/{status,realm,anomalies,instances/<id>}`
  with its bearer token; dashboard page at `http://127.0.0.1:8080/admin`.
- Driving runs to completion without fighting: owner `spacetime call --server http://127.0.0.1:3000 realm admin_clear_instance <id>`
  (kills everything, real chest path) or `admin_finish_instance <id> true`.
- Real clients: `tools\run-3-clients.ps1 -Autoplay` (accounts tank/healer/dps@local.mygame.test).
  Autoplay queues/accepts/starts and returns to hub but does not fight or open the chest.
- Backend tests while the API runs: `--target-dir backend\target-verify` (exe lock), then delete it.
  DB tests: `DATABASE_URL=postgres://realm:realm-dev-password@127.0.0.1:5434/realm cargo test -p realm-game -- --ignored` (from `backend\`).
- realm-bot owner scenarios need `SPACETIME_CLI=%LOCALAPPDATA%\SpacetimeDB\spacetime.exe`.
- Disk: each worktree target dir is ~10 GB. Delete worktrees after merging.
- Migrations: 0006 damage reports, 0007 ops permission, 0008 talent points, 0010 key modifiers (0009 unused).

## Known open items

See `docs/P1_FINAL_REPORT.md` §10–11. Highlights: rest of the realm sim RNG still constant-seeded;
chat overlaps party frames at 960×540; visual/audio checks not done in a running game; no player
stun/silence; offline play cannot earn talent points.
