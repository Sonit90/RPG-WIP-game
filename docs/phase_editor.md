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

## Phase 5 — placing what the browser found

The browser could name an asset and the level could store it, but nothing turned that name
into geometry: a placed rock drew nothing at all, and so did a level file that named one.

### A model is uploaded when something asks for it

```text
"pillar"                                  -> the compiled library
"generated/environment/rocks/rock_small"  -> assets/<id>.glb, through the importer
neither                                   -> the grey "missing asset" marker
```

`upload_missing_models` runs over a scene's distinct assets and uploads whatever is not
uploaded already — at load, and again whenever the editor reports a change. The library still
goes up in one pass at startup, because a level made of it is the common case; everything
else arrives this way. An asset is uploaded once and then shares its handles with every
placement of it, so the instancing story is unchanged, and a directory of two hundred assets
costs nothing until one of them is actually placed.

A file that will not load is reported once and replaced by the marker. An object that is
simply not drawn is indistinguishable from one nobody placed — that was exactly the bug.

### What a placed file blocks

A built-in has an authored answer that no bounding box could produce: a pine collides with its
trunk and not its canopy. A file has no author to ask, so the editor imports it once, takes a
box around its level-0 geometry, and writes that into the level. A model with no thickness in
some axis — the water plane — becomes `EnvironmentCollision::None` rather than a zero-extent
box that every query tests and none can touch.

The collision is stored **in the level**, not derived at load, so a placement keeps what it
was placed with even if the rule changes later.

**Caves and overhangs are wrong under this rule**, deliberately and visibly: a tunnel wrapped
in its own bounding box is a solid block. A wrong box an author can see and edit beats no box
at all, and Phase 9 is where a cave piece gets collision describing its cavity.

### Verification

A level naming one built-in, one generated file and one name that answers to nothing:

```text
environment 'probe': 2 model(s) uploaded from disk
environment: nothing on disk answers to '…/does_not_exist'; drawing the missing marker
environment 'probe': 4 objects, 4 render entities, 20 models uploaded
```

Four objects, four render entities — before this, three of them drew nothing.

## Phase 5b — the transform toolset

Placement without a way to adjust it is a level built by luck. The verbs that existed — drag,
`R`, `T`, `X`, `C` — acted on one object; they now act on a selection, and there are enough of
them to lay out a blockout.

### A selection is a list

`selection: Option<usize>` became `selected: Vec<usize>`, with the **last** entry as the
primary one — the thing the inspector describes, because the thing an author just clicked is
the thing they are looking at. `Editor::selection()` still answers the primary one, so nothing
that only wanted "what is selected" changed.

* `CTRL`+click **toggles** membership, in the world and in the hierarchy: control-clicking a
  selected object takes it out again, which is the one gesture every editor agrees on.
* `CTRL+A` takes everything, `ESC` drops it.
* Every verb acts on the whole selection: `X` deletes it (highest index first, so removing one
  does not move the next one under it), `C` duplicates it and selects **the copies**, `T`
  scales each, `R` turns each about its own origin.
* A drag moves everything by the delta the dragged object moved by, so a group keeps its
  shape instead of collapsing onto the cursor.
* Undo is unchanged and still one step: the whole object list is the undo unit, so a
  twelve-object nudge comes back in one press.

### Nudging, and what "along X" means

Arrow keys move the selection by the grid step — the same step a drag snaps to, so nudging
and dragging agree about what "one over" means. With the grid off it is 10 cm, which is the
size of an adjustment somebody makes with a key rather than with the mouse. `SHIFT`+up/down
moves in Y; this engine's input state has no page keys to spend on it.

`L` switches between world and local axes. In local space the step runs along the object's own
facing, so nudging a yawed wall moves it *along the wall*. Rotation is yaw-only in this
engine, so the local basis is one sine and one cosine rather than a matrix.

### Rotation snapping

`SHIFT+G` cycles the rotation step: 90°, 45°, 22.5°, 15°, 5° — coarse first, because a
blockout is built out of right angles and an author reaches for 15° when they specifically
want it.

`R` **snaps** rather than adds: an object sitting at 7° stepped by 22.5 lands on 22.5, not on
29.5. That is what makes a rotation snap mean "aligned with everything else" rather than
"turned by the same amount as everything else".

### Reset

`0` restores rotation and scale and leaves the position alone. Where a thing stands is what an
author placed; how it is turned and how big it is are what they have been fiddling with.

### On screen

The inspector carries a line that is always visible — `GRID 1.0  ROT 22.5  WORLD` — because an
author who cannot see the grid step cannot predict what an arrow key will do. The hierarchy
highlights every selected row and outlines only the primary one, and its heading shows `SEL n`
when there is more than one.

