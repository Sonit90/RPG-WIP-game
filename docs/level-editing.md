# Level editing

How a level is built, what is in one, and the development controls that go with it.

The record of *why* each piece is shaped the way it is lives in `docs/phase_editor.md`. This
file is the reference: what to press, what a level contains, and where the files go.

---

## 1. Opening the editor

```bash
cargo run -p engine-app --features editor -- --scene-file assets/scenes/showcase.json
```

`F6` enters and leaves. A shipping build (`--no-default-features`, no `editor` feature)
contains none of the editor: no palette, no fly camera, no scene writer.

The editor edits **data**. It never touches a mesh, a device or a collision world, which is
what lets every one of its verbs be tested with no window open.

### The screen

```text
┌ File Edit View Help │ tools │ place sculpt │ World Active │ snap grid rotation module │ undo redo save ─ ⌨ 📈 Play ┐
├──────────────┬─────────────────────────────────────────────────────┬───────────────┤
│ Assets       │ Perspective - World                  Grid 1 m - Snap │ Hierarchy     │
│ [search]     │                                                     │ [search]      │
│ tree / tiles │                     viewport                        ├───────────────┤
│              │ Rock Small                                    X Y Z │ Inspector     │
├──────────────┴─────────────────────────────────────────────────────┴───────────────┤
│ tool │ selection │ snap │ what just happened / what you can do next   X Y Z │ file ● │
└────────────────────────────────────────────────────────────────────────────────────┘
```

* **Toolbar** — the menus, the Select / Move / Rotate / Scale tools, placing and sculpting,
  world or local axes, the pivot (the active object or the centre of the selection), snapping,
  the grid and rotation steps, module snap, undo, redo, save, the shortcuts window, the debug
  overlay, and **Play**. Every control has a tooltip naming its key.
* **Assets** — a tree or tiles (the two buttons in its header), a search field, type icons and
  readable names. Drag an asset into the viewport to place it — a footprint shows where it will
  land — or double-click it to place one in front of the camera. Double-click a level to open it.
* **Hierarchy** — the level grouped by kind (ground, cliffs, caves, rocks, vegetation, props,
  water, spawn points, lights). Groups fold; the eye and lock on a row hide it or make clicks in
  the viewport pass through it. Click, `CTRL`-click, `SHIFT`-click for a range, double-click to
  frame. Right-click for actions.
* **Inspector** — folding sections: Transform (numbers you can type, step with the wheel, or
  scrub by dragging the coloured X/Y/Z tag; reset buttons for rotation and scale), then Asset,
  Water, Light or Spawn, and Editor (hidden and locked).
* **Viewport** — only what matters: the view and snap chips at the top, what is selected at the
  bottom left, the camera's axes at the bottom right, and a banner while placing or sculpting.
  The object under the pointer is outlined faintly before you click it; a drag shows how far it
  has moved, snapped.
* **Status bar** — the tool, the selection, snapping, the last thing that happened (for a few
  seconds) or a hint for what you can do next, the ground under the pointer, and whether the file
  is saved.
* **Right-click** an object for Focus, Duplicate, Copy, Paste, Delete, Reset Transform, Drop to
  Ground, Select All of This Asset, Show in Browser, Hide and Lock; right-click empty ground to
  paste or add a light, water or a spawn point there.
* **Splitters** between the panels drag to resize them; *View → Reset Panel Layout* puts them
  back. The editor lays itself out larger on tall windows and follows the interface-scale
  setting.
* **F1** opens the keyboard shortcuts; **F3** the debug overlay (frame rate, counts, history,
  camera, the autosave path) together with the level's collision drawn in the world.

While the editor is open the game's own HUD is not drawn, and gameplay keys (the dev interrupt on
`H`, the kill key) are ignored; the rendering switches (`F4`, `F5`, `F8`, `F9`, `F10`) still work.

---

## 2. The controls

Grouped by what is being built rather than by key, which is how the editor prints them itself.

