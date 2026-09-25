# Phase — Graveyard Dungeon Vertical Slice

One integrated feature: six undead monsters with offensive kits, monster placement in the level
editor, a dungeon-completion percentage, two optional routes, a complete test dungeon — the
**Graveyard Crypt** — and a Moroes-inspired boss, **Lord Ashgrave**, who calls two random
**empowered** versions of the dungeon's monsters. Balanced for a Tank + DPS + Healer party.

```text
cargo run --release --features editor --bin mygame -- --scene graveyard [--party 3] [--editor]
cargo test -p engine-game --test graveyard
```

Design documents: [`docs/dungeon/graveyard/`](docs/dungeon/graveyard/README.md) (layout, routes,
completion, balance), [`monsters.md`](docs/dungeon/graveyard/monsters.md),
[`boss_lord_ashgrave.md`](docs/dungeon/graveyard/boss_lord_ashgrave.md); the file format and
editor workflow in [`docs/level-editing.md`](docs/level-editing.md) §3.

---

## What was built, and where

Nothing here is a parallel system. Every piece extends one that existed.

| Piece | Where | Extends |
|---|---|---|
| 13 monsters (6 + 6 empowered + boss), 44 abilities | `engine-game/src/enemy/mod.rs` | the `ARCHETYPES` / `ENEMY_ABILITIES` tables (rows, not code) |
| New ability columns: lines, multi-target, zones, delays, follow/chain marks, channels, charges, growing pools, riders, frenzy, requirements, audio cue | same | `EnemyAbilityDef` (with a `BASE` row so old rows are unchanged) |
| Delayed strikes (marks, rains, quakes, shockwaves) | `enemy/strike.rs` | the hazard field's shape: bounded, event-announced, cleared by death/reset |
| Spreading pools | `enemy/hazard.rs` | growth steps announced as area end + start |
| Brain: gap-closers while chasing, ranged retreat, frenzy haste, per-placement aggro, requirements | `enemy/brain.rs` | the same state machine; still no `match` on monster |
| Executor: cast planning, charges, channels, riders, strikes | `sim/enemy_exec.rs` | the same `deal_damage` path, same telegraph events (+`follow`) |
| Stacking bleed, slow | `status.rs` (`Rended`, `Crippled`) | the status system; slow read by `move_speed_multiplier` |
| Completion, boss seal and gate, boss adds, boss reset | `dungeon_run.rs`, `sim/dungeon_exec.rs` | encounters (adds are encounter *summons*), collision statics, the seeded RNG |
| Levels with monsters → encounters + dungeon run, party scaling, per-placement level | `dungeon.rs` (`populate_from_scene`, `build_level_file`) | `arena::environment_arena` (now `…_for_party`) |
| Monster placements + dungeon settings in the scene and its file | `engine-environment/src/monster.rs`, `scene_file.rs` | the scene file (optional keys, no version bump) |
| **The Graveyard Crypt** | `assets/scenes/graveyard_crypt.json` | a level file — no Rust describes its layout |
| Editor: Monsters browser folder, `U` to place, `Selected::Monster` with every verb, inspector section, hierarchy group, markers | `engine-editor` (`monster.rs` + the existing modules) | the tagged-selection pattern spawn points use |
| Progress panel | `engine-presentation` `game_ui.rs`, `snapshot.rs` (`DungeonView`) | the HUD draw list |
| Monster looks (tint, glow), line/mark telegraphs, gate wall, add/unlock bursts | `extract.rs`, `vfx.rs` | the existing effect catalogue (no new effects) |
| Audio | `audio.rs` | enemy casts by `EnemyCue`; unlock/add/completion reuse existing sounds |
| `--scene graveyard`, `--party N`, catalogue → editor, markers, rebuild on leaving the editor | `engine-app` | the scene loader and editor integration |

Crate boundaries hold: `engine-game` has no renderer types (a monster's look is three floats and
a model name); `engine-environment` names monsters by id only; the editor is handed the monster
list by the app and never links gameplay directly.

---

## Tests

`cargo test --workspace --all-features --no-fail-fast`: **3,217 passed, 1 failed** — the failure
is `engine-editor browser::tests::the_real_directory_shows_its_categories_when_the_editor_opens`,
which counts rows from the real `assets/` folder and already failed before this phase because of
local untracked files there (`my_level.json`, `showcase.autosave.json`, root-level `.glb`s).
`cargo clippy --workspace --all-features --tests` and `cargo fmt --check` are clean.

New tests:

