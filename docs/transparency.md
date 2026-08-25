# Transparent rendering

What a transparent surface is in this engine, how it is drawn, what that costs, and what it
deliberately does not do.

The authoritative in-code statement of the same contract is the module documentation on
`engine_renderer::transparency`. This file is the version with the reasoning and the numbers.

---

## 1. The contract

Every material declares exactly one **surface class**. That declaration is the whole of the
engine's transparency vocabulary — there is no material graph, no per-object override, and no
inference from an alpha value.

| class | phase | depth test | depth write | blend | ordered by |
|---|---|---|---|---|---|
| `Opaque` | opaque | `LESS_EQUAL` | yes | none | `(material, mesh)` |
| `AlphaTest` | opaque | `LESS_EQUAL` | yes | none | `(material, mesh)` |
| `Transparent` | transparent | `LESS_EQUAL` | **no** | `src.a` over `1 - src.a` | back to front |

Three things follow from the table and nothing else does:

* **The class decides the phase.** An object is drawn in the opaque phase or the transparent
  one, never both.
* **The class decides two pipeline descriptors** — the blend state and the depth-stencil state.
* **The class decides nothing about shading.** A transparent surface runs the identical pixel
  shader an opaque one runs, with the identical material constants and the identical four-map
  descriptor table.

### Why alpha testing lives in the opaque phase

A cutout fragment either survives whole or does not exist. It never mixes with what is behind
it, so it needs no ordering, and it can write depth like anything else — which matters more
than it sounds: the depth buffer the opaque phase leaves behind is exactly what the transparent
phase tests against. Foliage that could not write depth would stop occluding the effects drawn
after it.

### Where the phase sits in the frame

```text
shadow stage -> opaque batches -> skinned -> TRANSPARENT -> particles -> overlay -> UI
```

* **After every opaque draw**, because a blended surface has to test against a finished depth
  buffer.
* **Before the particles**, because both are blended, they share a screen, and a spell's card
  and its sparks belong to one effect.
* **Before the interface**, because a health bar drawn underneath a fireball is a health bar
  nobody can read.

---

## 2. What the renderer does, step by step

1. The cull produces one visible list, exactly as it always did.
2. `RenderList::split_transparent` moves every entry whose material is `Transparent` into a
   second list. What remains is byte-identical to the list the opaque phase received before
   this feature existed — asserted by a test, not assumed.
3. The opaque list is grouped by `(material, mesh)` and recorded unchanged, batched or
   instanced according to the same settings as before.
4. The transparent list is ordered by `RenderList::sort_back_to_front`: farthest first, by
   squared distance from the camera, ties broken by position in the list.
5. Each transparent object is recorded as its own draw, in that order, bound to a pipeline
   state whose blend is enabled and whose depth write mask is zero.

### Why the split happens after the cull and not inside it

The culler answers "is it on screen", which it can do from the world's bounding spheres alone.
Giving it a material table would put a shared lookup inside a loop that runs over every entity
in the world and is chunked across worker threads — to answer a question that can be asked once
afterwards, over the visible set only.

### Why the split happens before the grouping

The opaque phase's counting sort keys on `(material, mesh)`. A transparent entry left in it
would be batched by material and drawn with depth writes on, in an order decided by a
histogram. The grouping has to see the opaque set and only the opaque set.

### Why the transparent phase is not batched or instanced

Its order is fixed by distance; grouping or instancing would reorder it, and an instanced draw
gives the GPU no ordering guarantee between its instances. Adjacent entries that happen to
share a mesh and a material still cost one set of binds, because `CommandContext` drops a
redundant bind — so the runs a sort happens to produce are exploited without the order ever
being changed to produce one.

The phase is small by construction: it is for effects, not for scenery.

---

## 3. Cost

Per frame, for a scene with **no** transparent material: one predicate pass over the visible
set's material handles. No writes, no copy, no allocation. That is what makes it safe to run
unconditionally.

For a scene that has them:

| step | cost |
|---|---|
| split | one linear pass; transparent entries copied into the second list, opaque entries compacted in place |
| sort | one `(f32, u32)` key per transparent object, then an unstable sort — `n log n` on a list of effects, not of scenery |
| record | one draw per transparent object, plus a pipeline-state change on entering the phase |

Both lists reuse their allocations across frames, so after the scene reaches its high-water
mark neither the split nor the sort allocates. A unit test asserts this.

The transparent pipeline states are created on first use and cached beside the opaque ones,
keyed by `(features, wireframe, surface)`. `AlphaTest` and `Opaque` produce the same state
object and share a cache entry — the cutout is a `discard` driven by the material constant, not
a pipeline setting.

---

## 4. Authoring

```rust
// Opaque. The default; nothing to say.
let stone = MaterialDesc::colored(0.5, 0.5, 0.5);

// Alpha tested. The threshold is what makes it a cutout.
let leaves = MaterialDesc::colored(0.2, 0.7, 0.2).with_alpha_cutoff(0.5);

// Transparent. `with_opacity` sets the alpha and the class together.
let ghost = MaterialDesc::colored(0.6, 0.8, 1.0).with_opacity(0.35);
```

