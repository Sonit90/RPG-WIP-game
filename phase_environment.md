# Phase 2 — Environment & Level Art Foundation

Living document. Every milestone appends to it as it completes: what was implemented, what
was tested, what was measured, what was decided and what was left undone.

Target: **one small, visually coherent, performant combat arena** that reads as a fantasy
combat arena rather than as a collection of test meshes, built on the existing renderer,
asset pipeline, collision world and culling — not beside them.

---

## Milestone status

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Audit current renderer and asset pipeline | **complete** |
| 2 | Define environment asset architecture | **complete** |
| 3 | Environment object representation | **complete** |
| 4 | Combat arena layout | **complete** |
| 5 | Ground and terrain | **complete** |
| 6 | Environment materials | **complete** |
| 7 | Modular environment props | **complete** |
| 8 | Instancing | **complete** |
| 9 | Environment collision | **complete** |
| 10 | Lighting | **complete** |
| 11 | Sky and background | **complete** |
| 12 | Camera composition | **complete** |
| 13 | Environment LOD | **complete** |
| 14 | Frustum culling / visibility | **complete** |
| 15 | Environment scene representation | **complete** |
| 16 | Separate environment demo scene | **complete** |
| 17 | Performance budget | **complete** |
| 18 | Environment stress test | **complete** |
| 19 | Automated tests | **complete** |
| 20 | Manual gameplay verification | **complete** |
| 21 | Visual quality pass | **complete** |
| 22 | Full regression | **complete** |
| 23 | Final performance audit | **complete** |

---

# Milestone 1 — Architecture audit

Read before a line of production code changed. Nothing below was rewritten; everything the
environment needed already existed in some form, and this milestone's output is the list of
which seam each new piece attaches to.

## The crate graph as found

```text
                        engine-core
                       /     |      \        \
                      /      |       \        \
        engine-platform      |   engine-render-data
                      \      |      /  ^        ^
                       \     |     /   |        |
                     engine-renderer   |    engine-game ── engine-collision
                             ^         |        ^
                             |    engine-assets |
                             |         |        |
                             |    engine-presentation
                             |              ^
                             \             /
                              engine-app
```

Hard rules, all of them checkable with `cargo tree`, all of them still true after this
phase:

* `engine-render-data` depends on `engine-core` **and nothing else**. No `windows`.
* `engine-game` depends on `engine-core` and `engine-collision` only. It cannot name a mesh.
* `engine-assets` has no device and no `windows`. A model is built and validated in a unit
  test with no adapter present.
* `engine-presentation` is the only crate that knows both worlds, and it still links no
  D3D12.

## What already exists and is reused unchanged

| System | Where | What the environment gets from it |
| --- | --- | --- |
| Runtime model | `engine_assets::ModelAsset` | LODs → submeshes → geometry + material index; materials; textures. Exactly the shape an environment prop needs, minus the skeleton. |
| Model LOD | `ModelAsset::lods`, `select_lod(distance, bias)` | Distance-threshold LOD selection, already sorted, already NaN-safe. |
| Procedural authoring | `engine_assets::procedural::Prism` | The one primitive every existing model is built from: a four-sided prism with independent cross-sections, correct winding and a UV span. Rocks, trunks, walls and crates are all prisms. |
| Procedural textures | `TextureAsset::generated(name, w, h, fmt, f)` | Deterministic RGBA generation from a UV closure, already used by the dummy's wood and straw. |
| Validation | `engine_assets::validate_model` | Every index inside a model checked once at load. `Renderer::create_model` refuses to upload a model that fails. |
| Model upload | `Renderer::create_model(&ModelAsset) -> ModelBinding` | Uploads every LOD's every submesh as a mesh + material, returns the `engine-render-data` binding presentation reads. One call per distinct asset. |
| Draw data | `engine_render_data::RenderWorld` | Structure-of-arrays: transform, mesh, material, style, bounds, visible. Generational handles. |
| Culling | `engine_renderer::Culler` | One pass over world-space bounding spheres against the camera frustum; serial and parallel paths produce identical lists. |
| Batching | `RenderList::group_by_material_and_mesh` | Sorts survivors so entries sharing a mesh **and** a material are contiguous, and emits `RenderBatch` runs. |
| Instancing | `Renderer::draw_batches_instanced` | One `DrawIndexedInstanced` per batch, instance transforms + styles composed straight into the upload ring. Already the production path. |
| Static collision | `engine_collision::CollisionWorld` | Plane / AABB / OBB shapes, layer+mask filters, lazy grid broad phase, generational handles. `add_box`, `add_block`, `add_ramp`, `add_ground`. |
| Lighting | `engine_render_data::Lighting` | One directional light + an ambient term, authored on the data side. |
| Camera | `engine_presentation::ThirdPersonCamera` → `engine_render_data::Camera` | Orbit/follow controller, perspective projection with near/far. |
| Scenes | `engine_app::args::Scene` | `--scene combat|collision|character`, each independently launchable with its own window title. |
| Benchmarks | `engine-bench` | Six binaries, each isolating one pass. `cull_bench` carries a counting global allocator. |

## Findings that shaped the design

1. **Environment objects are exactly rigid `ModelAsset`s.** A training dummy already *is* an
   environment prop in every respect that matters: rigid, multi-submesh, multi-material,
   three LODs, drawn through the instanced batch path. No new asset type is needed and none
   was added.

2. **The instancing win is free if and only if assets are shared.** `group_by_material_and_mesh`
   batches by `(material, mesh)` handle pairs. Two placements of `rock_a` that came from
   **one** `create_model` call share both handles and collapse into one draw; two placements
   that each called `create_model` do not. So the whole instancing requirement reduces to:
   upload each distinct asset exactly once, and let placements reference it by index.

3. **`RenderWorld` slots are the right home for static props.** They are spawned once, never
   despawned, never restyled — so they cost zero churn per frame and participate in culling
   and batching with no special case. The extractor's per-frame loop walks *gameplay* slots
   only, so environment entities are invisible to it.

4. **The visual floor and the collision floor are already separate**, and
   `RenderExtractor::set_ground_height` exists precisely because the collision demo brings
   its own floor. The arena ground follows the same pattern.

5. **There is no sky.** The back buffer is cleared to a constant `CLEAR_COLOR`. A skybox
   pass would need a new pipeline state, a depth-write-off state and a cube map. Deferred in
   favour of a sky *dome* — an inverted, gradient-textured shell drawn through the existing
   opaque mesh pipeline — plus distant ridge geometry. Costs two draws and no new pipeline.

6. **Nothing in the renderer knows what a scene is.** `App` hardcodes the arena's ground
   plane, marker and decals. That is where the "environment must be data-driven" requirement
   bites: the composition has to live somewhere that is neither `App` nor the renderer.

---

# Milestone 2 — Environment asset architecture

## The problem the crate split solves

An environment object has to be read by three crates that are deliberately forbidden from
knowing about each other:

```text
              EnvironmentScene  (placements: what, where, what collides)
                     │
        ┌────────────┼────────────────┐
        ▼            ▼                ▼
   engine-game   engine-assets   engine-presentation
   builds the    builds the      spawns render
   collision     ModelAssets     entities
   world
```

`engine-game` may not name a mesh. `engine-assets` may not name a collision world.
`engine-presentation` may name both but must not be where the layout lives, because the
layout is content. So the scene description goes in a crate below all three.

## The new crate

```toml
crates/engine-environment      # depends on engine-core and nothing else
```

The same rule `engine-render-data` lives under, for the same reason: it is checkable.

```
cargo tree -p engine-environment
engine-environment
└── engine-core
    └── log
```

Contents:

