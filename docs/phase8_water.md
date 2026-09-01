# Phase 8 — Water

Water as a world element: something a level *has*, alongside its terrain and its lights, and
not something a level *places*.

This file is the brief the phase is executed from. The record of what was actually built goes
into `docs/phase_editor.md` when the work lands, as every phase from 2 onwards has.

---

## 1. What water is here, and what it is not

A **water body** is a rectangle with a yaw, a surface height and a depth.

```text
        yaw
         ↻                     ~~~~~~~~~~~~~~~~~~~~~  surface, at center.y
   ┌───────────┐               │                   │
   │           │  half_extents │      depth        │
   │     •     │  in X and Z   │                   │
   │           │               └───────────────────┘  the volume the query answers about
   └───────────┘
     seen in plan                    seen from the side
```

It is drawn as one transparent quad and it answers one question: *is this point under water?*

It is **not** a simulation. There are no waves, no flow, no buoyancy and no swimming. The
brief's own words are the specification: *"Initially a simple water plane/polygon is
sufficient. Do not attempt to build a fully realistic water simulation."*

### Why a world element rather than a placed object

A placement is an asset id and a transform; the engine resolves the id to a mesh and draws it.
Water has no asset — it has a size somebody dragged out, a height somebody chose, and a
question gameplay asks of it. Making it a placement would mean a level's water was a rock that
happened to be flat and blue, and "is the player in water" would have to be answered by
searching the placement list for objects whose material happens to be transparent.

`EnvironmentScene` already holds two things like this: `lights` and `terrain`. Water is the
third.

---

## 2. Scope: visual and a query, and nothing else

**In scope**

* Water renders, through the transparent pass that already exists.
* Water is created, moved, resized, rotated and deleted in the editor.
* Water is written to the scene file and read back.
* The engine can answer `is_submerged(point)` and `water_at(point)`.

**Out of scope, deliberately**

| Not doing | Why |
| --- | --- |
| Swimming, buoyancy, a movement change | Nothing asks for it. `MovementConfig` gains a field the day something does. |
| Drowning, damage, death on submerge | Phases 11–13 own the player lifecycle. They can call `is_submerged` when they need a second death trigger; inventing one here would build the lifecycle in the wrong phase. |
| A collision shape for water | Water does not block movement, and a volume nothing collides with is a volume the collision world should not carry. See §7 for the door left open. |
| Waves, ripples, per-frame animation | A scrolling normal map is an art pass, not a level system. The mesh is static, uploaded once. |
| Reflection, refraction, underwater fog | Every one of them is a renderer feature with its own phase's worth of cost and tuning. The transparent pass already draws a blue surface you can see through, which is what a blockout needs. |
| A water *material* per body | One surface for the whole level, named `"water"`, from the generated material library. When somebody needs two, it becomes a field on the body. |

---

## 3. The data

New module: `crates/engine-environment/src/water.rs`.

```rust
pub struct WaterBody {
    /// The middle of the surface. `y` is the height the water sits at.
    pub center: Vec3,
    /// Half-width and half-depth of the rectangle, in metres, before rotation.
    pub half_extents: Vec2,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// How far the volume reaches below the surface, in metres.
    pub depth: f32,
}
```

Methods:

* `contains(&self, point: Vec3) -> bool` — inside the rectangle *and* between the surface and
  `surface - depth`. Yaw-aware: bring the point into the body's own space by rotating it about
  the centre by `-yaw`, then compare against the half-extents. One sine and one cosine, the
  same trick the editor's local-space nudge uses.
* `surface_height(&self) -> f32`
* `bounds(&self) -> Aabb` — the box containing the rotated rectangle and its depth, for the
  editor's picking and for anything that wants a cheap reject.
* `is_valid(&self) -> bool` — finite numbers, both half-extents above zero, depth above zero.

On `EnvironmentScene`:

```rust
pub water: Vec<WaterBody>,

pub fn water_at(&self, point: Vec3) -> Option<usize>;
pub fn is_submerged(&self, point: Vec3) -> bool;
```

`is_submerged` is the one question with one answer, in the same spirit as
`EnvironmentScene::ground_at` from Phase 6: a caller must not have to know how many bodies a
level has or how they are shaped.

### Why a depth and not just a surface