Imported models carry glTF's `alphaMode` across unchanged: `OPAQUE` → `Opaque`, `MASK` →
`AlphaTest` with its `alphaCutoff`, `BLEND` → `Transparent`. Before the transparent phase
existed the importer turned `BLEND` into a cutout, because a leaf card with a hard edge was a
closer answer than an opaque rectangle; that workaround is gone.

The class and the cutoff must agree, and `Material::new` rejects them when they do not: a
transparent material carrying a cutoff would discard the fragments it was meant to fade, and an
opaque one carrying a cutoff would punch holes in itself in the opaque phase. Both look like
shader bugs and are material bugs.

### A ghost that should not cast a solid shadow

Shadow casting is a property of the **entity**, not of the material, and it always has been.
Spawn with `ShadowCasting::RECEIVES_ONLY` and the ghost is lit and shadowed without blocking the
sun.

---

## 5. Known limitations

Named here rather than discovered later.

* **Sorting is per object, by origin.** Two transparent surfaces that intersect have no correct
  back-to-front order at all, and the intersection shows a hard seam. A large or concave
  transparent mesh is answered for by its origin, so it can be sorted wrongly against a small
  one nearby. Per-fragment ordering is the only real fix and is deliberately not built.
* **Transparent surfaces are single-sided.** They are rasterized with the same back-face culling
  opaque geometry uses, so a transparent quad seen from behind is invisible. Author effect
  geometry facing the player, or build it as two quads.
* **Skinned characters are drawn in the opaque phase.** A skinned material that declares itself
  transparent does get the blended pipeline state, but it is recorded with the characters rather
  than sorted among the rigid transparent objects.
* **Equal distances are drawn in list order.** Stable and repeatable, but arbitrary. Give two
  layers distinct depths if which one is on top matters.
* **The transparent phase does not receive its own contribution.** Nothing reads the colour
  target, so there is no refraction and no screen-space transparency.

## 6. Deliberately not implemented

* Order-independent transparency of any kind — no weighted-blended OIT, no per-pixel linked
  lists, no depth peeling.
* Refraction, and anything else that samples the colour target.
* Per-triangle or per-fragment sorting.
* A material graph. The three classes are the whole vocabulary.
* HDR, bloom, fog and stylized lighting. None of them is a prerequisite for blending, and
  transparency was not the place to introduce them.
* Deferred rendering. The renderer is still forward.

---

## 7. The debug scene

`engine_renderer::transparency::debug_scene()` is the smallest scene that exercises every rule
above:

| object | class | what it demonstrates |
|---|---|---|
| `opaque_cube` | `Opaque` | the opaque path untouched, and something for the quads to blend over |
| `foliage_cross` (two quads) | `AlphaTest` | a cutout above its threshold draws unblended and writes depth |
| `foliage_discarded` | `AlphaTest` | a cutout below its threshold vanishes — the test really fires |
| `transparent_quad` | `Transparent` | one blended layer over opaque geometry |
| `transparent_far`, `transparent_near` | `Transparent` | two overlapping layers, which only look right composited farthest-first |

It is plain data — meshes, materials, transforms — so the pixel tests and the interactive viewer
draw the same objects rather than two similar sets.

Run it interactively:

```
cargo run -p engine-showcase -- --transparency
```

---

## 8. Tests

| what | where |
|---|---|
| opaque and alpha-tested pipeline state is byte-for-byte unchanged | `src/pipeline.rs` |
| transparent state tests depth, writes none, blends straight source-alpha | `src/pipeline.rs` |
| a list with no transparent material comes out of the split identical | `src/render_list.rs` |
| the split moves exactly the transparent entries and preserves both orders | `src/render_list.rs` |
| the sort is farthest-first, deterministic, and total even with a NaN transform | `src/render_list.rs` |
| the split and the sort stop allocating | `src/render_list.rs` |
| the debug scene covers every class and its cutouts straddle one threshold | `src/transparency.rs` |
| an opaque surface renders unchanged and still occludes | `tests/transparency.rs` |
| a cutout above its threshold is unblended; below it, absent | `tests/transparency.rs` |
| a transparent surface blends, over the clear and over opaque geometry | `tests/transparency.rs` |
| a transparent surface is occluded by nearer opaque geometry | `tests/transparency.rs` |
| overlapping transparent surfaces both contribute — no depth write | `tests/transparency.rs` |
| the picture does not depend on authoring order — the sort decides | `tests/transparency.rs` |
| repeated renders of one scene are pixel-identical | `tests/transparency.rs` |
| the whole debug scene renders with every class at once | `tests/transparency.rs` |

The pixel tests state the arithmetic they expect rather than comparing against a stored image: a
screenshot comparison says a frame changed, and these say which rule broke.