| Module | Type | Purpose |
| --- | --- | --- |
| `asset.rs` | `EnvironmentAsset` | A `&'static str`-backed id and the eleven constants naming the library. No allocation, `Copy`, comparable. |
| `object.rs` | `EnvironmentObject` | Transform + asset + collision + a shadow/decoration flag. |
| `object.rs` | `EnvironmentCollision` | `None` / `Box { half_extents, center }` / `Cylinder`-as-box. Plain math, no `engine-collision` type. |
| `scene.rs` | `EnvironmentScene` | The objects, the player spawn, the dummy spawns, the lighting preset, the arena bounds. |
| `arena.rs` | `combat_arena()` | The one shipping layout, deterministic. |
| `arena.rs` | `stress_arena(density)` | The scaling test layout. |
| `lighting.rs` | `EnvironmentLighting` | Sun direction, colour, intensity, ambient. Mirrors `Lighting` field-for-field without depending on `engine-render-data`. |

## Where each consumer attaches

| Crate | Gains | Why it is allowed |
| --- | --- | --- |
| `engine-assets` | `procedural::environment` — one `ModelAsset` per `EnvironmentAsset`, plus `library()` returning all of them. Dep on `engine-environment` for the ids. | An id is a name, not a device. |
| `engine-game` | `arena::build_environment_collision(&mut CollisionWorld, &EnvironmentScene)`. Dep on `engine-environment`. | Same dependency class as `engine-collision`: simulation-side geometry data. |
| `engine-presentation` | `environment::{EnvironmentVisuals, EnvironmentPlacer}` — id → `ModelBinding`, and the one-shot spawn into a `RenderWorld`. | Presentation is already the crate that knows both worlds. |
| `engine-app` | Uploads the library once, builds the scene, places it, converts the lighting. `--scene environment`. | Wiring only. |
| `engine-bench` | `env_bench` binary. | Measurement. |

## Directory layout

No `assets/` directory is created. Every environment asset is **procedural**, authored in
`engine-assets::procedural::environment`, for the same reasons the FireMage and the dummy
are: the project has an engine and no art pipeline, the result is original work with no
licence attached, and the glTF importer already proves that swapping a procedural model for
an artist's `.glb` is a change of which function fills a library slot. The proposed
`assets/environment/{terrain,architecture,rocks,vegetation,props}` tree is realised as
modules rather than directories, and the mapping is one-to-one.

## What the asset system supports, and where

| Requirement | Mechanism |
| --- | --- |
| Static meshes | `ModelAsset` with no skeleton. `SubMesh` per material. |
| Materials | `MaterialAsset` — base colour, roughness, metallic, emissive, three texture slots. |
| Textures | `TextureAsset::generated`, RGBA8 sRGB, shared between materials by index. |
| Transforms | `engine_core::transform::Transform` on each `EnvironmentObject`. |
| Collision representation | `EnvironmentCollision`, separate from the visual mesh and far coarser. |
| Optional LODs | `ModelAsset::with_lod(submeshes, min_distance)`. Assets carry one, two or three. |
| Instancing | Shared `ModelBinding` per asset ⇒ shared `(mesh, material)` handles ⇒ one batch. |

---

# Milestone 3 — Environment object representation

```text
EnvironmentObject                    44 bytes + a discriminant
├ asset       EnvironmentAsset       a &'static str id, Copy, comparable
├ transform   Transform              translation, yaw, uniform scale
└ collision   EnvironmentCollision   None | Box { center, half_extents }
```

Three decisions worth their reasons:

**The collision primitive is in the model's local space**, and
[`EnvironmentObject::world_collision`] carries it through the object's transform. That is
what lets one authored number serve every placement of an asset at every scale: a pillar is
`standing(0.62, 4.6, 0.62)` once, and a placement at scale 0.94 and one at 1.08 each get a
correctly-sized box without either number being written twice. The rotated case is handled by
rotating the box's *offset centre* about the origin, not just orienting it — a wall segment
turned to run along Z otherwise ends up beside the wall it is meant to be.

**The hand-off type is `WorldBox`, not `engine_collision::Shape`.** `engine-environment` does
not depend on `engine-collision`, so it cannot name a shape. It reports a centre, half-extents
and a quaternion, and the consumer that owns a collision world turns those into an AABB or an
OBB. That consumer is `engine_game::arena::add_environment_collision`, and the choice between
the two is made from the placement's rotation rather than authored — see Milestone 9.

**Environment objects never carry an `InstanceStyle`.** A style describes what is happening to
one entity *right now*: it is selected, it was hit, it is dying. Nothing happens to a rock.
Every environment render entity is spawned `NEUTRAL` and never written again.

## Tests

`engine-environment::object` — 5 tests: a decoration produces no world box; a standing box
sits on the model's origin; scaling a placement scales the box; a yawed placement reports a
rotation and an unyawed one does not; a yawed box carries its offset centre round with it.

---

# Milestone 4 — Combat arena layout

```text
                       distant ridges (6, at 235–300 m)
         ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
             treeline: pines, rocks, ferns, shrubs (r 43–118)

         P---W--W--W--P            [ARCH]            P--W--W--P     z = -38.6
         |                                                    |
         W                      D  (featured dummy)           W
         W                                b b                 W
         P                                b b  (group)        P     x = ±38.6
         W                                                    W
         W          ^  (player spawn)                         W
         P---W--W--W--W----------P----------W--W--W--W--W--W--P     z = +38.6

              b braziers at (±26, ±26)   crates against the kerb
```

| | |
| --- | --- |
| Objects | **196** in the shipping arena |
| Distinct assets | 16 |
| Blocking objects | 59 |
| Playable square | ±40 m (unchanged from Phase 3) |
| Combat space | 30 m radius, holding nothing but the floor |
| Boundary | 36 kerb segments, 9 pillars, 1 gate arch, 6 banners |
| Props | 4 braziers, 6 crates |
| Natural scatter | 37 rocks, 34 pines, 54 shrubs and ferns |
| Background | 6 ridges, 1 sky dome, 1 ground plain |

## The one composition rule

`CLEAR_RADIUS` = 30 m around the origin holds nothing but the floor, and it is **asserted**
rather than intended. Everything the fight touches lives inside it — the player spawn, the
featured dummy at 12 m, the dummy group whose furthest member is about 24 m out, and every
area ability placed among them. An ability indicator that clipped through a boulder would be
lying to the player about what it is going to hit.

The braziers at (±26, ±26) are 36.8 m from the middle, the crates are against the kerb, and
the inside scatter is placed on an annulus starting at 31.5 m. A test walks every object and
fails if any of them is closer than 30 m.

## The gate

The −Z side's middle four kerb segments are left out and the arch spans the gap, resting on
two pillars at x = ±13. This is the one asymmetry in the layout and it is deliberate: a
symmetric ring of identical wall has no direction and reads as a container. The gate gives
the arena a front, which is what a composition needs to be a composition rather than a
boundary.

The arch is **decoration**. Its span is five metres overhead where nothing can reach it, and
the two pillars it appears to stand on are separate placements that already block. Modelling
the legs into the arch would have meant either a collision primitive that cannot express two
boxes or geometry drawn twice.

## Determinism

The scatter is placed with a hash of the object's index, not a random number generator, so
`combat_arena()` produces a byte-identical scene on every call, in every process, on every
machine. `EnvironmentScene::signature` makes that a one-line assertion, and the benchmark
records it so two runs are known to have measured the same arena.

Angles are jittered around an **even spread** rather than being wholly random: a wholly random
angle clumps, and a clump of pines against an empty arc reads as a mistake rather than as
nature. Radii are square-rooted so an annulus fills evenly by area instead of crowding its
inner edge.

## Tests

`engine-environment::arena` — 13 tests, including: the arena is identical every time; every
category the brief names is present in the counts stated above; the combat space holds nothing
but the floor; every spawn point is well inside it; nothing straddles the arena wall; the gate
is a gap with an arch over it; ground cover never blocks and large obstacles always do; nothing
outside the wall carries a collision shape.

---

# Milestone 5 — Ground and terrain

Two objects and one plane, and the three are deliberately separate things:

| | What | Where |
| --- | --- | --- |
| Visual plain | A unit quad scaled to 640 m, tiling its texture 64 times | `ground_plain`, y = 0 |
| Visual paving | A 32-segment disc, radius 41 m | `arena_floor`, y = **0.02** |
| Collision floor | An infinite `Shape::Plane` | `CollisionWorld`, y = 0 |

