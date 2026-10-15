# P1 final report — Core Gameplay, Progression & Production Readiness

Branch `p1-core` (based on `cee0562`, the end of P0), 2026-10-15. Not pushed.
Everything below marked **verified** was run or observed during P1; anything not verified says so.

**Status: P1 is not complete.** The systems are implemented server-authoritatively and covered by
automated tests, and a real-client run of the loop worked end to end, but several §49 checks were
only done through bots/tests, not with real clients (see §11), and the realm's combat RNG is still
seeded with a constant apart from loot (see §10).

## 1. Summary of implemented systems

| Spec | System | State |
|---|---|---|
| §2.1 | Warrior: Valor (from War Stomp, Colossus Strike, Shield Charge) → Battle Fervor (+10 % dmg, −3 % taken, 30 s, max 3), replaces Deflection; Colossus bleed cap 3; Shockwave retune | done, tested |
| §3 | Druid damage nerf (Nature's Exposure 15→10 stacks, 3→2 %; Wrath 2.20→1.85) | done, sim-verified |
| §4 | Mage Moving Fire Beam ultimate | done, tested (no in-game visual check) |
| §5 | Enemy ability audit: all 58 abilities checked; fixed pounce targeting/min range, random picks need line of sight, marks skip dead/departed victims, hazard ticks flagged periodic (`docs/monsters-audit.md`) | done, 25 tests |
| §6–8, §11 | Balance simulator for all classes: gear none / 4× medium / top, builds Default/A/B, rotation AIs, talent build validation (`docs/balance/sim-report.md`) | done |
| §9–10 | Data-driven talents for all heroes (`assets/catalog/talents.json`), points per completion, server validation, engagement lock | done |
| §12–19 | Dungeon modifiers (Rotating Beams, Plague Explosion), progression table, rolling on keys, reroll at 50 % kills with currency, +25 % reward bonus, free downgrade, gold cleanse | done |
| §20–21 | Key selection (available keys), key progression: owner +1 on success, every participant with a lower key +1 | done, **verified with real clients** |
| §22–29 | Loot: data-driven `loot.json`, 2 chest items per player, mob drops, quality 50–110, epic chance, aether shards + gold, enhancement (server RNG, break chances 95/96/97/98/99/99.5 % on the last six levels), tooltips Base/Enh/Total | done |
| §30 | Active trinkets: group damage reduction ward, mass dispel, group cooldown reduction, group absorb | done, tested |
| §31–32 | Recount (server meter, current fight / overall, damage/healing) and persistent damage reports (`game.dungeon_damage_reports`, per-ability breakdown) | done, **verified with real clients** |
| §33–35 | Reconnect, party disconnect prompt (Wait/Leave), leaver removed from the fight, equipment/talent lock while engaged | done; reconnect **verified with real clients** |
| §36–38 | Admin dashboard `/admin` (`ops.view`), live instance monitoring and map, anomaly report | done, **verified via the ops API** |
| §39 | Audio pass: 53 new sounds (SOUND_COUNT 112), edge-triggered UI/realm sounds | done; sounds never auditioned |
| §40 | Icon pass: Warrior icons reframed, `ui/key.svg`, `ui/dice.svg`, talent family icons, contact sheets `docs/images/icons/` | done |
| §41 | UI polish: kit tones/close/banner/hint, layout audit test (3 resolutions × 3 scales), screen hierarchy pass, Enter/Escape handling | done |
| §42 | Chat: server / party / whisper, account display names | done |

## 2. Files/modules changed

281 files, +48 811 / −3 737 lines since `cee0562`. Main areas:

- `crates/engine-game`: `warrior/valor.rs`, `firemage/beam.rs`, `talent/{mod,druid,warrior}.rs`,
  `modifier/{mod,beams,plague}.rs`, `trinket.rs`, `loot.rs`, `sim/{beam_exec,modifier_exec,talent_exec,trinket_exec,loot,enemy_exec}.rs`, `enemy/brain.rs`.
- `crates/engine-realm`: `loot.rs`, `enhance.rs`, `haul.rs`, `meter.rs`, `modifiers.rs`, `ops.rs`,
  `reconnect.rs`, `chat.rs`, `combat/instance.rs`.
- `server/realm-module`: new `chat.rs`, `modifiers.rs`; `admin`, `combat`, `connection`, `instance`,
  `loot`, `matchmaking`, `party`, `schema`, `service`, `views` changed.
- `backend/crates/api`: `ops/{mod,anomalies,errlog,gear,dashboard.html}`, `routes/ops.rs`.
- `crates/engine-sim`: the whole balance simulator (`ai/`, `balance`, `class`, `gear`, `matrix`,
  `metrics`, `validation`).
- `crates/engine-presentation`: `ui/{recount,modifiers,chat,layout_audit,kit,talent_menu,frontend}`, `beam.rs`, `audio.rs`.
- `crates/engine-app`: `online.rs`, new `online_audio.rs`, `app.rs`.
- `crates/engine-audio`, `crates/engine-renderer` (icons, atlas), `crates/engine-net/src/bindings` (regenerated).

## 3. Database/schema changes

Postgres migrations: `0006_damage_reports.sql`, `0007_ops_permission.sql` (`ops.view`),
`0008_talent_points.sql`, `0010_key_modifiers.sql` (0009 unused). SpacetimeDB schema: modifier,
reroll and spend state on instances, chat tables, damage meter rows, loot rows (realm data is cleared on
schema change via `--delete-data=on-conflict`).

## 4. New server systems

Realm module: modifier rolling/reroll/spend, key selection with modifiers, talent validation on
engagement, active trinkets in the authoritative sim, server damage meter and report hand-off, chat,
reconnect/leaver handling, owner admin reducers (`admin_finish_instance`, `admin_clear_instance`),
admin snapshot procedure, per-instance loot seed. API: enhancement endpoint (server RNG, row lock),
talent points, damage-report persistence, ops status/realm/instance/anomaly endpoints and dashboard.

## 5. New client/UI systems

Recount window, modifier window and key reroll UI, available-keys select, talent menu rebuilt on the
UI kit with family icons, chat panel, reconnect Wait/Leave prompt, loot HUD tones, completion screen
with grouped rewards, Moving Fire Beam presentation, online UI sounds (`online_audio.rs`).

## 6. New configuration/data structures

`assets/catalog/talents.json`, `loot.json`, `modifiers.json`; `economy.json` (enhancement, break
chances, reroll/cleanse costs) and `characters.json` extended.

## 7. Tests added

`engine-game/tests/{enemy_audit,warrior_talents,druid_talents,fire_beam,modifiers,trinkets}.rs`,
`engine-sim/src/tests.rs`, `engine-renderer/tests/icon_sheets.rs` (ignored; writes contact sheets),
`backend/crates/api/tests/ops_authz.rs`, unit tests across `engine-realm` (incl. the loot-seed test
added in the final step), `engine-presentation` (layout audit for the online screens, talent icons),
`engine-app` (`online_audio`), and realm-bot scenarios `chat`, `loot`, `modifiers`, `reconnect_party`, `results`, `combat`.

## 8. Tests executed and results (final state)

On `p1-core` after the last merge and the loot-seed fix (`dcd9e81`):

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test --workspace` | 4372 passed, 0 failed, 3 ignored (147 test binaries) |
| `server/realm-module`: `cargo clippy --all-targets -- -D warnings` | pass |
| Backend `cargo test --workspace` (before the fix; backend unchanged since) | 111 passed, 0 failed, 20 ignored |
| Backend DB tests (`realm-game -- --ignored`, live Postgres) | 18 passed, 0 failed |
| `realm-bot scenario all` against the republished realm | 12/12 pass: slice, isolation, decline, reconnect, reconnect_party, security, results, combat, loop, chat, loot, modifiers |
| `engine-sim --iterations 20` (seed `0xF12E5EED`) | `docs/balance/sim-report.md` regenerated; byte-identical to the committed report |

`server/realm-module` has no native test run: it only links as wasm (SpacetimeDB host imports), so
`cargo test` there fails at link time; its logic is tested through `engine-realm`.

Real clients against the local realm (release build, `tools/run-3-clients.ps1 -Autoplay` and three
fresh `p1e2e-*` accounts), verified on 2026-10-15:

- Fresh accounts registered, characters created, hub → queue → match → accept → key → start: **verified** (instance 46).
- `/admin/ops/realm` shows each run live (players, presence, health, kills, tick lag); `/admin` page serves (200): **verified**.
- Disconnect (healer process killed): presence `disconnected`; relaunch: `present` again in the same instance: **verified** (instance 43).
- Combat: the Fire Mage was driven with synthetic key presses; monsters attacked players (health dropped); the damage report persisted Mage 2892 damage, 31.8 DPS, Ignite 1512 / Flame Lance 780 / Firebolt 600: **verified** (instance 46).
- Recount window open in the client listing all three players: **observed** (screenshot).
- Completion persisted (`game.dungeon_results`, success, 2 chest items per player in `loot_deliveries`, +1 talent point each; TAL counter in the client showed it): **verified**.
- Key progression: owner 1→2; equal key unchanged; next run used the upgraded key; participant with a lower key 1→2 (`participant_key_bump`); owner 2→3 rolled `plague_explosion` at level 3: **verified**.
- Return to hub and queue again: **verified** (autoplay looped through 3+ runs).

## 9. Performance observations

Realm ticks kept up (`ticks_behind` 0–2) with 2–3 concurrent instances and three clients on one
machine. No profiling of P1 systems (recount, modifiers, UI) specifically was done.

The development machine's GPU changed on 2026-10-15 from an AMD RX 6700 XT to an NVIDIA RTX 5080;
the client runs above were on the RTX 5080. Every earlier performance figure in `docs/` is from the
RX 6700 XT, so new measurements need a fresh baseline before comparing.

## 10. Known bugs

- **Fixed in the final step:** every realm fight was built from the constant default seed, so every
  run rolled the same chest per seat (observed: identical items across runs and accounts). Loot is
  now seeded per instance (id + creation time; stable across a fight rebuild). Verified live:
  consecutive runs now roll different chests.
- The rest of the realm sim (crit rolls, enemy random picks) still uses the constant seed: runs with
  identical inputs make identical rolls. Not exploitable in practice, but should be seeded the same way.
- The loot seed is derived from public values (instance id, creation time), so it is predictable to
  someone who knows the algorithm; it cannot be chosen. A stored server-random seed needs a schema change.
- At 960×540 the chat panel overlaps the party frames (the layout audit covers 1280×720 and up).
- Autoplay leaves the chest unopened, so chest items are mailed as `unclaimed` (by design, noted for testing).
- A run whose boss dies to `admin_clear_instance` reports no `boss_kill_seconds`.
- Runs whose players all disconnect linger as `active` for several minutes before cleanup.

## 11. Remaining P1 work

- §49 with real clients, not yet done (covered only by bots/unit tests): modifiers active in a client
  run (11), talents used in combat (13), trinkets (14), players killing the boss (15), opening the
  chest and item quality in the UI (17–18), enhancement from the client (19), dungeon state restore
  during a boss fight (23).
- Visual checks in a running game: icons, Moving Fire Beam, modifier telegraphs, new panels.
- Sounds never auditioned.
- Player stun/silence do not exist; channel-break and trinket stun refusal are hooks only.
- Offline play starts with 5 talent points and cannot earn more.
- Blood/Crimson Frenzy and Mend have no dedicated sound/VFX (`docs/monsters-audit.md`).

## 12. Recommended P2 priorities

1. A manual (human) §49 pass covering the items in §11, then fix what it finds.
2. Seed the whole realm sim per instance (server random, stored), not just loot.
3. Combat automation for E2E (a bot that fights in the real client) so §49 can run unattended.
4. Player crowd control (stun/silence) to make interrupt-style trinkets and talents real.
5. Then the P2 content backlog (§47: more dungeons, item fusion, gems, sets, run history).
