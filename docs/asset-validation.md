# Asset validation

How the engine checks an asset against [`style/ASSET_LIMITATIONS.md`](style/ASSET_LIMITATIONS.md),
what it does when the asset does not fit, and how to change either.

`style/ASSET_LIMITATIONS.md` is the specification. This document does not restate it: every
number the validator enforces is that document's, transcribed once into
`crates/engine-assets/src/asset_validator/limits.rs` and nowhere else. When the two disagree,
the specification is right and the table is a bug.

---

## Where it runs

```text
 Source .glb
     │
     ├─ import_glb          bytes to ModelAsset. No device, no GPU resource.
     │
     ├─ normalize           textures fitted to the runtime budget
     │
     ├─ VALIDATION          this document
     │
     └─ ModelAsset ──────►  engine-renderer uploads meshes and textures
```

Validation happens once per asset, at load or at import, **before** anything is uploaded. An
asset that fails costs a file read. Nothing here runs per frame, and nothing here ever will:
a model's triangle count does not change between frames, so there is nothing a frame could
learn by asking again.

Three entry points, for three different callers:

| Caller | Function | Behaviour |
|---|---|---|
| The game, loading a model | `disk::load_glb` | Structural rules only. First error refuses the load. |
| The asset pipeline | `disk::load_glb_validated` | Every rule. Errors refuse; warnings are logged. |
| The command line | `disk::validate_file` | Every rule, on the source file. Reports; refuses nothing. |

`load_glb` deliberately applies **only** the structural rules — dangling indices, unusable
weights, malformed textures — and not the art-direction budgets. Those need to know whether
the thing being loaded is a pebble or a cathedral, and the loader knows a file name. The
budgets are enforced by the pipeline and by `asset_check`, where the category is known.

---

## Running it

```text
cargo run --bin asset_check -- assets/                      a directory, recursively
cargo run --bin asset_check -- assets/props/torch.glb       one asset
cargo run --bin asset_check -- --category rock assets/rocks/
cargo run --bin asset_check -- --library --summary          the assets the game ships
cargo run --bin asset_check -- --help
```

Exit status is `0` when nothing failed and `1` when anything did — an error-severity finding,
or a file that could not be imported at all. Warnings do not fail a run unless
`--strict-warnings` is passed, which is what makes the tool usable as a build step: a build
that fails on advice is a build people learn to ignore.

`--summary` prints one line per asset, for a whole content tree.

---

## Severity: error, warning, info

| Severity | Meaning | Effect |
|---|---|---|
| `Error` | The runtime cannot draw it safely, or it is past a limit the specification calls hard. | The asset does not enter the library. |
| `Warning` | Legal and expensive, or legal and unusual. | It loads. Somebody should look. |
| `Info` | Worth saying. | Nothing. |

The distinction is the specification's own, between a **hard limit** and a **preferred
target**. A 4096 texture is inside section 25's hard limit and outside section 6's preferred
2048, so it warns and loads. It is never rejected for being 4096, and never silently accepted
either.

**Nothing is repaired.** No rule resizes a texture, decimates a mesh, renormalizes a scale or
regenerates a normal. A validator that quietly fixed things would make "it passed" mean
nothing. Reporting is this code's job; fixing is a tool's.

---

## Reading a failure

```text
Asset:    environment/rocks/giant_rock.glb
Category: large_prop
Category source: path

FAIL

Vertices:     72,341 / 15,000
Triangles:    118,422 / 12,000
Submeshes:    3 / 4
Materials:    2 / 6
Textures:     3
Max texture:  2048x2048 (24,576 KiB total)
LODs:         1 [118,422]
Collision:    not supplied
Size:         4.10 m across

Errors:
  [ASSET_VERTEX_LIMIT] lods[0]
  72,341 vertices is past the large_prop maximum of 15,000
  actual: 72,341
  limit:  15,000
  hint:   decimate or retopologize the mesh, and remove hidden geometry
```

Every finding carries a stable code, the part of the asset it is about, both numbers, and —
where there is a useful answer — what to do. The code is the part to depend on from a script;
the sentence is the part to read.