**The paving is two centimetres above the plain** because two coplanar surfaces are a
depth-buffer tie, and a tie flickers across the whole floor. Two centimetres is far below what
the camera resolves at gameplay distance and far above the depth buffer's precision at these
ranges.

**The floor is a disc inside a square boundary.** Along each axis the paving runs a metre past
the wall, so there is no seam where a player can stand; on the diagonals it stops well short
and the corners are ground. That is the composition — a paved arena worn into a plain, rather
than a paved square whose corners are dead space nobody fights in.

**The collision floor is the plane it always was.** The visual ground carries no collision at
all: `EnvironmentCollision::None` on both objects. Movement, jumping and landing are unchanged
from Phase 5, because nothing about them changed — the character still stands on `y = 0`, and
`crates/engine-collision/tests/movement.rs` and `crates/engine-game/tests/collision_gameplay.rs`
both still pass untouched.

The extractor's own placeholder ground plane is **hidden** in any scene that brings an
environment (`world.set_visible(extractor.ground(), false)`), for the same depth-tie reason.
Hidden rather than despawned, because the extractor owns that entity and would be left holding
a stale handle.

No height-mapped terrain, no chunking, no streaming. The brief asks for the smallest terrain
representation that supports the arena, and a flat arena is what the combat is designed
around: an ability placed on a slope would need its ground indicator projected onto the slope,
which is a different feature in a later phase.

---

# Milestone 6 — Environment materials

Nine textures and eleven materials for the whole library.

| Texture | Size | Used by |
| --- | --- | --- |
| `env_stone` | 128² | wall segment, pillar, arch |
| `env_flagstone` | 128² | arena floor |
| `env_ground` | 128² | ground plain |
| `env_rock` | 128² | rock small, medium, spire |
| `env_bark` | 128² | pine trunk |
| `env_foliage` | 128² | pine canopy, shrub, fern |
| `env_wood` | 128² | crate, banner pole |
| `env_metal` | 64² | brazier |
| `env_cloth` | 64² | banner |
| `env_sky` | 8×128 | sky dome |

Two materials carry no texture at all: `env_coals` (an emissive constant — a glow is added
after lighting and needs no map) and `env_ridge` (a mountain at 250 m is a shape and a
colour, and a rock texture on it would alias).

All of it goes through the existing `Albedo` pipeline: `MaterialAsset` with a base colour, a
roughness, a metalness, an emissive and a base-colour texture index. **No new shader variant
and no new pipeline state** were introduced by this phase.

The normal and ORM slots exist on `MaterialAsset` and are left empty, as they are for the
character: a normal map without vertex tangents is a wrong answer rather than a cheap one.

## Measured

Whole library: **932 KiB** of texture memory, 16 models, **922 triangles** at LOD0 across all
of them. A test asserts the library stays under a megabyte and that no more than twelve
distinct material names exist across it.

## The duplication that is not fixed

`ModelAsset` owns its textures and `Renderer::create_model` uploads them, so `rock_small`,
`rock_medium` and `rock_spire` each carry and upload their own copy of `env_rock`. Eighteen
texture uploads for nine distinct images. At 932 KiB total this is under half a megabyte of
waste on a 12 GiB card, and the fix — a renderer-side texture cache keyed by name — is a
change inside the uploader that no caller would notice. It is listed under deferred work
rather than pretended away.

---

# Milestone 7 — Modular environment props

Sixteen assets, all procedural, all original, all built from the `Prism` primitive the
FireMage and the training dummy already use, plus two new flat primitives (`quad` and `disc`)
promoted into a shared `procedural::MeshBuilder`.

| Category | Assets | Triangles at LOD0 |
| --- | --- | --- |
| Terrain | `ground_plain`, `arena_floor` | 2, 32 |
| Architecture | `wall_segment`, `pillar`, `arch` | 36, 48, 60 |
| Rocks | `rock_small`, `rock_medium`, `rock_spire` | 24, 36, 48 |
| Vegetation | `shrub`, `fern`, `pine` | 36, 60, 48 |
| Props | `brazier`, `supply_crate`, `banner` | 60, 36, 36 |
| Background | `ridge`, `sky_dome` | 60, 288 |

The brief's minimum was 2–3 of each of four categories; this is 3 of each plus terrain and
background.

**A boulder is built from prisms**, which is the one shape a prism is not obviously the
primitive for. Two or three offset, unevenly tapered prisms give a faceted mass with a
readable silhouette at twenty-four triangles a lobe, and the stylized target wants facets
rather than a smooth ellipsoid. A UV sphere at the same triangle count is a lumpy ball; at a
silhouette good enough to compete it is four times the cost, for geometry there are a hundred
of.

**Nothing is hardcoded into the renderer.** The renderer never learns that `pillar` exists.
The composition is a `Vec<EnvironmentObject>`, the resolution from an id to a model is one
`match` in `engine-assets`, and the resolution from a model to handles is one loop in the
application's `upload_environment`.

**An unknown asset id draws a magenta marker**, loudly, and logs an error. A scene naming an
asset the library does not have is a content bug, and an empty space is indistinguishable
from an object that was never meant to be there.

## Tests

`engine-assets::procedural::environment` — 16 tests, including: every asset validates and is
rigid; the library stays inside its triangle budget; every model stands on its own origin
(with three documented exceptions); every level is cheaper than the one before it and draws
the same materials; the material set is small and shared; every texture reference is in
range; the floor and wall constants match the layout's; the sky dome is wound inwards; every
asset is byte-identical on every build.

---

# Milestone 8 — Instancing

**No instancing system was added, because the renderer already had one and the requirement
reduces to sharing.**

```text
upload_environment()            one create_model per ASSET  (16 calls)
        │
        ▼
EnvironmentVisuals              asset id -> ModelBinding
        │
        ▼
EnvironmentPlacer::place()      196 placements, each referring to the binding's
        │                       MeshHandle and MaterialHandle
        ▼
RenderWorld                     196–570 entities, ~20 distinct (mesh, material) pairs
        │
        ▼
Culler::cull                    survivors gathered into a RenderList
        │
        ▼
group_by_material_and_mesh      a counting sort: survivors sharing a pair become contiguous
        │
        ▼
draw_batches_instanced          ONE DrawIndexedInstanced per batch
```

Every placement of `rock_small` refers to the handles one `create_model` returned, so all of
them land in one bucket of the counting sort, become one `RenderBatch`, and are issued as one
draw with an instance count. Adding a thousand more rocks adds a thousand 44-byte records and
**no GPU resources at all**.

## Measured, on the real GPU

| Scene | Objects | Render entities | Visible | Draw calls | Triangles |
| --- | ---: | ---: | ---: | ---: | ---: |
| Combat arena, 100 dummies | 196 | 240 | 272 | **33** | 7,948 |
| Stress ×4 | 571 | 717 | 400 | **34** | 10,084 |
| Stress ×8 | 1,071 | 1,353 | 582 | **34** | 13,240 |

The object count goes up 5.5× and the draw-call count goes up by **one**. That is the whole
argument.

## A per-frame allocation this exposed and removed

`RenderList::group_by_material_and_mesh` copied its bucket cursors with
`self.counts[..buckets].to_vec()` — one heap allocation of `material_count × mesh_count`
words **every frame**. It went unnoticed while the scene had nine materials and four meshes,
and `env_bench` counted it the moment the environment brought forty of each: **1.18
allocations per frame**. The cursors now live in a reused `starts` field. Measured after:
**0.0003 allocations per frame** in the empty-arena scenario, and zero in the three that hold
the camera still.

The same pass in `EnvironmentPlacer::update_lod` took a `Vec` and replaced it, discarding its
capacity on every level change; it now drains and refills in place.

## Tests

`engine-presentation::environment` — a thousand placements of one rock produce one mesh
handle, one material handle and a thousand distinct transforms; the arena collapses into at
most thirty `(mesh, material)` pairs; the upload count is the asset count and not the object
count; a level change keeps an object inside the same material batch.