Without a depth, "under water" means "below this height anywhere in the level", which is true
inside the hill next to the lake. A body is a volume, and the volume is what the query is
about.

---

## 4. The file

A `WaterRecord` in `crates/engine-environment/src/scene_file.rs`, following `TerrainRecord`
exactly:

```rust
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub water: Vec<WaterRecord>,
```

* Optional and skipped when empty, so every level written before this phase reads unchanged
  and a level with no water carries no water key.
* Validated on load — finite numbers, positive extents, positive depth — with the error naming
  `water[n]`, the way an object's error names `objects[n]`. A body with a zero extent is a
  surface nothing can be inside and a query that can never be true.
* Round trip asserted by a test: write a level with water, read it back, compare.

### A related gap this phase must not repeat, and must not silently fix

`EnvironmentScene::lights` is **not serialized**. There is no lights record in `scene_file.rs`,
so a level authored in the editor loses every brazier's local light when it is saved. That is a
real bug and it is *not* this phase's: fixing it means deciding how a light is authored in the
editor, which is a phase of its own. Water is serialized from the start so the same hole is not
dug twice, and the lights gap should be written down in `docs/phase_editor.md` as known.

---

## 5. The mesh

New module: `crates/engine-assets/src/procedural/water.rs`.

```rust
pub fn water_model(body: &WaterBody, material: &str) -> ModelAsset;
```

* One quad, at the surface height, sized to the body's half-extents, facing +Y.
* The yaw is **not** baked into the mesh — it goes on the render entity's transform, the way
  every placement's yaw does. A mesh that carried its own rotation would be a mesh that has to
  be rebuilt when the body is turned.
* UVs tiled by world metres, reusing the rule `procedural::terrain::TEXTURE_METRES` states, so
  the texel density of a pond and a lake are the same and neither is stretched.
* The material comes from `starter_material("water")`, which is already the only transparent
  entry in the starter library.

A test must assert **the material is transparent**. An opaque water surface is a silent
regression — the level still renders, the water is just a solid blue slab, and nothing about
the geometry says why.

Whether this reuses the existing `water_plane.glb` starter asset or generates a sized quad is
the implementer's call. Generating is the better default (a body is an arbitrary size, and
scaling a quad by a non-uniform factor makes the bounding sphere lie), but reusing is
acceptable if the sizing is done on the transform and the bounds are corrected.

---

## 6. Rendering

The transparent pass already exists and needs no work: see `docs/transparency.md`. A material
whose surface class is `Transparent` is moved into a second list, sorted back to front, drawn
after every opaque batch and before the particles, with depth test on and depth write off.

The application gains `App::rebuild_water`, mirroring `App::rebuild_terrain` from Phase 6:

* One render entity per body, despawned and respawned when the water changes.
* Called once at load, and again whenever the editor reports water changed.

`EditorOutcome` gains `WaterChanged`, for the reason `TerrainChanged` exists: `SceneChanged`
re-places every object in the level, and paying that on every frame of a drag that is resizing
one quad is exactly the cost the separate outcome avoids.

---

## 7. Collision: none, and the door left open

Water adds nothing to the collision world. A character walks through it as if it were not
there, because as far as movement is concerned it is not.

`engine-collision` has a layer bitmask (`CollisionLayer` / `CollisionMask`,
`crates/engine-collision/src/layer.rs`) with room for a `WATER` layer that a query could select
and the character's mask could exclude. That is the shape of the answer *if* water ever needs
to be a queryable volume in the physics world rather than a fact about the level. It is not
needed for `is_submerged`, which reads the scene directly, so this phase does not add it.

---

## 8. The editor

### The tool

`V` places a water body on the ground under the cursor, 8 m square, 2 m deep, at the height of
the ground it was placed on.

A selected water body responds to the verbs that already exist:

| Key | What it does to a water body |
| --- | --- |
| drag | moves it |
| arrows | nudge by the grid step |
| `T` / `SHIFT+T` | grow / shrink its half-extents |
| `R` / `SHIFT+R` | turn it, snapped to the rotation step |
| `X` | delete it |
| `Z` | undo, including the placement that created it |