| Group | Keys |
| --- | --- |
| **Placing** | drag a browser row into the world · double-click a row to place in front of the camera · `N` placing mode: every `LMB` in the world puts one down. A drop lands on the terrain, or on top of the solid thing under the pointer |
| **Assets** | `TAB` next asset · `CTRL+F` find (in the panel under the pointer) · `F2` rescan |
| **Terrain** | `B` sculpt · `TAB` brush mode · `1` `2` brush size · `3` `4` strength |
| **Caves** | `M` module snap — a 4 m grid and quarter turns |
| **Put** | `V` water · `P` spawn point · `I` light — each under the cursor |
| **Selecting** | `LMB` on something selects it · double-click frames it · drag on empty ground draws a selection box · `CTRL+LMB` adds · `SHIFT+LMB` in the hierarchy selects a range · `CTRL+A` every object · `ESC` drops it, and leaves placing mode · right-click for actions |
| **Gizmo** | `1` move · `2` rotate · `3` scale · the Select tool on the toolbar hides the handles · hold `CTRL` while dragging to turn snapping off |
| **Editing** | arrows nudge (`SHIFT` for up/down) · `R` yaw · `T` size · `0` reset · `L` local axes · `X`/`DELETE` delete · `CTRL+D` duplicate · `CTRL+C`/`CTRL+V` copy and paste (under the pointer) · `CTRL+G` drop onto what is below · click an inspector number to type it, use the wheel over it, or drag its tag |
| **Showing** | `H` hide the selection · `SHIFT+H` show everything · `K` lock the selection (clicks in the world pass through it) |
| **Undo** | `CTRL+Z` undo · `CTRL+Y` or `CTRL+SHIFT+Z` redo |
| **Grid** | `G` step · `SHIFT+G` rotation step |
| **View** | right-drag look · `WASD`/`QE` fly (`SHIFT` faster) · `ALT`+left-drag orbit · wheel forward and back · `F`/`HOME` frame the selection (the whole level with nothing selected) · double-click a hierarchy row to frame it |
| **File** | `CTRL+S` save · `CTRL+SHIFT+S` save as · double-click a level in the browser to open it · `F1` keyboard shortcuts · `F3` debug overlay · `F6` back to playing |

Every edit is counted against the last save: the status bar says `Unsaved`, and the save button
carries a dot, while they differ. Opening another level over unsaved work asks whether to save,
discard or stay. Unsaved work is written to `<level>.autosave.json` beside the level every two
minutes, on `F6`, and when the game closes, and a successful save removes the autosave. Undo
survives `F6`, so walking through the level and coming back to fix it keeps your history.

Cliffs, rocks and trees have no controls of their own: they are **assets**, placed from the
browser like everything else. That is the point — a piece added to `assets/` tomorrow is
placed the same way, with no code change.

### Placing and selecting are different gestures

One mouse action means one thing. The order the editor resolves a press in is:

1. a drag on a gizmo arm **transforms** what is selected;
2. a click on something in the world **selects** it — it never places anything;
3. a drag from a browser row into the world **places** one where it is dropped;
4. a double click on a browser row **places** one in front of the camera;
5. a drag that starts on empty ground **draws a selection box**;
6. anything else belongs to the camera (right-drag to look, `WASD`/`QE` to fly).

A click on empty ground used to place the browser's asset, which meant a click that missed the
rock you were aiming at left a second rock behind it. Placing is now something you do *from the
browser*, deliberately.

### Placing mode

`N` suspends rule 2. While it is on, **every left click in the world puts an object down** —
on top of whatever is already there — and nothing selects, drags or moves. Laying out a row of
pillars is a dozen clicks rather than a dozen drags out of the browser, and a click that lands
on the pillar you just placed does not turn into "you meant to pick that up".

The gizmo disappears while the mode is on, because a handle you cannot grab is a lie about
what a click will do, and the mode badge along the top reads `PLACING`. `N` again
or `ESC` gives the left button back to selection. It is mutually exclusive with the terrain
brush: `B` turns it off and it turns `B` off, since both want every press in the viewport.

Everything that is not a click still works: `TAB` picks the next asset, `X` deletes what was
just placed, `CTRL+Z` takes it back, the arrows nudge it, `G` still decides the grid it lands on.

### The gizmo

Whatever is selected carries a three-armed gizmo — red `X`, green `Y`, blue `Z` — standing on
its origin. Dragging an arm moves the selection along that axis and no other; everything
selected moves with it. The arm you are about to grab lights up, the arms are drawn over the
level with a dark edge so they read against any background, and a drag keeps the pointer even
if it passes over a panel. The gizmo grows with distance so it is the same size on screen
whether you are two metres from a rock or two hundred.

The grid applies: an `X` or `Z` drag lands on the grid step `G` chose, and a `Y` drag snaps
height to the same step — which is how a stack of blockout pieces lands on the same levels as
the floor plan.

`2` turns the gizmo into a ring: dragging round it turns the selection about its primary, in
rotation steps. `3` makes it a scale gizmo: its arms run along the object's *own* axes, so
stretching a turned wall along its length is its `X` arm, and the square in the middle scales
all three at once. `CTRL` held during any drag turns the snapping off. `R` and `T` still work.