---

# Milestone 9 — Environment collision

The brief's own table, implemented as `arena::natural_collision` and the layout's explicit
primitives:

| Object | Collision | Count |
| --- | --- | ---: |
| Ground | `Shape::Plane` at y = 0, from `arena::build_collision` | 1 |
| Arena boundary | Four `Shape::Aabb` walls at ±40, from `arena::build_collision` | 4 |
| Kerb segment | Box, 8 × 1.15 × 0.9 m — `Aabb` on two sides, `Obb` on the other two | 36 |
| Pillar | Box, 1.24 × 4.6 × 1.24 m | 9 |
| Brazier | Box, 0.9 × 1.35 × 0.9 m | 4 |
| Crate | Box, 0.92 × 0.92 × 0.92 m | 6 |
| Reachable boulder | Box | 3 |
| Everything else | **nothing** | 137 |

**59 of 196 objects block.** The other 137 — the treeline, the ridges, every fern and shrub,
every small rock, the banners, the arch's span, the sky, the ground and the paving — carry no
collision at all.

Two rules produce that split, and both are asserted:

1. **Ground cover never blocks.** Knee-high geometry that stops a character dead is worse
   than no geometry: the player cannot see why they stopped.
2. **Nothing outside the arena wall blocks.** The wall at ±40 confines the character, so a
   shape beyond it is a broad-phase proxy, an AABB and a narrow-phase candidate for geometry
   no capsule can ever touch. Thirty-four pine trunks nobody can reach is thirty-four objects
   the grid has to hold and every query has to skip.

**Collision complexity is chosen, not derived.** A pillar is 48 triangles of tapered prisms
and one box. A pine is 48 triangles and a 0.56 m trunk box — the canopy is not blocked,
because a character brushing through the lower branches of a conifer is correct and being
stopped four metres from the trunk is not. There is no code path anywhere in this phase by
which a visual mesh becomes a collision mesh; it would have to be written.

**AABB or OBB is decided at build time** by comparing the placement's rotation magnitude
against 2×10⁻³ — about a quarter of a degree of yaw. An axis-aligned box is the cheaper
narrow-phase test and most of the arena is unrotated; which one a placement needs is a
consequence of its transform, not a decision an author should have to make.

A collision box with a zero extent is **refused and logged**, not added: it would be a shape
every query tests and no query can touch.

## Tests

`engine-game::arena` — 7 new tests: an environment scene becomes a playable arena with a
non-dirty broad phase; only the objects that block become shapes, and each is named after its
asset; a yawed placement becomes an `Obb` and a straight one an `Aabb`; a zero-volume box is
refused; every spawn point and every entity position is free of geometry; the kerb stands
inside the wall that confines the player; the whole thing is built identically every time.

---

# Milestone 10 — Lighting

```rust
EnvironmentLighting::arena_afternoon()
    sun_direction     (-0.62, -0.55, -0.56)   low, travelling down and across
    sun_color         (1.00, 0.91, 0.75)      warm
    sun_intensity      1.25
    ambient_color     (0.46, 0.58, 0.80)      cool
    ambient_intensity  0.34
    sky_zenith        (0.16, 0.31, 0.60)
    sky_horizon       (0.74, 0.66, 0.56)
```

Every number has a reason, and each of the first four is asserted in a test:

* **The sun is low** — its horizontal travel exceeds its vertical — so a vertical face and a
  horizontal one receive clearly different amounts of light. A sun overhead flattens every
  silhouette in the scene, which is the opposite of the readability the phase asks for.
* **The sun is warm and the ambient is cool.** Two-tone lighting is what separates a lit face
  from a shadowed one when there are no cast shadows to do it: the lit side goes gold, the
  unlit side goes blue, and the eye reads form from the hue difference rather than from
  brightness alone.
* **The ambient is 0.34, which is high.** There are no cast shadows and no bounce, so an unlit
  face receives exactly this and nothing else. Below about 0.25 the far side of every rock
  reads as a hole.
* **The sun's intensity is 1.25 rather than 1.0** because the ambient is added on top of it,
  and a fully lit surface should still reach its base colour.

**Lighting is content and lives with the scene.** `EnvironmentLighting` is a plain struct in
`engine-environment`; `engine_presentation::scene_lighting` is a five-field copy into
`engine_render_data::Lighting`, and it is the only place in the project that performs it —
neither neighbour can name the other's type. A boss arena wanting a different sun is a
different preset, not a change to the renderer.

**No shadows were added and no new lighting feature was measured in.** The existing
directional-plus-ambient model is what the arena is composed for. Shadow mapping is a depth
pass, a second matrix, a comparison sampler and a filter kernel — real GPU cost — and the
brief says not to introduce expensive lighting features without measuring them. Deferred, and
the ambient is set high specifically because there are none.

The **clear colour** now comes from the scene too: the horizon colour at 45%. Deliberately
darker than the sky rather than matched, so a gap in the dome reads as a gap instead of
hiding.

---

# Milestone 11 — Sky and background

Three layers, none of which needed a new pipeline state:

**The sky dome.** An inverted 24 × 6 shell at 420 m radius, 288 triangles, one draw, drawn
through the existing opaque mesh pipeline. A skybox would have been a second pipeline state, a
depth state that writes nothing, a cube texture and a shader that reconstructs a view ray; a
dome is geometry, and geometry is what this renderer already draws well. It is cheaper than a
single pine tree.

Two details that matter:

* **Every vertex's normal is `+Y`.** The dome's colour has to be the ramp, not the ramp times
  a lighting term that varies over the shell — a sky lit like a solid object has a bright
  side, which is exactly what a sky does not have. A constant normal makes `dot(N, L)` one
  number for the whole dome, and the gradient the eye sees is the texture's alone. The
  texture is brightened by 1.18 to cancel that constant, and a test computes the shader's own
  arithmetic and asserts the sky comes out within 12% of its authored colour.
* **The shell continues 18% of its radius below the horizon.** The plain is a finite quad; from
  the middle of the arena the ground ends at 320 m, and without a skirt the horizon would be a
  band of clear colour between the two.

**Six distant ridges** at 235–300 m, each 200 m of mountain silhouette, turned to face the
origin so they are seen broadside. Untextured, with a small emissive term standing in for
atmospheric haze — which is what stops them reading as enormous nearby rocks. Their feet are
buried, because a mountain meeting the ground at a hard edge reads as a cardboard cut-out.

**A treeline** of 34 pines, 37 rocks and 54 pieces of ground cover on annuli from 43 to 118 m,
outside the wall. This is what gives the horizon a silhouette between the arena and the ridges.

A GPU test asserts the background is not decoration-in-name-only: removing everything past the
arena wall changes more than 400 pixels of the gameplay view.

---

# Milestone 12 — Camera composition

**The camera system was not touched.** `ThirdPersonCamera` remains authoritative; the
environment was composed around it.

What the composition is:

| | |
| --- | --- |
| Default eye height / distance | The Phase 3 controller's, unchanged |
| Far plane | 1,000 m — the sky dome at 420 m and the furthest ridge at ~430 m both fit |
| Near clipping into geometry | The kerb stands 1.4 m **inside** the collision wall, so a character pressed against the wall does not have the camera inside a pillar |
| Empty space | The combat space is 30 m of clear floor; the boundary, the braziers and the crates give the eye something at the edge of it |
| Obstruction | Nothing over 1.35 m stands inside 30 m. Nothing the player fights around can hide a dummy |
| Depth | Kerb at 39 m, treeline at 43–118 m, ridges at 235–300 m, sky at 420 m — four distance bands in a 40 m arena |

The one camera-adjacent constant that did change is the **level-of-detail centre**: a level is
selected from the distance to the middle of the model's LOD0 bounds, not to its origin. A pine
is authored standing on its origin and is six metres tall, so measuring to its feet swaps it
to its coarsest form while its canopy fills the screen.

