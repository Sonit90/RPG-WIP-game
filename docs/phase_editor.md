# Level editor phases: inspection, plan and record

This document is written as the work happens. Section 1 is the forensic inspection that
preceded any change; the sections after it record what each phase did and why.

## 1. Architecture summary

A Rust workspace, eighteen crates, D3D12 renderer, no external engine. The dependency rule
the workspace is built around is that *level data depends on nothing*:

```text
engine-core            math, transforms, input, time
engine-environment     the level AS DATA (assets by name, placements, collision primitives,
                       lighting, spawn points, the JSON scene file). Depends on engine-core
                       and serde. Never a mesh, a device or a collision world.
engine-assets          meshes, materials, textures, glTF import, procedural authors
engine-collision       static world (Plane/Aabb/Obb), character controller
engine-game            simulation: entities, health, statuses, abilities, arena build
engine-render-data     RenderWorld, Camera, UiDrawList  (no device)
engine-renderer        D3D12
engine-presentation    extraction, camera, UI widgets/screens/painting, HUD
engine-editor          the level editor: edits an EnvironmentScene, paints a UiDrawList
engine-app             the one binary where all of it meets (`mygame`)
```

`engine-editor` deliberately depends on neither `engine-renderer` nor `engine-game`: it
edits data and returns `EditorOutcome`, and the application answers it.

## 2. Existing editor capabilities (before this work)

`crates/engine-editor`, 1 693 lines, behind `engine-app`'s `editor` feature (`--features
editor`), opened in-game with **F6** or at launch with `--editor`.

* fly camera (`camera.rs`), WASD + right-drag look
* palette of the 18 compile-time library assets, drawn as `UiScreen<usize>` buttons
* left click on ground places the palette's asset; left click on an object selects it
* drag moves, snapped to a grid (`G` cycles 1/2/4/0.25/off)
* `R`/`SHIFT+R` yaw by 22.5°, `T`/`SHIFT+T` scale by 1.1×, `X` delete, `C` duplicate
* `Z` undo — whole-scene snapshots, depth 64
* `CTRL+S` writes `assets/scenes/<name>.json` through `engine_environment::scene_file`
* ray picking against per-asset bounds (`pick.rs`)

## 3. Engine capabilities relevant to level editing

* `EnvironmentScene`: name, `Vec<EnvironmentObject>`, `SpawnPoints`, lighting, local lights,
  `half_extent`, `ground_height`
* `EnvironmentObject`: asset id, `Transform`, `EnvironmentCollision::{None, Box}` in model
  space, carried through the transform when the collision world is built
* `scene_file`: versioned JSON (`FORMAT_VERSION = 1`), strict loading, object records carry
  asset *name*, position, yaw, scale, optional collision
* `engine_game::arena::{environment_arena, add_environment_collision, build_collision}` —
  the whole of "load a level" for gameplay: floor plane, boundary walls, then every blocking
  placement as an Aabb (unrotated) or Obb (yawed)
* `engine_presentation::EnvironmentPlacer` — spawns/clears render entities for a scene and
  updates LOD per frame
* `--scene-file <path>` loads an authored level into the game with no editor present

## 4. Asset pipeline

* `assets/` currently holds one `.glb` and a README — production content is *absent*, which
  matches the brief.
* `engine_assets::disk::asset_roots()` searches `$MYGAME_ASSETS`, `<exe>/assets`,
  `<cwd>/assets`, then the repository's `assets/`. A missing file is not an error.
* `gltf_import` turns bytes into a `ModelAsset`; `asset_validator` enforces the documented
  limits at import.
* `procedural::environment::environment_model(asset)` authors all 18 library models in code
  (prisms, generated textures). `environment_library()` is what the application uploads.
* Textures are generated procedurally from a deterministic hash (`hash01`), 128×128 RGBA.

## 5. Player spawn / respawn (before this work)

* `EnvironmentScene::spawns.player` is a single `Vec3`; `ArenaConfig::from_environment`
  copies it into gameplay and `populate` spawns the player there. There is **no rotation**,
  no multiple spawns and no spawn *object* in the level.
* There is **no respawn and no death state**. `Simulation::slay()` kills through the damage
  path; `revive_player()` restores health *where the corpse stands* and is used by the
  development encounter reset. `player_alive()` is the one liveness question.
* `StatusKind` is a closed enum with a full runtime (stacks, durations, dispel schools,
  events) — the status system to reuse, not to duplicate.

## 6. Reusable systems

Scene file, `EnvironmentPlacer`, `add_environment_collision`, `UiScreen`/`Widget`/`paint`,
`ui::targeting::{cursor_ray, ground_under_cursor}`, the procedural mesh builder
(`MeshBuilder`, `Prism`) and texture generators, `StatusKind`/`Status`, `Simulation::slay`.

## 7. Missing systems

1. **A runtime asset catalogue.** `LIBRARY` is a compile-time `[EnvironmentAsset; 18]` and
   `EnvironmentAsset` wraps a `&'static str`; nothing can name an asset discovered on disk.