## Phase 6 — terrain

The ground was a number: `EnvironmentScene::ground_height`, one float, flat everywhere. It is
now optionally a heightmap, and the level can have hills, valleys, slopes and plateaus.

### The division that makes a heightmap safe

A heightmap has one height per point, so it cannot express a cave mouth, an arch, or a ledge
with space under it. That is fine, because it is never asked to: **anything that overhangs is
placed geometry standing on the terrain**, which is what `EnvironmentObject` has always been.
The terrain does not know those exist.

A player walks under a cliff because the cliff is a mesh standing on the ground, not because
the ground was bent into something a heightmap cannot hold. The terrain is never asked to do
the thing it cannot do, so it never has to become a voxel field to keep its promises.

### Three descriptions, one set of numbers

```text
                 ┌─► terrain_model  (engine-assets)      the mesh, one render entity
Terrain heights ─┼─► Heightfield    (engine-collision)   the surface, one collision shape
                 └─► height_at      (engine-environment) where the ground is, for anything
```

The mesh and the collision surface split every cell along the same diagonal. That is asserted
in `engine-presentation/tests/terrain_agreement.rs` — the only crate that can see both, since
`engine-assets` has no collision world and `engine-collision` has no meshes. A terrain split
two different ways would put a ridge where the picture shows a valley, half a cell wide, over
the whole level: invisible in a screenshot and impossible to explain from one.

### A new collision shape, and why it earned its place

`engine-collision` had `Plane`, `Aabb` and `Obb`. A 129×129 terrain as boxes is 32 768 static
shapes, all of them in the broad phase, and a hill made of boxes is a staircase a character
catches on. So `Shape::Heightfield` is one shape that tests the cells under the capsule — the
cost depends on the capsule's size, not the terrain's — using capsule-vs-triangle with the
triangle's own normal, so a character on a slope is pushed along the surface instead of being
nudged sideways at every triangle edge.

`Shape` lost `Copy` for this: every other variant is a handful of floats and this one owns a
`Vec`. The places that copied a shape now borrow it, which is what they wanted anyway.

The field is a **surface, not a solid**: something far below it is falling through empty
space, and the level's floor plane is what catches it. The plane stays under the terrain for
exactly that reason, and because the terrain covers part of the playable square while the
plane holds up whatever stands on the rest.

### The brush

`B` enters and leaves sculpt mode. While sculpting the pointer shapes the ground and `TAB`
cycles the brush — the same "next thing" `TAB` means in placement mode.

| | |
|---|---|
| drag | apply the brush |
| `TAB` | RAISE → LOWER → FLATTEN → SMOOTH |
| `1` / `2` | brush size, multiplied and divided by the same step so the keys are inverses |
| `3` / `4` | brush strength |
| `Z` | undo the whole stroke |

Flatten takes its target from the height under the middle of the brush, so a plateau is a
flatten stroke where the plateau should be rather than a number somebody types. The falloff is
a smoothstep: a stroke with a hard edge leaves a cliff behind it, and cliffs are not this
system's job.

A level with no terrain gets one the first time somebody paints on it, sized to the level's own
playable square.

### Three bugs the tests caught

* **`B` cycled the brush and left the mode in one press** — the brush answered the key and the
  editor answered it again. A key the brush consumes is no longer passed on, and `B` is now
  deliberately not one of the brush's keys: a mode key that means something else once you are
  inside the mode is a key nobody can find their way out with.
* **`1` and `2` were not inverses** (×0.75 then ×1.25 is 0.9375), so the brush drifted smaller
  every time an author changed their mind.
* **Undo did not restore the terrain.** The undo stack held `Vec<EnvironmentObject>` — right
  while placements were the only thing an author could change. It holds a `Snapshot` of the
  placements *and* the ground now, and one stroke is still one undo however many frames the
  drag lasted, because the snapshot is taken when the stroke starts.

### Cost

A brush stroke reports `TerrainChanged`, which rebuilds one mesh and one collision shape.
`SceneChanged` respawns a few hundred render entities; paying that on every frame of a drag is
what the separate outcome exists to avoid.

### Verification

A 41×41 sculpted level: `1 objects, 0 collision shapes, and terrain` →
`terrain: 41 points a side, 3200 triangles`. Whole workspace green, clippy clean, and the
non-editor build still compiles without any of it.

## Phase 7 — modular cliffs, and walking underneath one

The pieces existed since Phase 3. What did not was collision that describes them: a placed
`.glb` got one box around its geometry, so the cliff overhang was a wall and the cave tunnel
was a rock. The level looked right in a screenshot and was unwalkable in the game.

