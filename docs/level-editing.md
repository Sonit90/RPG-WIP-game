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

---

## 2. The controls

Grouped by what is being built rather than by key, which is how the editor prints them itself.

| Group | Keys |
| --- | --- |
| **Placing** | `N` placing mode: every `LMB` in the world puts one down · drag a browser row into the world · double-click a row to place in front of the camera |
| **Assets** | `TAB` next asset · `CTRL+LMB` add to selection · `F` find · `F2` rescan |
| **Terrain** | `B` sculpt · `TAB` brush mode · `1` `2` brush size · `3` `4` strength |
| **Caves** | `M` module snap — a 4 m grid and quarter turns |
| **Water** | `V` pour a body under the cursor |
| **Spawn points** | `P` put one down · `R` turns it |
| **Selecting** | `LMB` on an object selects it · `LMB` on empty ground drops the selection · `ESC` drops it, and leaves placing mode |
| **Moving** | drag the `X` `Y` `Z` arms of the gizmo · arrows nudge · `R` yaw · `T` size · `0` reset · `X` delete · `C` copy · `Z` undo · `L` local axes |
| **Grid** | `G` step · `SHIFT+G` rotation step · `CTRL+A` every object |
| **File** | `CTRL+S` save · `F6` back to playing |

Cliffs, rocks and trees have no controls of their own: they are **assets**, placed from the
browser like everything else. That is the point — a piece added to `assets/` tomorrow is
placed the same way, with no code change.

### Placing and selecting are different gestures

One mouse action means one thing. The order the editor resolves a press in is:

1. a drag on a gizmo arm **transforms** what is selected;
2. a click on something in the world **selects** it — it never places anything;
3. a drag from a browser row into the world **places** one where it is dropped;
4. a double click on a browser row **places** one in front of the camera;
5. anything else belongs to the camera (right-drag to look, `WASD`/`QE` to fly).

A click on empty ground used to place the browser's asset, which meant a click that missed the
rock you were aiming at left a second rock behind it. Placing is now something you do *from the
browser*, deliberately.

### Placing mode

`N` suspends rule 2. While it is on, **every left click in the world puts an object down** —
on top of whatever is already there — and nothing selects, drags or moves. Laying out a row of
pillars is a dozen clicks rather than a dozen drags out of the browser, and a click that lands
on the pillar you just placed does not turn into "you meant to pick that up".

The gizmo disappears while the mode is on, because a handle you cannot grab is a lie about
what a click will do, and the inspector's heading reads `INSPECTOR - PLACING (N)`. `N` again
or `ESC` gives the left button back to selection. It is mutually exclusive with the terrain
brush: `B` turns it off and it turns `B` off, since both want every press in the viewport.

Everything that is not a click still works: `TAB` picks the next asset, `X` deletes what was
just placed, `Z` takes it back, the arrows nudge it, `G` still decides the grid it lands on.

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

Rotation and scaling still live on `R` and `T`. The gizmo is built to take them as further
modes rather than as a second manipulator.

### A selection you have turned away from

If the camera moves so the selected object is off screen, the selection is not hidden: a
**striped amber silhouette** appears at the edge of the window in the direction the object
lies, labelled with what it is, sized as the object would be if you could see it. Turn back
and it walks around the edge to meet the object's own cyan outline as it comes into view.

### The selection holds one kind at a time

A level contains placements, bodies of water and spawn points, and the selection is tagged so a
verb reaches exactly one kind. Selecting a body of water clears a selection of objects and the
reverse; `CTRL+A` takes every object and no water. A rule an author can predict beats a general
one nobody can — `T` over a mixed selection would mean "scale these and resize those".

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
  "spawn_points": [ ... ]     // position, yaw
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

### A known gap

`EnvironmentScene::lights` is **not serialized**. A level authored in the editor loses every
brazier's local light when it is saved. Fixing it means deciding how a light is *authored*,
which is a phase of its own.

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
collapse, `F` filters, `F2` rescans. Arbitrary nesting works; the tree above is a convention,
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