2. **Recursive asset discovery** of `assets/` with folders preserved.
3. **Collision is not rebuilt after an edit** — the editor says so in the log and asks for a
   relaunch. Playtesting what you just built is the whole point.
4. **Spawn points as level objects** (position + yaw, more than one).
5. **Death → respawn**, respawn protection, and a debug kill key. F7 is currently the shadow
   debug view.
6. **Terrain, cliffs as first-class modular pieces, water, cave modules.**
7. Multi-selection, rotation snapping, local/world transform modes, reset transform.

## 8. Implementation order

Phases 2–14 of the brief, unchanged, each compiling and tested before the next.

## 9. Architectural problems resolved before implementation

* **`EnvironmentAsset` cannot name a disk asset.** Fixed with an interner in
  `engine-environment` (`EnvironmentAsset::interned`) so a name read from a file becomes a
  `Copy`, `'static` id — the type, the scene file and every existing `match` are unchanged.
* **Model resolution is a `match` over constants.** A catalogue in `engine-assets` resolves
  a name to a `.glb` on disk, else to a generated model, else to the visible "missing" cube.
* **Collision after an edit.** The application rebuilds the static world from the scene when
  the editor reports a change, through the same `add_environment_collision` the loader uses.

## Phase 2 — the editor foundation

What was already there was kept; four things were missing and were added.

### A level that does not exist yet

`EnvironmentScene::empty(name)` and `--editor`. With `--editor`, a `--scene-file` naming a
file that is not there opens a *new* empty level saved to that path by CTRL+S:

```text
mygame --editor --scene-file assets/scenes/my_level.json
```

Without `--editor` a missing scene file is still a hard startup failure — a game handed an
empty level instead of the one it asked for looks exactly like a level that failed to save.

### Opening the editor at launch

`--editor` sets a request the first frame answers through the same path F6 goes down, so
"started in the editor" and "pressed F6" cannot drift apart. The editor's save path is set
from `--scene-file`, so saving writes back to the file the level was opened from.

### Collision follows the edit

`App::rebuild_environment_collision` calls the loader's own
`arena::build_collision` + `arena::add_environment_collision` + `rebuild_broad_phase` when
the editor reports a change and again when it is left. Before this, the log told the author
to relaunch the game to walk into what they had built.

### Hierarchy and inspector

Two panels down the right-hand side:

```text
┌ HIERARCHY 141 ┐      every placement, in file order, the selected one boxed
│ 0 GROUND_PLAIN│      click a row to select it
│ 1 ARENA_FLOOR │      the wheel scrolls it; a new selection scrolls itself into view
│ ...           │
└───────────────┘
┌ INSPECTOR ────┐      the selected object's transform, as numbers
│ PILLAR        │
│ POS 12.0 0.0 -4.0
│ YAW 45.0
│ SCALE 1.0 1.0 1.0
└───────────────┘
```

Rows are painted rather than made of `Button`s: a row's label is built from the scene
(`"12 PILLAR"`), and a `Button`'s label is a `&'static str`. The rectangle a click is tested
against and the rectangle drawn come from one function, `ui::hierarchy_row_rect`.

The inspector is read-only text. A field an author types into needs a caret, a parser and a
commit rule; the keys that change a transform already existed, and what was missing was
being able to see what they did.

### A key that was bound twice

F6 opened the editor *and* cycled shadow quality — one press did both, because the editor
reads the input state and the quality key is a key event. Shadow quality moved to **F4**.

### Acceptance

| Criterion | How |
| --- | --- |
| empty level opens | `--editor --scene-file <new path>`; verified by running the binary |
| editor camera works | `FlyCamera`, unchanged, its own tests |
| entity can be created | click on ground (`a_click_on_empty_ground_places_the_palettes_asset_once`) |
| entity can be selected | in the world, and now in the hierarchy (`a_click_on_a_hierarchy_row_selects_that_object_and_places_nothing`) |
| transform can be modified | drag, `R`, `T` (`r_and_t_turn_and_resize_the_selection`) |
| level can be saved | `CTRL+S` (`saving_writes_the_scene_where_the_status_line_says_it_did`) |
| reopens with the same state | `a_new_empty_level_saves_and_reopens_as_an_empty_level`, `a_file_round_trips_through_the_disk_too` |

## Phase 3 — procedural starter content, as files

The brief asks for starter objects and materials that are *actual assets*, not hardcoded
debug rendering. So they are written to disk as the same kind of file an artist delivers,
and read back by the same importer:

```text
  procedural::starter ──► gltf_export ──► assets/generated/**/x.glb ──► gltf_import ──► ModelAsset
                          material_file ─► assets/generated/materials/x.json + x.png