### Codes

| Code | Fires when |
|---|---|
| `ASSET_VERTEX_LIMIT` | LOD0, or one submesh, is past the vertex ceiling. |
| `ASSET_TRIANGLE_LIMIT` | A level of detail, or one submesh, is past the triangle ceiling. |
| `ASSET_MATERIAL_LIMIT` / `ASSET_MATERIAL_PREFERRED` | More material slots than allowed / than preferred. |
| `ASSET_MATERIAL_INVALID` | A parameter outside its range, or a NaN. |
| `ASSET_MATERIAL_REFERENCE` | A submesh names a material that does not exist. |
| `ASSET_TEXTURE_LIMIT` / `ASSET_TEXTURE_PREFERRED` | Past 4096 / past the category's preferred resolution. |
| `ASSET_TEXTURE_DATA` | Texels do not match the dimensions, a zero dimension, or a non-power-of-two size. |
| `ASSET_TEXTURE_FORMAT` | A normal or ORM map delivered as sRGB, or colour delivered as linear. |
| `ASSET_TEXTURE_REFERENCE` | A material names a texture that does not exist. |
| `ASSET_TEXTURE_COUNT` | Textures nothing samples. |
| `ASSET_MISSING_NORMALS` / `ASSET_INVALID_NORMALS` | No normal / a normal that is not unit length. |
| `ASSET_MISSING_TANGENTS` / `ASSET_INVALID_TANGENTS` | No tangent frame / a degenerate or badly signed one. |
| `ASSET_MISSING_UV` / `ASSET_INVALID_UV` | No unwrap / a coordinate that is not finite. |
| `ASSET_INVALID_INDICES` | An index past the end, or a count that is not a triangle list. |
| `ASSET_INVALID_GEOMETRY` | A non-finite position, or zero-area triangles. |
| `ASSET_EMPTY_MESH` | A model, level or submesh with nothing in it. |
| `ASSET_SUBMESH_LIMIT` / `ASSET_SUBMESH_PREFERRED` | More draws per instance than allowed / than preferred. |
| `ASSET_SKINNING_INVALID` | A joint past the palette, or weights that do not sum to one. |
| `ASSET_ANIMATION_INVALID` | A clip that does not match the skeleton. |
| `ASSET_LOD_INVALID` | The chain cannot be selected from, or a level is not a reduction. |
| `ASSET_LOD_MISSING` | Fewer levels than the category expects. |
| `ASSET_LOD_REDUCTION` | A level a long way from its recommended share of LOD0. |
| `ASSET_COLLISION_LIMIT` / `ASSET_COLLISION_MISSING` | Past 20 primitives / none where the category needs some. |
| `ASSET_INVALID_SCALE` | A transform that is not finite, is degenerate, or is wildly out of range. |
| `ASSET_BOUNDS_SUSPECT` | The asset does not measure like the thing it says it is. |
| `ASSET_GEOMETRY_DENSITY` | Triangles dense enough for the footprint to suggest raw generator output. |

Missing tangents are a **warning** on an asset nothing normal-maps and an **error** as soon as
a material samples a normal map: the shader has no basis to transform the map into, and the
result is a lighting bug rather than a missing detail.

---

## Categories

A category is what decides an asset's budget. The ten the specification implies, plus one:

```text
tiny_prop  small_prop  medium_prop  large_prop  hero_prop
architecture  hero_architecture  foliage  rock  tree
unclassified
```

`unclassified` is the runtime load path's category and the answer for anything the pipeline
has not been told about: only section 25's absolute limits apply, and none of the
per-category targets do.

**A category is never inferred from polygon count.** An asset that promoted itself to the
next budget up every time it went over budget would make the limits enforce nothing. "Hero"
is a decision somebody makes about one piece of content, and the validator only ever reads
it.

Where a category comes from, most specific first:

| # | Source | Reported as |
|---|---|---|
| 1 | `--category` on the command line | `command-line` |
| 2 | An asset-metadata system, where one supplies it | `asset metadata` |
| 3 | A keyword in the asset's path (`rocks/`, `foliage/`, `hero_architecture/`) | `path` |
| 4 | Nothing — the section 25 absolute limits | `fallback` |

Every report says which of the four it used:

```text
Asset:    assets/environment/rocks/large_rock.glb
Category: large_prop
Category source: path
```

```text
Asset:    assets/environment/rocks/large_rock.glb
Category: hero_prop
Category source: command-line
```

A budget an asset was *told* to meet and one it was *guessed* into are worth different
amounts of trust, and a report that does not say which invites an argument about the wrong
number.

There is **no metadata file**. The validator reads no sidecar, manifest or descriptor of its
own: the engine has no asset-metadata system yet, and inventing one here would make the
validator its owner — the wrong crate for that, and a format everything else would then have
to be taught. Source 2 is the slot such a system fills when it exists; nothing produces it
today except the environment library's own table in `asset_check`.

### The fallback, and what it is for

`unclassified` measures against section 25's absolute limits — 50,000 vertices, 40,000
triangles, 8 material slots, 4096 textures, 8 submeshes — and against none of the
per-category art-direction targets. It is the honest answer for an asset this specification
does not describe:

* a character
* a VFX mesh
* background scenery the size of the level

No budget is guessed for those. `ASSET_LIMITATIONS.md` is environment-focused, and a number
invented here would be a number nobody agreed to.

### Exceptions

Section 25 allows an asset past the absolute limits only when it is "explicitly marked as an
exception". That mark is `--exception` on the command line, or `AssetSpec::as_exception` from
a caller. It raises the ceiling from 50,000 vertices to 100,000 and from 40,000 triangles to
80,000, and it raises nothing else. It is deliberately something a person has to state.

---

## Scope

### Currently validated

| Area | Checks |
|---|---|
| Vertices | LOD0 total and per-submesh, against the category ceiling and the absolute one |
| Triangles | per level of detail and per submesh |
| Materials | slot count (hard and preferred), parameter ranges, dangling submesh references |
| Textures | resolution (hard and preferred), texel/dimension consistency, colour space per slot, power-of-two, textures nothing samples, dangling material references |
| Normals | present, finite, unit length |
| Tangents | present, finite, handedness ±1; an error once a material samples a normal map |
| UVs (structural only) | the attribute exists where required, one per vertex, every value finite |
| Indices | in range, a multiple of three, triangle-list shaped |
| Degenerate geometry | repeated indices and zero-area faces, counted |
| Numerical validity | NaN and infinity in positions, UVs, tangents, normals, material parameters, transforms |
| Scale | non-finite, zero, extreme and sheared import transforms; bounding-box sanity against the category's real-world size; triangle density for the footprint |
| LOD structure | level count, LOD0 at distance zero, strictly increasing thresholds, each level an actual reduction, reduction inside the recommended band |
| Collision | the representation the engine currently has — see below |
| Skinning and animation | joints inside the palette, weights that sum to one, clips bound to the skeleton |

### Intentionally deferred

Not implemented, and not pretended to be:

| Deferred | Why |
|---|---|
| UV overlap | Needs UV-island analysis: an offline pass, not an import-time one. |
| UV stretching | Same — it is a per-island measurement against 3D area. |
| UV padding | Same — it is a rasterization of the islands at a target resolution. |
| Texel density (section 9) | Same — needs island area against texture area. |
| Advanced collision primitive budgets | The engine's `EnvironmentCollision` is `none` or one box, so the count is 0 or 1. |
| Character budgets | `ASSET_LIMITATIONS.md` is environment-focused and states none. |
| VFX budgets | Section 20 states triangle counts but no category; they belong in their own specification. |
| `.meta` asset metadata | No metadata system exists, and the validator must not become its owner. |

Sections 8 and 9 of the specification still hold for artists. They are simply not machine-
checked yet, and this document says so rather than implying coverage that does not exist.

### The architectural line