---

# Milestone 13 — Environment LOD

Levels are chosen per asset from size, screen-space importance and count — not three each
because three is the number.

| Levels | Assets | Thresholds |
| ---: | --- | --- |
| 3 | `pine` | 55 m, 110 m |
| 3 | `pillar` | 40 m, 95 m |
| 3 | `rock_medium`, `rock_spire` | 34/45 m, 80/105 m |
| 2 | `wall_segment`, `arch`, `brazier`, `banner` | 45, 70, 42, 60 m |
| 2 | `supply_crate`, `shrub`, `fern`, `rock_small`, `arena_floor` | 32, 30, 25, 38, 150 m |
| 1 | `ground_plain`, `sky_dome`, `ridge` | — |

Three levels go to what is both numerous *and* tall enough to stay on screen from across the
arena. One level goes to what has nothing to remove: two triangles, a shell, and a silhouette
only ever seen from 250 m.

**Ground cover is the first thing that stops costing anything**: a fern drops from five blades
to two at 25 m, which is still a green smudge in the right place — all it was contributing.
Nothing disappears entirely; a popping object at a fixed distance is more distracting than a
cheap one.

Two invariants, both asserted for every asset:

* **Every level is strictly cheaper than the one before it.** A coarser level that costs more
  is a level that is never worth selecting.
* **Every level draws the same materials.** A level change is different geometry and the same
  surface — if it also changed the material, crossing a threshold would move an object between
  batches and the draw-call count would jump for a reason nobody could see.

## The per-frame cost

`EnvironmentPlacer::update_lod` runs over every placement every frame: a cached centre, a
subtract, a length and a walk of a sorted three-element threshold list. A placement that
crossed a threshold has its one or two render entities despawned and respawned; one that did
not costs the comparison and nothing else.

Measured: **2.4 µs per frame for 196 objects**, 7.4 µs for 571. Level changes over 3,000
frames of the benchmark's movement rotation: **0.19 per frame**. In the running game, with the
camera following a player inside a 40 m arena, the periodic report shows **0 level changes per
second** — nothing in the arena is near enough to a threshold for ordinary movement to cross
it, and the levels earn their keep on the treeline as the camera turns.

---

# Milestone 14 — Frustum culling / visibility

Environment entities are ordinary `RenderWorld` slots, so they go through
`Culler::cull` with everything else. No environment-specific visibility code exists, and that
is the point: one cull, one frustum, one render list. A character and the ground it stands on
cannot disagree about whether they are on screen.

## Measured

| Camera | Visible | Culled | Rejected |
| --- | ---: | ---: | ---: |
| Gameplay, looking down the arena | 63 | 177 | 74% |
| Gameplay, 100 dummies | 220 | 290 | 57% |
| Corner, looking across the whole arena | 343 | 100 | 23% |
| Middle, facing the gate and the treeline | 272 | 189 | 41% |
| Stress ×4, looking across | 398 | 342 | 46% |

Culling cost: **1.2 µs** for the empty arena, 3.9 µs at the worst view, 5.2 µs on the stress
scene — over a world of 240 to 741 entities. `cull_bench` independently reports 6.0 ns/object
at 50% visibility with zero allocations, unchanged from Phase 3's 5.5 ns/object within
run-to-run variance.

The brief's five cases:

* **Camera looking toward the arena** — 57–74% rejected, above.
* **Camera looking away** — the corner and gate views bracket it; with the camera pointed out
  of the arena the treeline behind it is rejected and the visible count falls below the
  gameplay figure.
* **Camera close to an object** — the level pass selects LOD0 and the object survives; nothing
  in the path special-cases proximity.
* **Camera far from an object** — the treeline at 118 m is at its coarsest level and is culled
  the moment it leaves the frustum.
* **Objects outside the frustum** — 100 to 290 of them, every frame, rejected by a four-plane
  sphere test that reads only `bounds` and `visible`.

**The environment introduces no unnecessary CPU or GPU work**: it is spawned once, never
extracted, never restyled, never transform-written. Its whole per-frame cost is the level pass
(2.4 µs) plus its share of a cull that was happening anyway.

---

# Milestone 15 — Environment scene representation

```text
EnvironmentScene
├ name            "combat_arena" | "environment_demo" | "stress"
├ objects[]       every placement, in a deterministic order
├ spawns          player, featured dummy, group centre, group spacing
├ lighting        sun, ambient, sky colours
├ half_extent     40 m
└ ground_height   0
```

Nothing is hardcoded into Rust source *in the renderer or in gameplay*. The layout is a
function in `engine-environment` producing a `Vec<EnvironmentObject>`, and three crates
consume it as data:

```text
                        EnvironmentScene
                               │
        ┌──────────────────────┼──────────────────────┐
        ▼                      ▼                      ▼
   engine-game           engine-assets        engine-presentation
   add_environment_      environment_model    EnvironmentPlacer
   collision             (id -> ModelAsset)   (id -> render entities)
```

## Why a function and not a file

A scene file needs a parser, a schema, an error path for a malformed one and a decision about
what happens when it names an asset that does not exist. All four are real work that buys
nothing until scenes are authored by someone who is not editing the source anyway. What the
brief asks for is that the composition is *data* rather than being hardcoded into the
renderer, and that is what this is. Replacing the function with a loader is a change to one
module — everything downstream already treats the result as data it did not write.

## Reproducible and deterministic

`EnvironmentScene::signature` is an order-sensitive FNV hash over every asset name, every
transform component and every collision flag. It exists so "the scene is reproducible" is a
one-line assertion rather than a comparison of 196 transforms, and so a benchmark can record
*which* scene it measured. Tests assert it is stable across calls and changes when a placement
moves by a millimetre, when a collision primitive changes, and when two objects swap order.

`ArenaConfig::from_environment` is the seam to gameplay: the scene owns the spawn points,
because the scene is what put a boulder somewhere and therefore what knows where there is
room; gameplay owns what stands on them. The arena half-extent comes from the scene too, so
the collision wall and the drawn kerb cannot disagree about where the arena ends.

---

# Milestone 16 — Separate environment demo scene

```
mygame --scene environment
```

The combat arena, plus one of every exhibitable asset in two rows of eight standing at
z = 27 and z = 34.5 — past the combat space, inside the wall, where the camera can be walked
down them. 209 objects against the combat arena's 196.

**It is the same arena rather than a bespoke one**, for the same reason the character test bed
is the arena with the fight taken out: the thing being inspected has to be the thing the game
ships, or inspecting it proves nothing. The three assets that *are* the scene — the sky dome,
the ground plain and the arena floor — are not duplicated onto the row; a second dome would be
a shell inside a shell and a second floor a depth tie.

What it makes inspectable, and how:

| | |
| --- | --- |
| Terrain | Stand on it; the paving/plain boundary is visible at the kerb |
| Props | One of each on the row, at three-quarter view |
| Materials | Every material in the library is on screen at once |
| Lighting | The row runs east-west across the sun, so each asset is seen lit and unlit |
| LOD | Walk away from the row; `--env-density` and the report line show level changes |
| Culling | The periodic log reports visible and culled counts every second |
| Collision | Walk into the row: the wall, pillar, brazier and crate exhibits block, the plants do not |

Scene separation is unchanged and asserted in `args.rs`: `combat`, `collision`, `character`
and `environment` are four independently launchable scenes with four window titles. The
collision course and the character test bed bring **no** environment at all
(`Scene::has_environment()` is false for both) — a collision course with the arena's kerb in
it would be a course with scenery in the way of the tests.

The combat scene uses the final combat arena.

---

# Milestone 17 — Performance budget

## The machine

```text
Ryzen 5800X / RX 6700 XT / 1920×1080 / windowed, no vsync
Target: 240 FPS minimum (4.167 ms), 480 FPS aspirational (2.083 ms)
```

## In the running game

Release build, 12,000–40,000 frames per run, figures from the periodic report and the final
window. The report's CPU figure is the whole frame; GPU is the timestamp query.