### A selection you have turned away from

If the camera moves so the selected object is off screen, the selection is not hidden: a
**striped amber silhouette** appears at the edge of the window in the direction the object
lies, labelled with what it is, sized as the object would be if you could see it. Turn back
and it walks around the edge to meet the object's own cyan outline as it comes into view.

### The selection holds one kind at a time

A level contains placements, bodies of water, spawn points and lights, and the selection is
tagged so a verb reaches exactly one kind. A selection box that catches several kinds keeps the
first in hierarchy order — placements first. Selecting a body of water clears a selection of objects and the
reverse; `CTRL+A` takes every object and no water. A rule an author can predict beats a general
one nobody can — `T` over a mixed selection would mean "scale these and resize those".

### The ground, pits, and falling out of the world

A level that brings a terrain has **no floor plane under it**. The terrain is the ground, all
of it: sculpt a pit to -6 m and a character walks down into it. It used to be that an
infinite plane sat at y = 0 beneath everything, so a hill worked and a pit was a hole with a
lid on it at zero.

The plane is kept for a level with no terrain at all, and for one whose terrain does not reach
its own walls - a ring of level with nothing under it is a worse bug than a covered pit. A
terrain the editor creates is sized to the playable square, so this only affects levels
authored by hand.

What the plane used to do, besides covering pits, was catch somebody who fell off the edge.
That is now a clock: **ten seconds below the level's deepest ground, less 25 m, and the player
dies** - through the ordinary death path, so the usual respawn brings them back at the spawn
point. Standing at the bottom of the deepest pit you can dig is never counted as falling, and
the clock resets the moment you are above the line again.

### Module snap

Choosing a cave piece switches the grid to the 4 m module and the rotation step to quarter
turns, because modular pieces meet on a face: two tunnels four metres apart on a quarter-turn
grid join, and two on a 22.5° grid leave a gap only visible from inside the cave.

Pressing `M` takes the decision off the editor for the rest of the session, in both directions.
A piece counts as modular by its folder or its name (`caves/`, or a name starting `cave_`), so
a piece added to that folder gets the grid without a code change.

---

## 3. What a level contains

A scene file is JSON. Every key below except `objects` is optional and is written only when the
level has one, so a file written before a feature existed still loads.

```jsonc
{
  "version": 1,
  "name": "showcase",
  "half_extent": 60.0,        // how far from the origin a character may go
  "ground_height": 0.0,       // the flat floor under everything without terrain
  "spawns": { ... },          // the layout's own start points, from before spawn points
  "lighting": { ... },        // sun, ambient, sky
  "objects": [ ... ],         // every placement: asset name, position, yaw, scale, collision
  "collision_shapes": [ ... ],// compound shapes the objects name by index
  "terrain": { ... },         // origin, cell size, resolution, heights
  "water": [ ... ],           // centre, half extents, yaw, depth
  "spawn_points": [ ... ],    // position, yaw
  "lights": [ ... ]           // position, colour, intensity, range; a cone for a spotlight
}
```

* **Terrain** is a heightmap: the broad shape of the ground and nothing that overhangs.
* **Cliffs and caves** are placed geometry with *authored* collision, so a player walks under an
  overhang and through a tunnel. See §4.
* **Water** is a rectangle with a yaw, a surface height and a depth. It is drawn and it answers
  `is_submerged`. It adds no collision at all.
* **Spawn points** are a position and a yaw. The **first** one is where the player starts; a
  level with none falls back to `spawns.player`, which is what every level built before spawn
  points existed uses.

Nothing here is required. An empty level is a valid level.

* **Lights** are a position, a colour, an intensity and a range, plus a direction and a cone
  for a spotlight. The editor puts a warm point light with `I` and draws a diamond where each
  one is, since a light has no mesh.

When the editor saves, compound collision shapes no placement uses any more are left out of the
file.

---

## 4. Assets

```text
assets/
├── generated/            written by asset_gen; regenerate, do not hand-edit
│   ├── environment/
│   │   ├── caves/        cave_entrance, cave_tunnel_straight, cave_tunnel_curve,
│   │   │                 cave_junction, cave_room_small, cave_room_large, cave_exit
│   │   ├── cliffs/       cliff_straight, cliff_corner, cliff_end, cliff_slope,
│   │   │                 cliff_overhang, cliff_arch
│   │   ├── ground/       ground_tile, platform_square, ramp_square
│   │   ├── props/        stone_cube, pillar, arch, crate_wooden
│   │   ├── rocks/        rock_small, rock_medium, boulder_large, rock_spire
│   │   ├── vegetation/   bush_round, tree_simple
│   │   └── water/        water_plane
│   └── materials/        grass, dirt, rock, cliff, sand, water, simple_test
└── scenes/               level files
```