```text
                    SOURCE ASSET
                         │
                         ▼
                 ┌─────────────────┐
                 │ Import / Parse  │
                 └────────┬────────┘
                          ▼
                 ┌─────────────────┐
                 │ asset_validator │   cheap structural checks
                 └────────┬────────┘
                          ▼
                    Runtime Asset
                          │
             ┌────────────┴────────────┐
             ▼                         ▼
       Future offline              Renderer
       asset processing
             │
       ┌─────┼─────────┐
       ▼     ▼         ▼
      UV   Retopo     LOD
```

`asset_validator` runs on **every** import, so it stays cheap: two walks per submesh, no
allocation per vertex, no texture decode, no mesh copy. It must never grow mesh decimation,
retopology, UV unwrapping, UV-island analysis, texture resizing or automatic repair. Those
belong in the offline stage on the left, which does not exist yet — and when it does, it is a
different tool with a different cost model. The validator reports; a processing tool fixes.

---

## Adding a rule

1. Add a variant to `ValidationCode` in `asset_validator/diagnostic.rs`, with its
   `ASSET_…` string. The string is an interface — scripts match on it — so it is spelled once
   and never inlined at a call site.
2. Put the number it enforces in `AssetLimits` or `CategoryLimits`
   (`asset_validator/limits.rs`), and make sure it comes from the specification.
3. Push a `Diagnostic` from whichever function in `asset_validator/mod.rs` already walks the
   data the rule needs. Do not add a pass over the geometry: the validator runs on every
   import, and its cost is the two walks it already makes per submesh.
4. A rule about art direction rather than about what the GPU can survive goes behind
   `limits.content_rules`, so the runtime load path keeps enforcing exactly what it enforced
   before.
5. Add a test beside the others in `asset_validator/tests.rs` that asserts on **the code**,
   not only on the verdict. Add its row to the code table above.

## Adding a category

1. Add a variant to `AssetCategory` and a row to `CATEGORIES`, in the same position — a test
   checks the two are in step, because a row out of order would silently mis-budget every
   asset of that category.
2. Take every number in the row from `style/ASSET_LIMITATIONS.md`. If the specification does
   not say, the specification is what to change first.
3. If the category needs a path keyword, add it to `category_from_path` in
   `asset_validator/spec.rs`, most specific first.

No validation logic changes for either.

---

## What the assets that exist today report

`cargo run --bin asset_check -- --library` checks the eighteen procedural environment models
the game ships. All eighteen pass. The warnings they carry are honest ones:

* `ASSET_LOD_MISSING` (14) — the library authors one or two levels where the specification
  asks for three or four.
* `ASSET_LOD_REDUCTION` (3) — a LOD2 that keeps more of LOD0 than the recommendation.
* `ASSET_COLLISION_MISSING` (3) — assets whose category expects collision and which are
  authored as decoration.
* `ASSET_INVALID_GEOMETRY` (3) — the triangle fans in `dome` and `sky_dome` have zero-area
  faces at their poles, which is what a fan does at a pole.
* `ASSET_BOUNDS_SUSPECT` (1) — `dome` is 45 m across, and architecture is 0.5-30 m.

None of these stop an asset. They are the list of what the specification would like the
content to become.

---

## TODO

* **Character budgets.** Characters currently fall back to section 25's absolute limits. They
  need their own specification — bone counts, submesh counts, texture budgets and LOD
  expectations differ from a prop's — and a category per tier once it exists.
* **VFX mesh budgets.** Section 20 gives triangle counts for VFX meshes; there is no category
  for them and none should be invented here. Same treatment as characters: a specification
  first, a category second.
* **Collision primitive types.** `collision_primitives` counts the two shapes
  `EnvironmentCollision` can express. When capsules, spheres, convex hulls or compound shapes
  arrive, extend that count and revisit the preferred threshold. No rule changes.
* **UV analysis.** Overlap, stretching, padding and texel density, in the offline processing
  stage — not here, and not with a new dependency added to `engine-assets` for it.
* **Asset metadata.** When a metadata system exists, it sets the category with source
  `asset metadata` and takes precedence over the path. Nothing else in the validator changes.