| Scenario | CPU mean | CPU p99 | GPU mean | FPS | Draws | Triangles | Visible |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A — arena, no dummies | 0.172 ms | 0.31 ms | 0.076 ms | 5,808 | 31 | 2,596 | 85 |
| B — arena + player | 0.172 ms | 0.31 ms | 0.076 ms | 5,808 | 31 | 2,596 | 85 |
| C — + 10 dummies | 0.203 ms | 0.42 ms | 0.092 ms | 4,936 | 33 | 3,316 | 105 |
| D — + 50 dummies | 0.299 ms | 0.46 ms | 0.149 ms | 3,341 | 33 | 5,548 | 185 |
| E — + 100 dummies | 0.318 ms | 0.71 ms | 0.156 ms | 3,147 | 33 | 7,948 | 272 |
| Environment demo | 0.220 ms | 0.68 ms | 0.099 ms | 4,556 | 31 | 2,596 | 85 |
| Character bed (no environment) | 0.249 ms | 0.90 ms | 0.097 ms | 4,012 | 10 | 728 | 5 |
| Collision course (no environment) | 0.238 ms | 0.85 ms | 0.096 ms | 4,206 | 10 | 940 | 10 |

A and B are the same line because the player is one skinned character in a 240-entity world
and does not move the frame. The environment demo reports the same visible count as the
combat arena because its extra row stands **behind** the default camera — which is itself a
small piece of evidence that culling is working.

## Headless, by pass

`env_bench`, 3,000 measured frames per scenario plus 300 warm-up, microseconds per frame:

| Scenario | LOD | Extract | Cull | Group | **Total** | p99 | Allocs/frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A empty arena | 2.39 | 0.03 | 1.17 | 1.56 | **5.15** | 7.00 | 0.0003 |
| B + player | 2.36 | 0.13 | 1.21 | 1.59 | **5.28** | 5.80 | 0.0023 |
| C + 10 dummies | 2.34 | 0.77 | 1.43 | 1.64 | **6.18** | 6.70 | 0.023 |
| D + 50 dummies | 2.38 | 3.32 | 2.24 | 1.90 | **9.84** | 10.90 | 0.106 |
| E + 100 dummies | 2.43 | 7.09 | 3.19 | 2.07 | **14.77** | 16.40 | 0.229 |
| F across arena | 2.45 | 5.33 | 3.90 | 2.40 | **14.07** | 14.70 | 0 |
| G worst view | 2.29 | 5.68 | 3.58 | 2.12 | **13.67** | 14.40 | 0 |
| H stress ×4 | 7.41 | 0.67 | 5.23 | 2.51 | **15.82** | 16.60 | 0 |

The residual allocations in C, D and E are the damage meter appending a row the first time
each distinct target is hit — the same Phase 3 residue `game_bench` documents, and it is zero
in the scenarios where the target set has been seen. Nothing in the environment path allocates
per frame.

## Memory

| | |
| --- | --- |
| Environment texture memory | **932 KiB** (18 uploads of 9 distinct images) |
| Environment geometry | 922 triangles at LOD0 across 16 models; ~46 KiB of vertex and index data |
| Render entities added | 240 for the combat arena, 256 for the demo, 1,353 at stress ×8 |
| GPU memory reported by the adapter | 12,242 MiB dedicated; the environment is not measurable against it |
| Per-frame heap allocations | 0 in the environment path (was 1.18 before the fix in Milestone 8) |

## Verdict against the target

The worst measured case in the shipping game — the arena, a hundred dummies, a full UI — is
**0.318 ms of CPU and 0.156 ms of GPU**, a p99 of 0.71 ms, and roughly 3,100 FPS. The 240 FPS
floor needs 4.167 ms; the 480 FPS aspiration needs 2.083 ms. Both are met with more than a
factor of six of headroom on the p99, on the stated development machine, at 1920×1080.

That is an honest statement about *this* content on *this* machine and nothing more. The
frame is not pixel-bound (Phase 2 measured that dropping to 480×270 changed GPU time by
nothing measurable) and it is not draw-call bound (33 draws). What will move these numbers is
a bigger world, more characters, shadows, or transparency — none of which this phase added.

---

# Milestone 18 — Environment stress test

`stress_arena(density)` multiplies the scatter while keeping the boundary, the spawn points
and the lighting identical. A stress configuration that went down a different code path would
measure the other code path, so it loads through the ordinary one:

```
mygame --dummies 100 --env-density 4
mygame --dummies 100 --env-density 8
```

## Measured in the running game

| Density | Objects | Entities | Visible | CPU mean | GPU mean | Draws | Triangles |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 (shipping) | 196 | 240 | 272 | 0.254 ms | 0.120 ms | 33 | 7,948 |
| 4 | 571 | 717 | 400 | 0.385 ms | 0.187 ms | **34** | 10,084 |
| 8 | 1,071 | 1,353 | 582 | 0.416 ms | 0.203 ms | **34** | 13,240 |

At density 4 the scene holds 120 rocks, 232 plants and 46 architectural pieces, which meets
the brief's stress targets for three of its four categories; the fourth (100 miscellaneous
props) is not reached because the props are placed by the layout rather than by the scatter,
and multiplying the boundary would have measured a different level rather than a denser one.

## What it says

**The environment scales sub-linearly and the draw-call count does not scale at all.** 5.5×
the objects costs 1.6× the CPU and 1.7× the GPU, and one extra draw call. The three reasons
are the three mechanisms this phase built: assets are shared so batching collapses them,
culling rejects roughly half of what is added, and levels of detail make the additions cheap
before they are drawn.

The costs that *do* grow are the two that are linear in object count by construction: the
level pass (2.4 → 7.4 µs) and the cull (1.2 → 5.2 µs). Both are flat arrays walked once.

## Where it would stop

Extrapolating the level pass and the cull, the CPU side reaches the 240 FPS budget somewhere
around 100,000 objects — long after other things break first. The honest limit is not measured
here and is not claimed.

---

# Milestone 19 — Automated tests

**86 new tests**, and the workspace total is **1,572 passing, 0 failing**.

| Area | Where | Count | What is covered |
| --- | --- | ---: | --- |
| Environment data | `engine-environment` | 31 | Object creation, transforms, scaled and rotated collision boxes, asset references, the layouts, spawn points, determinism, the composition rule, the demo and stress scenes |
| Asset library | `engine-assets::procedural::environment` | 16 | Every asset builds, validates, is rigid, stands on its origin, stays inside budget; levels are ordered and material-stable; texture references are in range; constants match the layout; the sky is wound inwards; unknown ids draw a marker |
| Mesh building | `engine-assets::procedural` | 3 | The shared builder, disc winding, quad normals |
| Collision | `engine-game::arena` | 7 | Scene to collision world, AABB/OBB selection, zero-volume refusal, spawn-point clearance, boundary agreement, determinism |
| Placement, LOD, instancing | `engine-presentation::environment` | 13 | Every object becomes drawable; upload count is asset count; a thousand rocks share one mesh and material; the arena collapses to ≤30 pairs; distance selects levels; a still camera changes nothing; a level change stays in its material batch; a second scene replaces the first; an unresolved asset is counted not substituted; the light conversion |
| Rendering, on a real GPU | `engine-renderer/tests/environment_render.rs` | 7 | The arena reaches the screen; sky and ground exist and differ; the floor is under the player; the boundary is visible from the fight; the background gives depth; every asset draws something; the demo row is where it claims |
| Scene selection | `engine-app::args` | 2 | `--scene environment`, `--env-density`, which scenes bring an environment |

### Missing-asset and invalid-data handling

* A scene naming an unknown asset: `EnvironmentPlacer` counts it in
  `EnvironmentStats::unresolved`, skips it, and the application logs an error naming the
  scene and the count. Tested.
* `environment_model` on an unknown id: logs an error and returns a magenta emissive marker.
  Tested.
* A zero-volume collision box: logged and refused. Tested.
* A malformed model: `create_model` validates before uploading and refuses, naming the model
  and the place inside it. Existing behaviour, unchanged.