### Compound collision, and why the boxes live in the scene

`EnvironmentCollision` gained `Compound(u16)` — an index into
`EnvironmentScene::collision_shapes`.

One box is the overwhelmingly common case, and an `EnvironmentObject` is 44 bytes because of
it. An overhang needs two boxes, a tunnel four, a room ten; a variant big enough for a room
would put two hundred bytes into every rock in the level to hold nothing. So the boxes live in
the scene and a placement names one. Two placements of the same cliff piece share an entry,
the object stays `Copy`, and the scene file writes the table once however many placements
refer to it.

`EnvironmentScene::world_boxes(object)` is the one place the three kinds become the same
thing — nothing, one box, or several. Gameplay walks that and never asks which it was, which
is how compound shapes arrived without a second code path in the collision build.

### Where the boxes come from: a sidecar

```text
cliff_overhang.glb              the geometry
cliff_overhang.collision.json   the wall and the slab — and the gap between them
```

Three places this could have lived. Inside the `.glb` as a node-name convention, which every
exporter must honour and every artist must remember. In the engine as a table keyed by asset
name, which breaks the moment somebody adds an asset the build was not compiled with — the
exact thing the asset catalogue exists to allow. Or beside the file.

Beside the file wins: an artist delivering `arch.glb` can deliver `arch.collision.json` with
it, the editor reads it when the asset is placed, and an asset without one keeps the automatic
bounding box. No engine change, no exporter convention, nothing to remember for the assets
that do not need it.

The sidecar is an **authoring input**, not something the game loads. What ends up in the level
is the boxes, written into the scene file — so a level built today keeps working if the
sidecar changes tomorrow, and the game has no idea these files exist.

A sidecar that is present and will not parse is reported and the asset falls back to its
bounds: the asset stays placeable and the log says why it came out solid. Silently falling
back would hide the typo behind a cave nobody can walk into.

### What is authored, and what is not

Six pieces carry sidecars: `cliff_overhang`, `cliff_arch`, `cave_entrance`,
`cave_tunnel_straight`, `cave_tunnel_curve`, `cave_room_small`. Rocks, crates and pillars do
not — a box around the geometry is right for a solid thing, and authoring one by hand would be
work with no result.

Deriving the cavity from the mesh instead would mean convex decomposition: a large piece of
machinery to produce a result somebody can write down in six lines.

### The acceptance test

`engine-presentation/tests/walk_under_and_through.rs` walks a character through the **real
controller** in a world built by the **real level loader**:

| Test | What it proves |
| --- | --- |
| walks under the overhang | the space beneath the slab is walkable |
| the wall behind it still stops | an overhang you walk *through* is not an overhang |
| walks the length of a tunnel | the cavity is real, end to end |
| the tunnel wall is solid | the shell is a shell |
| a bounding box **fails** the walk-under | if this ever passes, compound collision has stopped being applied and the tests above are passing for the wrong reason |
| the shape survives the scene file | the level is what the game loads |

That last one is the guard that matters: it asserts the *old* behaviour still fails, so the
other five cannot quietly start passing for a reason that has nothing to do with the feature.

### Verification

106 test binaries green, clippy clean, `asset_check` still 24 assets / 0 errors, and
`asset_gen` writes six sidecars beside the six pieces that need them.

## Phase 8 — water

The third thing a level *has*, after its terrain and its lights, and not something it places.

### Why it is not a placement

A placement is an asset id and a transform; the engine resolves the id to a mesh and draws it.
Water has no asset — it has a size somebody dragged out, a height somebody chose, and one
question gameplay asks of it. As a placement, "is the player in water" would have to be
answered by searching the object list for meshes that happen to be transparent, and a pond
would be a rock that happened to be flat and blue.

So `EnvironmentScene` gained `water: Vec<WaterBody>` beside its `lights` and its `terrain`, and
`WaterBody` is a rectangle with a yaw, a surface height and a depth:

```text
        yaw
         ↻                     ~~~~~~~~~~~~~~~~~~~~~  surface, at center.y
   ┌───────────┐               │                   │
   │     •     │  half_extents │      depth        │
   └───────────┘               └───────────────────┘  the volume the query answers about
     seen in plan                    seen from the side
```

`is_submerged(point)` is the one question with one answer, in the spirit of `ground_at`: a
caller never has to know how many bodies a level has or how each is shaped. **Nothing consumes
it yet** — the lifecycle phases can when they need a second death trigger, and building one
here would put the lifecycle in the wrong phase.

The depth is not decoration. Without it, "under water" means "below this height anywhere in
the level", which is true inside the hill next to the lake.

