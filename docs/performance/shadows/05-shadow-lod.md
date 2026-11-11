# Milestones 6 and 7 — shadow level of detail, and the local shadow budget

Two milestones in one document: distance-based shadow quality, and which local lights get a
shadow map at all. They are the same question asked of the two halves of the system — the sun
and the torches.

## What was already there

Worth naming before the changes, because most of the geometric half of this milestone was
implemented before the work started and is described in `docs/shadows.md`:

* **Caster geometry LOD.** `shadow_lod_for` picks `Full`, `Reduced`, `Simplified` or `None` per
  caster per view. Each cascade beyond the first steps down `levels_per_cascade` (1) levels, and
  a caster whose shadow would cover fewer than `cull_texels` (2, or 3 at Low) texels is not
  drawn at all. The stand-in meshes come from the model's own LOD chain.
* **A hard shadow distance.** Past the last cascade's far plane nothing is shadowed, and the
  last tenth of that range is a fade rather than an edge. 35 m at Low, 150 m at Medium and
  High, 320 m at Ultra — which is Milestone 5's table.
* **A local-light budget in views.** `MAX_LOCAL_SHADOW_VIEWS` is 16; a spot spends one and a
  point spends six; `local.max_distance` (40 m) rejects lights outright.

So the parts of the brief that ask for "0–20 m full quality, 20–50 m reduced, 50–100 m lower
resolution, >100 m none" were already answered for *geometry*, by cascade rather than by metres
— which is the same rule expressed in the units the shadow map actually works in.

Two things were missing: the **receiving** half of the level of detail, and a light-selection
rule that was more than "priority, then distance".

## What changed

### The filter's level of detail

`ShadowLodSettings::filter_reduction_cascade` (1 by default): cascades at or beyond that index
use `PcfKernel::cheaper()` instead of the full kernel. At Ultra that is six taps inside 15 m
and four beyond it.

**The floor is four taps, not one.** Four and six produce the same penumbra width at the same
radius (measured in Milestone 4: 6 pixels each), so stepping between them across a cascade
boundary is invisible. Stepping to a single tap would turn a six-pixel penumbra into a
one-pixel one — a seam drawn across the world at the split. `cheaper()` returns `Poisson4`
unchanged, so High and Medium, which already filter with four taps, simply have no reduction.

### The local-light importance formula

`select_shadow_lights` sorted by priority, then by distance. It now scores every candidate with
`shadow_importance` and spends the budget on the highest scores:

```text
importance = intensity
           × (range / distance)²        the light's angular size: screen-space influence
           × range fade                 zero at local.max_distance, so a map does not
                                        appear and vanish as the player steps back and forth
           × 4 ^ priority               the gameplay term
```

Distance is measured to the light's *reach*, not to the light, for the reason the range test
already used that measure: a big brazier lighting the room the player stands in is not far away
because its centre is.

Priority at a weight of 4 per step is deliberate and is asserted in both directions. A boss's
light at priority 2 outranks a decorative one four times closer — the case the brief names. And
a priority-1 light on the far side of the world does *not* outrank the light burning next to
the player, which is the failure mode of a priority-first sort and the reason this is a product
rather than a sort key.

### An importance floor

`LocalShadowSettings::min_importance` (0.02). A budget is a ceiling; this is a floor, and they
answer different questions. The ceiling stops a room full of torches costing sixteen views. The
floor stops a *sparse* scene from spending views it has to spare on lights whose shadow nobody
can see — a small lamp thirty metres away passes every rejection the selector has (in view, in
range, asked for one) and its map is four hundred texels of nothing. Without a floor, a budget
with room in it is always spent.

## Files changed

| File | What |
| --- | --- |
| `crates/engine-renderer/src/shadow/local.rs` | `shadow_importance`, the new ordering, the floor, 6 tests |
| `crates/engine-renderer/src/shadow/settings.rs` | `filter_reduction_cascade`, `min_importance`, `cheaper()`'s floor |
| `crates/engine-renderer/src/scene.rs` | `filter_lod` |
| `crates/engine-renderer/shaders/scene_frame.hlsli` | the per-cascade kernel choice, near and far |
| `crates/engine-environment/src/lighting.rs` | `push_stress_lights` takes a casting count |
| `crates/engine-app/src/args.rs`, `app.rs` | `--shadow-lights`, `filter-lod=`, `min-importance=`, the final budget line |

## Results — the filter LOD

GPU p50, three interleaved repetitions, `--shadows ultra`.

| Scene | Resolution | No reduction | From cascade 2 | **From cascade 1** |
| --- | --- | ---: | ---: | ---: |
| arena | 1280×720 | 0.216 | 0.215 | 0.214 |
| arena | 1920×1080 | 0.399 | 0.402 | 0.398 |
| arena | 2560×1440 | 0.647 | 0.646 | 0.640 |
| arena | 3840×2160 | 1.342 | 1.340 | **1.320** |
| shadow course | 3840×2160 | 0.881 | 0.880 | 0.872 |
| environment | 3840×2160 | 1.341 | 1.340 | **1.326** |

**This is a 1 % improvement and the document says so.** Reducing from the third cascade out is
worth nothing measurable in any scene tested; reducing from the second is worth 0.02 ms at 4K,
consistently, in three different scenes.