### What was deliberately not added

No production overhead exists solely to make testing possible. Every seam the tests use —
`EnvironmentStats`, `lod_of`, `parts_of`, `signature` — is either something the developer
overlay and the log line already read, or a `#[must_use]` accessor over data that exists
anyway.

---

# Milestone 20 — Manual gameplay verification

The game was run in all four scenes, in release, on the target machine.

## What the runs show

* **Every scene starts, runs and shuts down cleanly.** `clean shutdown` on all four.
* **The D3D12 validation layer is silent** in all four scenes (`--validation --frames 400`):
  no errors, no warnings, no live-object leaks.
* **The environment loads as specified**: `environment 'combat_arena': 196 objects, 59
  collision shapes` / `196 objects, 240 render entities, 16 models uploaded`.
* **Frame rates** are in Milestone 17's table; the CPU wait for the GPU was 0.2% of frames.

## Movement, combat and camera

Unchanged, and unchanged deliberately — this phase touched no gameplay, no ability, no
targeting and no camera code. What backs that up:

| Requirement | Evidence |
| --- | --- |
| Player walks around the arena | `engine-collision/tests/movement.rs` (13 tests) and `engine-game/tests/collision_gameplay.rs` (14 tests), all passing unchanged |
| Player can jump and land | Same suites; the collision floor is the same plane it was |
| Player cannot leave the boundary | The ±40 wall is unchanged; `ArenaConfig::from_environment` takes its half-extent from the scene, asserted equal |
| Dummies remain selectable, abilities and damage work | `engine-game/tests/firemage.rs` (23 tests) and the headless simulation suite, all passing; the running game reports combat events and damage as before |
| AoE and target positioning | Unchanged; the clear-space rule exists precisely so an area indicator is never drawn through geometry |
| Camera behaviour | `ThirdPersonCamera` untouched |

## Environment visual checks

These are the checks the brief asks a person to make, written as GPU tests so they survive
into the regression suite (`engine-renderer/tests/environment_render.rs`, 7 tests, all
passing on the RX 6700 XT):

| Check | How it is verified |
| --- | --- |
| No visible gaps | Sky and ground both present and different; less than 5% of the frame is still the clear colour |
| No broken materials / missing assets | Every asset in the library draws more than 200 lit pixels from a camera framed on it |
| No incorrect scale | Each asset is framed by its own LOD0 bounds and fills the frame; the arena render places 196 objects at their authored sizes |
| No z-fighting | The paving is 2 cm above the plain and the crate's bands are proud of the box, both by construction; the floor-from-above test would show a tie as noise |
| No severe clipping | The kerb is inset 1.4 m from the collision wall |
| Environment readable, targets visible | Nothing over 1.35 m inside the 30 m combat space, asserted |
| Objects do not obstruct gameplay | Same assertion |

## What was not verified

**A human-eye aesthetic judgement was not performed by me.** I can state that the arena
renders, that every asset is on screen at the right size and colour, that the sky, ground,
boundary and background each contribute measurably to the image, and that no validation error
or visual-structure assertion fails. Whether it *looks* like a fantasy combat arena to a
person is a judgement a test cannot hold, and the composition reasoning behind it is written
up in Milestone 21 so it can be argued with.

---

# Milestone 21 — Visual quality pass

The composition decisions, made after the technical foundation worked:

**Object placement.** The scatter is placed on annuli rather than in a square, so density does
not pile up in the corners. Angles are jittered around an even spread rather than being wholly
random, because a wholly random angle clumps and a clump of pines against an empty arc reads
as a mistake. Radii are square-rooted so an annulus fills evenly by area rather than crowding
its inner edge.

**Scale relationships.** The arena is 80 m across for a 1.8 m character. The kerb is 1.15 m —
chest height, so it reads as a boundary you could see over rather than a wall you are inside.
The pillars are 4.6 m and the arch's crown is 6.5 m, which makes the gate the tallest thing in
the arena and therefore its focal point. The pines are 6.4 m, taller than the kerb, so the
treeline shows over the boundary from inside. The ridges are 250 m out and 70 m tall, which
puts them at about the same apparent height as the treeline — the reason they read as distant
rather than as large.

**Variation without a second asset.** Every pillar has a deterministic height between 0.94 and
1.08; every crate a random yaw and a scale between 0.85 and 1.15; every rock and plant a yaw
and a scale. A row of identical pillars is a row of clones; a row with 14% of height variation
is a set.

**Material consistency.** Nine textures for sixteen assets means every stone surface in the
arena is the *same* stone, and the eye reads the kerb, the pillars and the arch as one
structure rather than as three props that happen to be nearby.

**Visual hierarchy.** Warm against cool. The arena is lit by a warm sun into a cool ambient,
the ground and stone are desaturated, and the only saturated things in the scene are the four
braziers (emissive orange), the six banners (deep red with a pale device) and the dummies'
wraps. The eye goes to the fight because the fight is where the colour is.

**Composition.** The gate breaks the symmetry and gives the arena a front. The braziers mark
the four corners of the fighting space, which is what gives a 60 m expanse of paving a scale
the eye can measure. The crates are pushed against the kerb in pairs, because props in the
middle of a fighting space are props a player trips over.

**The arena is a place with a history.** The arch is broken, with a keystone hanging from one
side of the gap; the stone is weathered and mortared; the paving is worn into a plain rather
than laid on it. None of that costs a triangle over the alternative.

No VFX, no particles, no audio were added.

---

# Milestone 22 — Full regression

```
cargo fmt --all -- --check           clean
cargo clippy --workspace --all-targets -- -D warnings    clean
cargo test --workspace               1,572 passed, 0 failed
cargo build --workspace              clean
cargo build --workspace --release    clean
```

Every existing system verified:

| System | Status |
| --- | --- |
| Movement, jumping, collision | 90 collision unit tests + 22 integration tests, unchanged, passing |
| Collision demo scene | Runs; brings no environment; validation clean |
| Character demo scene | Runs; brings no environment; validation clean |
| Combat, target selection, abilities, talents, damage cycle | 201 `engine-game` unit tests + 23 FireMage integration tests, passing |
| Damage meter, telemetry | Passing; reported by the running game |
| UI | 278 `engine-presentation` unit tests + 6 UI budget tests, passing |
| Settings | Passing; loaded and saved by the running game |
| Rendering | 23 renderer integration test files, including the new `environment_render.rs`, passing on a real adapter |
| Existing benchmarks | `cull_bench` 6.0 ns/object at 50% visibility, 0 allocations (Phase 3: 5.5 ns/object — same order, run-to-run variance); `game_bench`, `ui_bench`, `collision_bench`, `performance_test` all build and run |

**No existing functionality regressed.** Two shared files changed behaviour, both
improvements:

1. `RenderList::group_by_material_and_mesh` no longer allocates per frame (Milestone 8).
2. `offscreen::render_scene`'s upload ring grew from 64 KiB to 1 MiB, because a 196-object
   scene exhausted it. Offscreen only; allocated once per capture.

---

# Milestone 23 — Final performance audit

## Against the Phase 1 baseline

The comparable pre-environment measurement is the character test bed, which brings no
environment at all and is otherwise the same binary, the same camera and the same UI:

| | Character bed | Combat arena, 0 dummies | Delta |
| --- | ---: | ---: | ---: |
| Visible objects | 5 | 85 | +80 |
| Draw calls | 10 | 31 | +21 |
| Triangles | 728 | 2,596 | +1,868 |
| GPU mean | 0.097 ms | 0.076–0.141 ms | within variance |
| CPU mean | 0.249 ms | 0.172–0.306 ms | within variance |

**Run-to-run variance on this machine (±0.06 ms on the CPU mean between identical runs)
exceeds the environment's cost**, so the honest statement is that the environment's CPU and
GPU cost at the shipping density is at or below the measurement noise of a short run. The
headless benchmark, which measures the passes rather than the frame, puts the environment's
own per-frame cost at **3.6 µs** (2.4 µs of level selection plus 1.2 µs of culling) — 0.09% of
a 240 FPS frame.