### The one architectural change: a tagged selection

The editor's selection was `Vec<usize>` into `scene.objects`. It is now `Vec<Selected>`:

```rust
pub enum Selected {
    Object(usize),
    Water(usize),
}
```

That reaches the hierarchy panel, the inspector, `reveal_selection`, every verb in
`update_keys`, the drag and `prune_selection`. It was worth doing as a shape rather than as a
special case for water, because a spawn point in Phase 10 wants exactly the same thing: click
to select, drag to move, `X` to delete, `Z` to take back.

**A selection holds one kind at a time.** Selecting a body of water clears a selection of
objects and the reverse; `CTRL+A` takes every object and no water. A rule an author can predict
beats a general one nobody can — `T` over a mixed selection would mean "scale these and resize
those", and the undo of that is a sentence rather than a gesture.

The assertion the refactor lives or dies on is that the existing object tests pass unchanged,
and they do: multi-select, group drag, duplicate, delete-highest-first, reset. Two new tests
cover the boundary — an object and a body of water each selected in turn, with `X` deleting
exactly the one that is, in both directions.

### The editor

| Key | What it does to a body of water |
| --- | --- |
| `V` | pours one where the cursor is: 8 m square, 2 m deep, at the height of the ground under it |
| drag | moves it |
| arrows | nudge by the grid step, along the world or along its own yaw |
| `T` / `SHIFT+T` | grow / shrink its half-extents, never below a quarter of a metre |
| `R` / `SHIFT+R` | turn it, snapped to the rotation step |
| `0` | straighten it |
| `X` | delete it |
| `Z` | undo, the pour included |

`SHIFT+T` stops at a quarter of a metre because a body with a zero extent is a volume nothing
can be inside — and a level the scene loader would refuse to read back. The editor must not be
able to author a file it cannot open.

`Snapshot` grew a third field, so undo carries the water the way Phase 6 taught it to carry the
terrain. The hierarchy lists the placements and then the water, one row index over both lists,
so the number beside a placement is still its index in the file. The inspector shows a body's
surface height, size, yaw and depth — an author who cannot see the depth cannot tell a puddle
from a lake in plan view.

Picking is against the **volume**, not the surface, so a pond can be clicked from underneath as
well as from above; `pick::under_ray` answers over both lists at once, because "which is
nearer, the rock or the pond" is precisely what a click asks and precisely what two separate
answers cannot settle.

### Rendering, and what was not built

One transparent quad per body, through the pass that already existed (`docs/transparency.md`).
The yaw is **not** baked into the mesh — it is on the render entity's transform, the way every
placement's is, so a body that is turned reuses the quad it has. UVs are tiled by world metres
using the terrain's `TEXTURE_METRES`, so a pond and a lake have the same texel density.

`EditorOutcome::WaterChanged` exists for the reason `TerrainChanged` does: `SceneChanged`
re-places every object in the level, and paying that on every frame of a drag that is resizing
one quad is exactly the cost the separate outcome avoids.

Not built, on purpose: swimming, buoyancy, drowning, waves, reflection, refraction, underwater
fog, a material per body, and any collision at all. `engine-collision`'s layer bitmask has room
for a `WATER` layer if water ever needs to be a queryable volume in the physics world; it is
not needed for `is_submerged`, which reads the scene directly.

### The acceptance test

`engine-presentation/tests/water_is_not_a_wall.rs` moves a character through the **real
controller** in a world built by the **real level loader**:

| Test | What it proves |
| --- | --- |
| walks straight through a pond | water adds no collision — asserted by moving a character, not by counting shapes |
| the level says wet exactly while the character is in it | the query and where the character actually stands agree |
| a rock in the pond still stops it | water adds no collision and takes none away either |

### A gap this phase did not dig, and one it did not fill

Water is serialized from the start: a `WaterRecord` in `scene_file.rs`, optional and skipped
when empty, validated on load with the error naming `water[n]`.

`EnvironmentScene::lights` is **still not serialized**. There is no lights record, so a level
authored in the editor loses every brazier's local light when it is saved. That is a real bug
and it is not this phase's — fixing it means deciding how a light is *authored*, which is a
phase of its own. It is written down here so the next person finds it.

### Verification

`assets/scenes/water_probe.json` is a level with two bodies at different heights and yaws.
Loaded headless it reports:

```text
environment 'water_probe': 3 objects, 2 collision shapes
environment 'water_probe': 2 water bodies
water: 2 bodies
```

Workspace tests green, clippy clean, and both builds compile — `--features editor` and the
shipping one, which still has no editor in it.