The reason is Milestone 4. Before it, the far cascades filtered with twenty-five taps and
stepping them down would have been worth a great deal. After it the near kernel is six taps and
the step is to four, so there are two fetches to save on a minority of the screen. The
mechanism is right and general — it is what makes a wider near kernel affordable if one is ever
wanted — but at the current tap counts its saving is 1 %.

It is enabled because it costs nothing to have: the step is invisible (both kernels give the
same penumbra), it is one comparison per pixel, and it is 1 % rather than 0.

## Results — the shadow budget

The arena with extra shadow-casting point lights added by `--lights N --shadow-lights N`. Every
brazier is a point light, so each one that wins asks for six views out of sixteen.

| Extra casting lights | Views granted | Lights refused | GPU p50 @1080p | GPU p50 @4K |
| ---: | ---: | ---: | ---: | ---: |
| 0 (4 lights, 2 casting) | 12 | 0 | 0.396 | 1.317 |
| 4 | 12 | 4 | 0.400 | 1.345 |
| 8 | 12 | 8 | 0.423 | 1.427 |
| 16 | 12 | 16 | 0.447 | 1.543 |
| 32 | 12 | 32 | 0.473 | 1.619 |
| 32, **none** of them casting | 12 | 0 | 0.467 | 1.577 |

The two rows that matter are the last two. Thirty-six lights all asking for shadow maps cost
**0.042 ms more at 4K** than the same thirty-six lights asking for none — 2.7 %. The budget
holds: twelve views are granted whatever happens, the shadow stage stays at 0.30 ms, and what
grows is the *shading* of thirty-two more lights, which is Milestone 1's curve and not this
one's.

Which twelve views: the two arena braziers the scene marks `casting_shadows(1)`. Thirty-two
stress lights at priority 0, scattered further out, are all refused — priority carrying two
lights past thirty-two competitors is exactly what it is for.

Across the presets, with twenty lights in the scene and sixteen of them asking:

| Preset | Views granted | Lights refused |
| --- | ---: | ---: |
| Medium | 0 | 18 |
| High | 6 | 17 |
| Ultra | 12 | 16 |

Medium grants none because it disables point lights entirely — one point light is six views and
a four-view budget with points enabled would be one torch and nothing else, which is the
existing preset rule and a test asserts it.

## What was *not* done, and why

**Per-light shadow-map resolution.** The brief asks for an atlas that can "use different
resolutions for different lights". It was considered and rejected, and the reason is a property
of this atlas rather than a general one.

The layout is fixed: a view's rectangle depends only on its index, and the module documents why
— a packed layout moves a light's tile between frames as the casting set changes, and a tile
that moves is a tile whose filtering bleeds across its neighbour's edge for one frame. Variable
tile sizes reintroduce exactly that: a tile's size and position would depend on the *set* of
selected lights and on their ranking, both of which change as the player walks.

It would also buy very little here. Local views are already 512² against a cascade's 2048², and
a torch lights a few metres where a cascade covers a hundred — so a local texel is already far
denser than a cascade texel. The whole local half of the shadow stage is a fraction of the
0.30 ms the stage costs at 4K, of which most is now the contact pass. Spending an
invalidation-prone packer on it would be optimising the wrong 3 %.

If it is wanted later, the shape that keeps the invariant is a *tiered slot layout* — slot 0 is
1024², slots 1..n are 512² — so a tile's rectangle still depends only on its index and the
ranking decides which light lands in which slot.

## Visual quality

* **The cascade-1 filter step is invisible**, by construction: four taps and six taps at the
  same radius give the same 6-pixel penumbra, which Milestone 4 measured. The existing
  `the_floor_shows_no_seam_where_one_cascade_meets_the_next` test covers the split and passes.
* **Light selection is stable.** The sort is `sort_by`, so lights that tie keep their authored
  order; the importance formula is continuous and the range term fades to zero rather than
  cutting, so walking towards and away from a light does not make its shadow map flicker in and
  out at a threshold.
* **The boss case works**: a priority-2 light three times further away than a decorative one
  takes the single available map.
* Six new unit tests in `local.rs` cover the inverse-square term, the intensity term, the range
  term, priority winning, priority *not* winning when it should not, and the floor rejecting a
  light while the budget has fifteen views free.

## Regressions

1. The filter LOD is 1 %. It is not the win the milestone list expected, and the reason it is
   not is that Milestone 4 already took that win.
2. `min_importance` is a new way for a light to get no shadow map. Set too high it would
   silently take shadows away from lights a designer expected to have them; 0.02 is about a
   brazier at twenty-five metres, and the final report line prints granted-and-refused counts
   so a scene that is losing shadows says so.

## Conclusion

**Keep both, enabled.** The budget is the real result: thirty-two extra shadow-casting lights
cost 2.7 % rather than eight times the shadow stage, and the two lights that matter keep their
maps. The filter LOD is a small, free, general improvement that will matter more if the near
kernel ever widens.

## Reproducing

```powershell
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 `
  -Configs "no filter LOD|ultra|filter-lod=4|;from cascade 2|ultra|filter-lod=2|;from cascade 1|ultra||"

powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Resolutions "1920x1080,3840x2160" `
  -Configs "as authored|ultra|||;8 casting|ultra|||--lights 4 --shadow-lights 4;20 casting|ultra|||--lights 16 --shadow-lights 16;36 casting|ultra|||--lights 32 --shadow-lights 32;36 not casting|ultra|||--lights 32"
```