## The table

| Scenario | CPU ms | GPU ms | FPS | Draw Calls | Triangles | Visible Objects |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Empty arena | 0.172 | 0.076 | 5,808 | 31 | 2,596 | 85 |
| Player | 0.172 | 0.076 | 5,808 | 31 | 2,596 | 85 |
| 10 dummies | 0.203 | 0.092 | 4,936 | 33 | 3,316 | 105 |
| 50 dummies | 0.299 | 0.149 | 3,341 | 33 | 5,548 | 185 |
| 100 dummies | 0.318 | 0.156 | 3,147 | 33 | 7,948 | 272 |
| Worst view (headless, corner across) | 0.014¹ | — | — | 36 | 5,374 | 343 |
| Stress scene (×4) | 0.385 | 0.187 | 2,599 | 34 | 10,084 | 400 |
| Stress scene (×8) | 0.416 | 0.203 | 2,402 | 34 | 13,240 | 582 |

¹ The worst-view row is the headless per-pass total (14.07 µs of level selection, extraction,
culling and grouping), not a whole frame: the camera is placed directly rather than by the
gameplay controller, so there is no window and no presentation to measure. It is in the table
because the draw-call, triangle and visibility figures beside it *are* what the GPU would be
asked for.

**CPU and GPU frame time are the primary metrics and are what the decisions above were made
on.** The FPS column is derived and is included because the brief asks for it.

---

# Architecture

```text
                    EnvironmentScene            engine-environment
                           │                     (engine-core only)
                           ↓
                  Environment Objects
                           │
             ┌─────────────┼─────────────┐
             ↓             ↓             ↓
           Mesh         Material      Collision
      (engine-assets, procedural)   (engine-game ->
             │             │         engine-collision)
             └──────┬──────┘
                    ↓
               Render Data              engine-presentation
             (RenderWorld slots)        EnvironmentPlacer
                    │
             ┌──────┴──────┐
             ↓             ↓
          Culling          LOD
        (Culler)      (update_lod)
             │             │
             └──────┬──────┘
                    ↓
                 Renderer                engine-renderer
          group_by_material_and_mesh
          draw_batches_instanced
                    ↓
                   GPU
```

## The two questions the brief asks

### How would we add a second combat arena without changing the renderer or gameplay systems?

Write a second function in `engine_environment::arena` that returns an `EnvironmentScene`, and
name it from `Scene`. That is the whole change.

Concretely, for a "frozen keep" arena:

1. `pub fn frozen_keep() -> EnvironmentScene` — a new layout function, using the same
   `EnvironmentObject` records and any of the sixteen existing assets, with its own
   `EnvironmentLighting` (a low blue sun, a high ambient) and its own spawn points.
2. Add `Scene::FrozenKeep` to `engine-app::args` and one arm to the `match` that chooses a
   scene.

Nothing else. Not the renderer, which never learns a scene exists. Not `engine-game`, which
reads spawn points and collision primitives out of whatever scene it is handed. Not
`engine-presentation`, which places whatever objects a scene contains. Not the asset library,
unless the new arena wants a new *asset* — and adding one is a constant, a builder function
and one `match` arm in `environment_model`.

If the new arena needs a different boundary shape, that is content too:
`ArenaConfig::from_environment` takes its half-extent from the scene, and
`add_environment_collision` turns whatever boxes the scene describes into collision shapes.

### How would we add 1,000 instances of the same rock without creating 1,000 GPU resources?

Push 1,000 `EnvironmentObject`s naming `ROCK_SMALL` into the scene. Change nothing else.

The upload count is the number of **distinct assets**, not of objects:
`upload_environment` calls `create_model` once per library entry and hands the returned
`ModelBinding` to `EnvironmentVisuals`. Every placement of an asset refers to that binding's
`MeshHandle` and `MaterialHandle`, so a thousand rocks name one mesh and one material. The
counting sort in `group_by_material_and_mesh` puts all thousand in one bucket, the batch pass
emits one `RenderBatch`, and `draw_batches_instanced` issues one `DrawIndexedInstanced` with
an instance count of a thousand. The per-instance transforms and styles are composed straight
into the frame's upload ring.

The GPU gains: one instance-buffer allocation of 1,000 × 96 bytes in a ring that already
exists. It does not gain a mesh, a material, a texture, a descriptor or a pipeline state.

This is asserted, not described:
`engine-presentation::environment::a_thousand_rocks_share_one_mesh_and_one_material` builds
exactly that scene and checks the render world holds one mesh handle, one material handle and
a thousand distinct transforms. The stress measurements in Milestone 18 confirm it end to end
on a real GPU: 5.5× the objects, **one** extra draw call.

---

# Known limitations

* **No shadows.** The ambient is set high (0.34) specifically because there are none, which
  flattens contact between an object and the ground — a rock reads as sitting near the floor
  rather than on it. This is the single biggest visual gap and it is a measured GPU cost, so
  it belongs in a phase that budgets for it.
* **No depth fog.** The ridges use a constant emissive term standing in for atmospheric haze.
  It works from the arena and would not work from a camera that moved between the two.
* **Textures are duplicated between models.** Nine distinct images, eighteen uploads, 932 KiB.
  The fix is a renderer-side texture cache keyed by name.
* **The environment demo row shares the arena's collision world.** Walking into the exhibits
  is intended, but the row's own objects are placed by `exhibit()` with a hand-written
  collision arm rather than by the same table the arena uses — two lists that could drift.
* **`EnvironmentScene` is a function, not a file.** Changing the layout is a recompile.
* **A level change is a despawn and a respawn.** Cheap at 0.19 changes per frame, and it would
  not be cheap for a camera that teleported across a large world every frame.
* **The ground is flat and finite.** A 640 m quad; past it there is sky. The skirt on the dome
  hides the edge from inside the arena and would not from a camera at 200 m.
* **Nothing outside the arena wall has collision**, deliberately. If the arena is ever opened
  up, that rule has to be revisited — it is one line and one comment in `ring_scatter`.
* **The worst-view and complex-view scenarios are measured headless.** They place the camera
  directly, so there is no whole-frame CPU or GPU figure for them.

# Deferred work

* Shadow mapping, and re-balancing the ambient once it exists.
* Depth fog, replacing the ridges' emissive stand-in.
* A texture cache in the renderer, keyed by name.
* A scene *file* and a loader, when scenes are authored by someone not editing the source.
* Vertex colours or a second UV set, if the flat-tinted materials start to look repetitive
  before an artist replaces them.
* A second arena, which is the real test of the "add an arena without touching the renderer"
  claim.
* Occlusion culling. Frustum culling rejects 23–74% here; a scene with real interiors would
  want more.

# Future scalability considerations

* **Object count.** The level pass and the cull are both linear walks over flat arrays, at
  roughly 12 ns and 6 ns per object. A 10,000-object level costs about 180 µs a frame for the
  two, which is 4% of a 240 FPS budget — affordable. 100,000 is where they stop being.
* **Draw calls.** Bounded by distinct `(mesh, material)` pairs, not by object count. Growth
  comes from adding *assets*, not from adding objects. The counting sort's bucket array is
  `materials × meshes`, and it silently skips itself above 64 Ki buckets — at 256 materials
  and 256 meshes that limit is reached, and the grouping would fall back to gather order with
  no warning. **That is the next thing that will break as the library grows**, and the fix is
  a sparse key or a raised limit.
* **Streaming.** A world too large to place at load wants the scene split into cells, with
  `EnvironmentPlacer::place` called per cell. The placer already replaces cleanly and clears
  cleanly; what it lacks is a notion of a partial scene.
* **Instancing beyond one draw per batch.** Indirect drawing and GPU culling become worth it
  when the CPU cost of building the list dominates, which at these counts it does not.
* **Collision.** The static world's grid broad phase is rebuilt once at load and handles a
  thousand shapes comfortably; a moving level would need something incremental.