Undo must carry water: `Snapshot` in `crates/engine-editor/src/lib.rs` currently holds the
objects and the terrain, and it grows a third field. Phase 6 learned this the hard way — the
undo stack held placements only, and a brush stroke could not be taken back.

### The one architectural change

**The editor's selection is `Vec<usize>` into `scene.objects` and has to become a tagged
selection.** Something like:

```rust
enum Selected {
    Object(usize),
    Water(usize),
}
```

This reaches into the hierarchy panel (which lists objects and must now list water bodies too),
the inspector (which describes the primary selection), `reveal_selection`, every verb in
`update_keys`, the drag, and `prune_selection`.

Requirements on that change:

* Every existing object verb keeps working exactly as it does now, asserted by the tests that
  already cover them — multi-select, group drag, duplicate, delete-highest-first, reset.
* An object verb must never reach a water body and a water verb must never reach an object.
  That is worth its own test: select one of each and assert that deleting removes the right one.
* Mixed multi-selection is *not* required. If selecting a water body clears an object selection
  and vice versa, say so in the docs and test it — a simple rule somebody can predict beats a
  general one nobody can.

### Making it visible

A water body is visible because it is drawn: a translucent blue rectangle is unmistakable from
the editor camera. The selected one should be marked the way a selected object is — an outline,
or the hierarchy row highlighted — so "which one am I about to resize" is answerable.

The inspector shows the surface height, the half-extents, the yaw and the depth. An author who
cannot see the depth cannot tell a puddle from a lake in plan view.

---

## 9. Acceptance criteria

```text
Launch the editor
      ↓
Press V, click the ground        → a water body appears where the cursor was
      ↓
Drag it, T to resize, R to turn  → it moves, grows and turns
      ↓
CTRL+S                           → the level is written, with a "water" key in it
      ↓
Launch the game with that level  → the water is there, in the same place, the same size
      ↓
Walk into it                     → nothing stops the character
      ↓
Ask is_submerged                 → true under the surface inside the body,
                                   false above the surface,
                                   false below the surface outside the body,
                                   false deeper than the depth
```

---

## 10. Tests the phase must produce

**`engine-environment`**

* `contains` with a yaw: a point that is inside the rotated rectangle and outside the
  unrotated one, and the reverse.
* Above the surface, below the bottom, and outside in X/Z — three separate `false`s.
* `is_submerged` over a level with two bodies at different heights.
* Scene-file round trip, and a malformed record refused with `water[n]` in the message.

**`engine-assets`**

* The quad's corners match the body's half-extents.
* The material is transparent.
* UV density does not change with the size of the body.

**`engine-editor`**

* `V` places a body; the scene gains one and the placement is undoable.
* Drag, resize, rotate, delete.
* An object and a water body can each be selected, and a verb reaches only the right one.
* The existing object tests still pass unchanged — that is the real assertion about the
  selection refactor.

**`engine-presentation`** (the integration crate that can see assets, environment and
collision — where `walk_under_and_through.rs` and `terrain_agreement.rs` already live)

* A character walks through a water body unimpeded: the assertion that water adds no
  collision, made by moving a character rather than by reading the collision world's length.
* `is_submerged` agrees with where that character is standing as it walks in and out.

---

## 11. Verification

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build -p engine-app --features editor
cargo build -p engine-app                 # the shipping build still has no editor in it

# A level with water, loaded headless
./target/debug/mygame.exe --scene-file assets/scenes/water_probe.json --frames 60 --hidden
```

The log should name the water the level brought, the way the terrain line does:

```text
environment 'water_probe': N objects, M collision shapes
environment 'water_probe': 2 water bodies
```

And the editor, opened on the same level:

```text
editor: editing 'water_probe' (N objects); F6 goes back to playing
```

---

## 12. Where this sits in the order

```text
Phase 6  terrain          the broad shape of the ground          done
Phase 7  cliffs, caves    what a heightmap cannot express        done
Phase 8  water            the third world element                this one
Phase 9  cave modules     placing the pieces into a cave         next
Phase 10 spawn points     where the player starts
```

Phase 9 will place the cave pieces Phase 3 generated and Phase 7 gave collision to. Nothing in
this phase should make that harder — in particular, the tagged selection introduced here is the
same mechanism a spawn point will need in Phase 10, so it is worth getting the shape of it
right rather than special-casing water.