| Area | Where | Count |
|---|---|---|
| Monsters, abilities, AI, mechanics | `engine-game/tests/graveyard.rs` | 20 |
| Completion, routes, gate | same | 8 |
| Boss, adds, empowered, reset, completion | same | 8 |
| Balance model | same | 7 |
| The level file itself, edit → save → reload → play | same | 7 |
| Table invariants, strikes, run arithmetic, hazards | `enemy/mod.rs`, `enemy/strike.rs`, `dungeon_run.rs` | 16 |
| Monster placement, dungeon settings, scene file round trip | `engine-environment` | 6 |
| Editor: browse, place (`U`, double-click), select, move, rotate, scale, duplicate, delete, undo, inspector, packs, save/load | `engine-editor` | 13 |
| Progress panel | `engine-presentation/tests/dungeon_progress.rs` | 3 |

## Verification performed

* The game ran the crypt from its JSON (`--scene graveyard --party 3 --hidden --frames 600`):
  351 objects, 37 monsters in 14 packs, 152 completion placed, clean shutdown.
* The editor opened on it (`--editor`), saving to `assets/scenes/graveyard_crypt.json`, clean
  shutdown.
* Flow, as automated tests: place/move/rotate/duplicate in the editor → save → load → the
  placement persists → it spawns in its pack and fights → main path + either route reaches 100 %
  → the gate opens → pulling the boss calls exactly two distinct empowered adds from the pool →
  adds and boss die → `DungeonCompleted` once → reset → run again.
* Performance: ~17 µs per simulation tick with the boss and both adds fighting (release),
  0.4 % of a 240 FPS frame.

**Not done:** a human playthrough with a real Tank + DPS + Healer — the engine has one
controllable hero and no allied party members, so party balance is verified against an explicit
numeric model (see the README's balance table), and the running game was exercised headlessly.
Pacing and feel still need a hand playtest; `--party 1` makes that possible solo.

## Later changes

* **Wipes keep progress.** Respawning after a death (or `BACKSPACE` while dead) resets only the
  monsters still standing; killed monsters stay dead and completion is kept
  (`Simulation::reset_after_wipe`). `BACKSPACE` while alive is still the full development reset.
* **Charges are casts on a player**: no floor telegraph, the charger follows its target, hits
  them on arrival and slows them 50 % for 1 s (`EnemyEffect::Charge { slow, slow_duration }`).
* **The crypt is brighter** — stronger moon and ambient, brighter and wider braziers, in the
  level file.
* **Number-pad digits** now type into editor fields (and act as the digit keys everywhere).

## Known limitations

1. **No party AI.** Balance for three is a model plus scaling, not three players.
2. **Stand-in art.** Monsters are the enemy model tinted and scaled per monster; graves are
   flattened rocks, sarcophagi are crates. The level only uses the existing library.
3. **A placement's custom name** is shown in the inspector and hierarchy but typed in the file,
   not in the editor (the inspector edits numbers).
4. **Sounds** reuse the three existing enemy gathers by cue; no new audio assets.
5. **Gate collision is axis-aligned**, exact for doorways along X or Z.
6. **Threat is still damage-only** (no taunt), so "the tank holds the boss" in solo play is simply
   "the player is the only target".

---

## Dungeon map and minimap (later)

The crypt now has a circular minimap (top right, rotating or north-up), a full-screen map on
`M` with zoom, pan and encounter selection, and a sidebar that lists every monster of a pack and
every ability of every monster from `ENEMY_ABILITIES` — with two new columns, `description` and
`counter`, filled from these design documents — plus the boss's phases, guests and loot table
(empty: "NO LOOT CONFIGURED"). The picture is baked from the level's geometry through the real
renderer on first load (~0.2–0.3 s) and kept in `assets/generated/maps/`; markers are derived
from the placements, so the editor's map preview (F4) follows edits live. Minimap cost: within
noise (+0.004 ms CPU/GPU). Full account, measurements and limitations:
[`docs/dungeon-map.md`](docs/dungeon-map.md).

Tests after it: `cargo test --workspace --all-features --no-fail-fast` — **3,268 passed, 1
failed**, the same pre-existing `browser::tests::the_real_directory_shows_its_categories_when_the_editor_opens`.

### Map interface redesign (later still)

The full-screen map, the journal and the minimap were rebuilt as a finished game interface:
bundled Inter + Cinzel drawn from a build-time signed-distance-field atlas (`UiCommand::Label`),
rounded/gradient/shadowed panels (`UiCommand::Panel`), a design-token module
(`engine-presentation/src/ui/map_style.rs`), new insignias and control glyphs, a restyled baked
map, run-bound completion and per-pack cleared/in-combat/sealed states, folding monster cards and
ability rows with counterplay callouts, filter tabs, keyboard navigation, eased motion with a
reduced-motion setting. Still one UI draw call. Details and measurements:
[`docs/dungeon-map.md`](docs/dungeon-map.md).