The browser reads this directory **recursively**, when the editor is opened. Folders expand and
collapse, `CTRL+F` filters, `F2` rescans. Arbitrary nesting works; the tree above is a convention,
not a rule. A file that cannot be read is reported and the rest of the scan continues, and a
missing `assets/` directory is not an error — it is a project nobody has put anything in yet.

**Adding an asset needs no code change.** Drop a `.glb` under `assets/`, press `F2`, place it.
Asset ids are interned by name, so a level can name a file this build was never compiled with.

Materials, textures and levels appear in the browser dimmed: they are content, and an author
looking for them has to be able to see that they arrived, but they are not placeable.

### Collision sidecars

A placed `.glb` gets one box around its geometry, which is right for a rock and the *worst*
possible answer for a tunnel — it fills the cavity the piece exists to provide. So the pieces
whose shape is their empty space carry a sidecar:

```text
cave_overhang.glb              the geometry
cave_overhang.collision.json   the boxes it actually blocks
```

The sidecar is an **authoring input**. The editor reads it when the asset is placed and writes
the boxes into the level, so the game never loads these files and a level built today keeps
working if the sidecar changes tomorrow.

### Regenerating

```bash
cargo run -p engine-assetgen --bin asset_gen     # the starter models, materials and sidecars
cargo run -p engine-assetgen --bin level_gen     # assets/scenes/showcase.json
```

Neither is linked into the game. What ships is the files they write.

---

## 5. Development controls

Not gameplay. Nothing in the game binds these, and they are listed here so that a key which
looks like it does nothing can be found.

| Key | What it does |
| --- | --- |
| `F3` | Collision debug view |
| `F4` | Cycle shadow quality |
| `F5` | Developer overlay |
| `F6` | Enter / leave the level editor (`editor` feature only) |
| **`F7`** | **Kill the player**, for testing death and respawn |
| `F8` | Audio off and on |
| `F9` | Wireframe |
| `F10` | Cycle the shadow and material debug views |
| `H` | Interrupt the target's cast |

`F7` goes through the ordinary damage path — the same one a monster takes — so what follows is
the shipping death and the shipping respawn rather than a second path only the debug key can
reach. Pressing it while the player is already dead does nothing.

---

## 6. Death and respawn

```text
   F7 or any death
        ↓
   death, 3 seconds
        ↓
   respawn at the level's first spawn point, at full health
        ↓
   "Cannot be respawned", 20 seconds
        ↓
   normal respawns available again
```

The editor decides **where**. The runtime decides **when** and **how**.

* The player respawns at the level's first spawn point, facing the way it points. A level with
  no spawn point respawns them at `spawns.player`; a missing spawn point is never an error.
* After a respawn the player carries **`Cannot be respawned`** for exactly **20 seconds**. It is
  an ordinary status effect — the same system burns and heals over time use — so it appears on
  the player frame with a countdown, and nothing in the gameplay code branches on it specially.
  The interface draws it uppercase, `CANNOT BE RESPAWNED`, like every other status label.
* **Dying during the protection does not respawn the player.** The respawn is *deferred*, not
  refused: they come back the moment the protection expires.
* Holding `F7` down produces one respawn per protection window, not one per press, because
  every death after the first falls inside the protection the first respawn started.
* An encounter reset clears the protection and any owed respawn. Twenty seconds of a previous
  attempt leaking into the next one would be a player who cannot respawn in a fight they have
  only just started.

The clock lives in `engine_game::sim::respawn` and the status is its published view — one
authority, one number. It is there rather than in the status because statuses do not tick on a
corpse, and this one has to run while the player is dead.

---

## 7. The showcase level

`assets/scenes/showcase.json` has one of everything: sculpted ground with a plateau and hills,
a cliff run with an overhang to walk under, a cave that can be entered at one end and left at
the other, a lake in a hollow, scattered rocks and trees, and a spawn point.

```bash
cargo run -p engine-app --features editor -- --scene-file assets/scenes/showcase.json
```

It is a test fixture that happens to be playable, not a designed level. Nothing in the engine
depends on it, and `crates/engine-presentation/tests/showcase_level.rs` reads the file off disk
and walks a character through it — so if the level on disk breaks, the test says so.