```

Nothing in the engine special-cases them. That is the property that makes them replaceable:
drop a sculpted `rock_small.glb` over the generated one and every level that placed it is
unchanged.

### The glTF writer (`engine_assets::gltf_export`)

`export_glb(&ModelAsset) -> Vec<u8>`: level-0 geometry (positions, normals, UVs, indices),
every material, and every texture as PNG inside the binary chunk.

Not written: levels of detail, skeletons, clips, tangents. glTF has no LOD concept without a
vendor extension, and a tangent is regenerated by `SubMesh::new` at import — writing one
would store a value the importer recomputes. A generated asset therefore comes back with one
LOD, which is what every artist-delivered `.glb` in this project already has, and it is why
`asset_check` reports `ASSET_LOD_MISSING` as a warning on all of them.

### The starter library (`engine_assets::procedural::starter`)

24 models on a 4 m module, one material each from a set of seven:

```text
rocks       rock_small  rock_medium  boulder_large  rock_spire
cliffs      cliff_straight  cliff_corner  cliff_end  cliff_slope  cliff_overhang  cliff_arch
ground      ground_tile  platform_square  ramp_square
props       stone_cube  pillar  arch  crate_wooden
vegetation  bush_round  tree_simple
caves       cave_entrance  cave_tunnel_straight  cave_tunnel_curve  cave_room_small
water       water_plane
```

The two shapes that decide the library are the ones a heightmap cannot express:

* **`cliff_overhang`** — a back wall with a slab cantilevered 4 m off it, 3.5 m of clear
  space underneath. This is the piece the cliff requirement exists for.
* **the cave pieces** — a floor, two walls and a roof around an empty middle, 3 m of
  headroom and 3 m wide. The cavity is real geometry; a test measures the headroom off the
  vertices rather than off the constants they were built from.

### The materials (`engine_assets::material_file`)

`grass dirt rock cliff sand water simple_test`, each written as `<name>.json` beside
`<name>.png`. The JSON holds what a person edits (colour, roughness, metallic, opacity) and
names the image beside it; the PNG is a 128×128 deterministic noise (or a checker, for
`simple_test`). Water is the only transparent one.

An image inside JSON is a base64 blob nobody can open, and roughness written into a corner of
a PNG is a convention that has to be documented rather than read — hence two files.

### The generator (`asset_gen`)

```text
cargo run --bin asset_gen              write assets/generated/**
cargo run --bin asset_gen -- --list    say what would be written
cargo run --bin asset_gen -- --force   overwrite what is there
```

A crate of its own, `engine-assetgen`, for the reason `engine-assetcheck` is one: nothing a
content tool needs can end up linked into the game.

**An existing file is never overwritten without `--force`.** The first thing that happens to
starter content is that somebody replaces it, and a generator that clobbered whatever it
found would delete an artist's work the next time anybody ran it out of habit. A test covers
exactly that: write, replace a file by hand, run again, assert the replacement survived.

### Verification

* `cargo run --bin asset_gen` → 38 files written.
* `cargo run --bin asset_check -- assets/generated` → **24 assets, 0 failed, 0 errors**,
  32 warnings, all of them the expected three: one LOD, the importer's extra default material
  slot, and "no collision was supplied to the validator".
* Round-trip tests assert every starter asset exports, imports, and comes back with the same
  triangle and texture count.

### `.gitignore`

`assets/` was ignored whole. Two exceptions now:

* `assets/scenes/` is **not** ignored — a level is source, not art. Losing it loses the level.
* `assets/generated/` **is** ignored, explicitly: it is reproducible with one command, so it
  is build output.

## Phase 4 — the recursive asset browser

Three things had to exist before a browser could: a way for a name found on disk to become
an asset id, something that reads the directory, and a panel that lists it.

### An id for a name nobody compiled

`EnvironmentAsset` wraps a `&'static str` — it is `Copy`, compared once per placement and
stored in every object — so a name read from a directory could not become one.
`EnvironmentAsset::interned(name)` looks the name up in a table of names already seen and
leaks it once if it is new. The leak is bounded by the number of distinct assets a run
touches. A name that already has a constant returns that constant, so `interned("pillar")`
and `PILLAR` are the same id.

### An unknown asset name is no longer a refusal

`scene_file` used to refuse a name the compiled library did not have, and the reason was
good: a typo should not become a missing wall the designer thinks is there. But the library
stopped being the set of assets that exist the moment assets came from a directory — a build
compiled last week does not know about a rock added this morning, and refusing to load a
level over that would make every new asset a code change.

The check moved rather than disappeared: whatever resolves an id to a *model* reports the
name it cannot find, and the level shows the "missing asset" marker standing where the object
should be, which is louder than a log line. An empty name is still refused.

### `engine_assets::catalogue`

`AssetCatalogue::scan(root)` walks a directory recursively into folders and entries.

* An asset's **id is its path** with the extension removed:
  `generated/environment/rocks/rock_small`. Unique by construction, says where the file is,
  and is what a scene file stores — a level written on Windows opens anywhere.
* `.glb`/`.gltf` are models, `.png`/`.jpg` are textures, `.json` is a material — or a
  **scene**, if it is under `scenes/`, because both are JSON and only the folder says which.
* A `grass.json` **claims** the `grass.png` beside it: one material, not two rows. A loose
  image with no material file is still listed, because an artist dropping a folder of maps in
  should see them.
* Hidden and tool-owned directories (`.git`, `__pycache__`) are skipped.
* Nothing is opened. A truncated `.glb` is listed and fails when something asks for its model.
* A missing root is an empty catalogue that says so, not an error — a project nobody has put
  anything in yet must still open.
* Folders and assets are sorted, because `read_dir` promises no order and a browser whose
  rows move between two scans of an unchanged directory is one nobody can point at.

### The browser panel

The palette is gone; the left-hand panel is the browser.

```text
ASSETS 25 MODELS
- builtin              the assets compiled into this build, as a folder
    ground_plain
    pillar  …
- generated
  - environment
    + rocks            click a folder row to open or close it
    + cliffs
  + materials          a material row is dimmed: it is content, but not placeable
F FIND   F2 RESCAN
```

* `F` opens the filter and **owns the keyboard while it is open** — `W` types a w rather than
  flying the camera, `X` types an x rather than deleting the selection. A test asserts exactly
  that.
* A filter flattens the tree: after typing `rock` an author wants every rock, not the three
  folders they are spread across. Clearing it puts the tree back, because filtering never
  touches the expansion state.
* `F2` rescans, keeping open folders and the chosen asset — by *name*, so a rescan that
  renumbered the entries does not move the selection to whatever is in that slot now. This is
  what makes "drop a file in `assets/` and it appears" true without restarting the game.
* Built-ins are shown as a `builtin` folder. They are not files, but a browser that hid them
  would show nothing at all in a checkout where nobody has run the generator.
* `TAB` walks placeable rows only.

`engine-editor` gained a dependency on `engine-assets`. It is still device-free and
gameplay-free; what it needs is the answer to "is this file an asset", and that has to be the
same answer the loader gives.

### Verification

* 43 editor tests, 9 catalogue tests, 3 interner tests, whole workspace green, clippy clean.
* Running the editor against the real directory logs:
  `assets holds 25 model(s), 7 material(s), 0 texture(s), 0 level(s)` — the 24 generated
  models plus the one `.glb` that was already in the repository.

### What the tree opens to, and why TAB walks what is not on screen

Two things were wrong the first time an author opened the panel, and both are fixed:

* The `builtin` folder was open, which put nineteen rows of fallback in front of the content
  the project actually has and pushed `generated` off the bottom of the panel. It starts
  closed now.
* Disk folders opened only to depth 1, so `generated` was visible and `environment` — the
  folder that holds `rocks`, `caves` and `cliffs` — was not. The rule is now **open to depth
  2, plus any folder holding nothing but folders**. A content directory is two or three levels
  of grouping over one level of files, so this shows the categories and leaves the leaves
  closed. The opening view of the real directory is nineteen rows:

```text
+ builtin
firemage_1
- generated
    - environment
        + caves      + cliffs   + ground
        + props      + rocks    + vegetation   + water
    - materials
        cliff [MATERIAL]  dirt [MATERIAL]  …
```

`TAB` walked the *visible* rows, which meant it did nothing at all in a browser whose folders
were closed — which is now how the browser starts. It walks every placeable asset that
exists, in a stable order that does not change when a folder opens, and opens the folders down
to whatever it lands on. A test asserts TAB works with everything collapsed, and the editor
test that presses TAB until it reaches a pillar is now bounded, so a TAB that stops moving
fails the test rather than hanging it.

### Not yet

Placing a browser asset writes an interned id into the scene, and nothing uploads a model for
it yet, so it draws as the missing-asset marker. Uploading discovered models, and deriving
collision for them, is Phase 5.
