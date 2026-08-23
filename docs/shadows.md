# Shadows

What the engine draws shadows with, why each piece is the way it is, what it costs, and how
to find out when it goes wrong.

The code is `crates/engine-renderer/src/shadow/` (nine modules), the shader half is
`crates/engine-renderer/shaders/scene_frame.hlsli` and `shadow_depth.hlsl`, and the settings
the player sees are `crates/engine-settings/src/graphics.rs`. Everything below is about those
files; where a number appears here it is the number in the code, and where the two disagree
the code is right and this document is stale.

## The shape of a frame

```text
extract ──► RenderWorld ──┬─► shadow.prepare()     cascades, caster lists      CPU
                          │
                          ├─► shadow.record()      depth-only, into the atlas   GPU
                          │       one viewport per view, no render target
                          │
                          ├─► depth prepass        only when contact shadows are on
                          │       + CopyResource into a sampleable depth texture
                          │
                          └─► mesh pass            samples the atlas, shades      GPU
```

The shadow stage sits between `begin_recording` and the mesh pass in
`Renderer::render_frame`, bracketed by its own GPU-timer query pair, and it is the same code
the offscreen test path runs — `offscreen::render_scene_shadowed` drives the real
`ShadowRenderer`, not a copy of it, which is what makes a pixel test evidence about the game.

There is one new pass and one conditional one. Nothing else about the frame changed shape.

## Directional light: cascaded shadow maps

### Splitting

`cascade::split_cascades` divides the view distance into up to four slices at the distances a
preset names — 15 m, 50 m, 150 m, 320 m by default. Practical-split schemes (a blend of
logarithmic and uniform) were not used: the distances are content decisions here, and a
scene whose combat happens inside thirty metres wants a near cascade sized for combat rather
than for a formula.

### One sphere per cascade, anchored at the origin

Each slice's eight frustum corners get a **bounding sphere**, not a bounding box. A sphere's
radius does not change when the camera rotates, so the cascade's extent — and therefore its
texel size — is constant as you turn. A box's is not, and a shadow whose resolution changes
while you turn shimmers.

The light's view matrix then looks from the **world origin**, not from the cascade's centre:

```rust
let view = Mat4::look_at(Vec3::ZERO, direction, up);
```

This is the subtle half of stabilisation and it was got wrong once during implementation. If
the eye follows the cascade centre, the light space itself translates as the camera moves,
and snapping a coordinate to a texel grid inside a space that is itself sliding snaps nothing.
With the eye pinned, light space is fixed to the world, and snapping the centre's light-space
position to a texel makes the cascade advance in whole texels:

```rust
let texel = diameter / resolution;
let snap = |v: f32| (v / texel).floor() * texel;
```

`stabilize` can be turned off in `ShadowSettings`; it exists so a test can show the difference
rather than because anyone should.

### Extrusion, not a tight near plane

The orthographic near plane is pushed back by `caster_extrusion` (60 m) and the **atlas**
depth pipelines are created with `DepthClipEnable: false`. An object behind the cascade's
near plane still casts into it — a wall outside the slice still shadows the ground inside
it — without needing a second "caster bounds" pass to widen the projection.

**The camera's depth prepass is the exception, and getting that wrong was a real bug.** The
prepass shares the shadow module's pipeline code, so it originally inherited the clipping
setting too. For the camera that is wrong: the main pass clips, so the prepass has to clip.
With clipping off, the part of a surface between the eye and the near plane is *flattened
onto depth 0* instead of being discarded, and every real fragment behind it then fails the
main pass's `LESS_EQUAL` and is never shaded — a swathe of unlit background with no shadow
in it, moving as the camera turns, and only at High and Ultra because only they run a
prepass. `shadow/pipeline.rs` now sets `DepthClipEnable: is_prepass`, and a unit test asserts
the two passes get opposite settings.

### Blending

`blend_fraction` (0.1) of each cascade's depth is a cross-fade with the next one, computed by
`cascade_blend` on the CPU side and applied in `directional_shadow` in the shader by sampling
both cascades and lerping. The seam is what a player would otherwise see as a line drawn
across the ground; `tests/shadow_render.rs::the_floor_shows_no_seam_where_one_cascade_meets_the_next`
asserts it is not there.

## Bias

Three biases, because they fail differently:

| Bias | Where it is applied | What it fixes | Default |
| --- | --- | --- | --- |
| constant depth | rasterizer state, baked per PSO | flat surfaces facing the light | 200 units |
| slope-scaled | rasterizer state, same PSO | surfaces at a glancing angle | 2.5 |
| normal offset | the shader, before projecting | the rest, without peter-panning | 1.4 texels, capped at 0.35 m |

Depth bias lives in the pipeline state rather than in a constant because D3D12 puts it there;
that is why there are two variants of every depth pipeline — a biased one for shadow views and
an unbiased one for the camera's prepass, where bias would be a lie about scene depth.

The normal offset is scaled by `cascade_texel_world`, so "1.4 texels" means the same physical
nudge in every cascade instead of being invisible near and enormous far.

## Filtering

`PcfKernel` is `Single`, `Poisson4` (the default above Low), `Poisson6` (Ultra), `Poisson8`, or
one of the two grids `Tap3x3` and `Tap5x5`. Every tap goes through a `SamplerComparisonState`
with `LESS_EQUAL` and a white border, so the hardware does the comparison and the bilinear
filter in one fetch — a six-tap disc is six fetches covering twenty-four texels. Each width is
its own unrolled branch, because a dynamic loop around a texture fetch is markedly slower and
costs registers in the frame's largest shader.

### The disc

`vogel_disc` places `n` taps on a golden-angle spiral of `filter_radius_texels` (2.5), rotated
per pixel by interleaved gradient noise. A golden-angle spiral rather than a stored Poisson
set, because it is well spread for *any* tap count — 4, 6 and 8 are one piece of code and one
radius to tune rather than three tables to keep in agreement.

**The rotation is a function of the pixel and of nothing else.** There is no temporal
accumulation in this engine and no TAA, so an angle that changed per frame would turn a static
penumbra into boiling noise. Fixed per pixel it is a dither that stands still.

Measured against the 5×5 grid it replaced, on the same scene: the same 6-pixel penumbra, drift
on a one-centimetre camera move of 0.034 against 0.031, local noise 0.108 against 0.064 — and
six fetches instead of twenty-five, which is 14 % of the frame at every resolution. The
four-tap disc beats the 3×3 grid on penumbra, noise *and* stability for five fewer fetches.
`docs/performance/shadows/03-shadow-filtering.md` has the radius sweep and the rest.

The two grids are kept so the tests have something to compare against; no preset selects the
5×5 any more.

`clamp_to_tile` keeps every tap half a texel inside its own atlas rectangle. Without it a
kernel at a tile edge reads its neighbour, and one light's depth showing up inside another's
shadow is a bug that looks exactly like a light leak.

## Culling and levels of detail

`ShadowCasterCollector` walks the same `RenderWorld` the camera culls, once per view, and
keeps a caster when its bounds intersect the view's volume. It reuses `RenderList` and the
counting-sort batching the main pass uses; there is no second scene representation.

Then `shadow_lod_for` picks one of four levels:

| Level | Meaning |
| --- | --- |
| `Full` | the geometry the camera draws |
| `Reduced` | one level coarser, if the entity has one |
| `Simplified` | the coarsest stand-in the entity has |
| `None` | not drawn into this view at all |

Two rules choose it: each cascade beyond the first steps down `levels_per_cascade` (1) levels,
and any caster whose shadow would cover fewer than `cull_texels` (2, or 3 at Low) texels drops
to `None`. A pebble at 140 m occupies a fraction of a texel of the last cascade; drawing it is
a draw call that changes nothing, and a scene made of pebbles is thousands of them.

The stand-in meshes come from the model's own LOD chain —
`engine_presentation::environment::shadow_lods` fills them — so a shadow LOD is never a
separate asset somebody has to remember to author.

## Who casts and who receives

`engine_render_data::ShadowCasting` carries two flags and up to two stand-in meshes, per
renderable. Opaque scene geometry is `CASTS_AND_RECEIVES` by default. Deliberate exceptions:

| Thing | Setting | Why |
| --- | --- | --- |
| ground / floor | `RECEIVES_ONLY` | a flat plane's shadow is the plane |
| ridges (distant backdrop) | `RECEIVES_ONLY` | they stand past every cascade |
| the sky dome | `NONE` | a shadow across the sky is unexplainable |
| target markers, decals | `NONE` | they are already a lie about lighting |
| particles, VFX | `NONE` | thousands of quads for a shadow nobody reads |
| debug geometry, UI | `NONE` | not part of the world |
| skinned characters | casts and receives | the shadows a player actually watches |

Scene props go through `engine_presentation::environment::base_casting`, which is the one
place that decision is made for the environment library; gameplay entities go through
`extract`, and the two agree on floors deliberately.

## Alpha-tested casters

A cutout material's silhouette is in its texture, so the depth pass has to run a pixel shader
that `discard`s. `ShadowSurface::AlphaTested` selects a PSO with `PSAlpha` from
`shadow_depth.hlsl` and `CULL_MODE_NONE`; `ShadowSurface::Opaque` has a **null pixel shader**,
which is the fast path and stays the fast path for almost everything. The cutoff comes from
the same `material.hlsli` constant buffer the mesh pass reads, so the two cannot disagree
about where a leaf ends.

Content that exercises it: `leaf_screen` in the environment library, and bay D of the shadow
course.

## Local lights

### Which lights a pixel evaluates

The frustum is a fixed grid of clusters — 16 tiles across, 9 down, 24 exponentially-spaced
depth slices — and every frame each light is assigned on the CPU to the clusters its bounding
sphere reaches. One `u32` per cluster, bit `i` for light `i`; the shader finds its cluster
from `SV_Position` and the view depth and walks the mask with `firstbitlow`.

```text
   screen tiles x depth slices          a fragment evaluates the lights of one cluster,
   +----+----+----+                     which in the shipping arena is one or none, and
   |    |    |####|  <- one cluster     in a 32-light scene is at most nine
   +----+----+----+
```

The grid is a `ByteAddressBuffer` in the upload ring, bound as a **root SRV** at `t3`. It was
in the frame constant buffer first and that cost 0.34 ms a frame at 4K: 13.5 KB took the
buffer past the hardware's constant cache and made every read of it slower, cascade matrices
included. `crates/engine-renderer/src/light_cluster.rs` carries the rest of the reasoning.

Assignment is on the CPU and output-sensitive — a light walks the clusters its projected
bounds cover, not all 3456 — which is why there is no compute pass: the renderer has no
compute pipelines at all, and one for a table this small would be the larger change.

`--lights <count>` adds decorative point lights to any scene as a stress control, and
`--shadow-tweak clusters=off` switches the grid off so the two paths can be compared in one
binary.

### The lights themselves

`engine_render_data::LocalLight` is a point or a spot. A light asks for shadows with
`casts_shadow` and a `priority`; the renderer decides. `select_shadow_lights` is greedy over
in-view → priority → distance, and it **skips rather than stops**: a point light that does not
fit is passed over and a later spot light that does fit still gets its view.

The budget is counted in **views**: a spot is one, a point is six (the cube faces of
`CUBE_FACE_DIRECTIONS`). Local views are 512² — a torch lights a few metres where a cascade
covers a hundred, so its texels are already far denser at half the side, and halving the side
buys four times as many views out of the same atlas.

Range is `local.max_distance` (40 m). Past that a light lights but does not shadow, which is
why bay F of the shadow course has to be walked into to be judged.

## Contact shadows

Short-range screen-space shadows for the contact region a 2048² cascade cannot resolve: a
foot on the ground, a crate against a wall. `contact_shadow` marches `contact.steps` (8)
samples along the light direction through this frame's depth buffer, up to `contact.length`
(0.35 m), with a `thickness` (0.06 m) test to avoid shadowing through thin geometry. It only
ever *reduces* the shadow factor, never invents one, and it is disabled past
`contact.max_distance` (20 m at Ultra, 15 m at High).

### It is a pass, at half resolution

The march runs in `shaders/contact.hlsl`, as a fullscreen triangle into a target of its own at
`contact.divisor` (2) on each axis — a quarter of the rays. It used to be a function called
from the mesh pixel shader, and at 4K that was 1.31 ms of a 2.50 ms frame, the largest single
item in it. It is 0.38 ms now.

The target is `R16G16_FLOAT`: red the shadow factor, green the view distance the ray started
from. The green channel is what makes half resolution look like full resolution —
`contact_shadow_at` in `scene_frame.hlsli` weights each of the four neighbouring texels by how
well its recorded distance agrees with the pixel being shaded, so a foot's contact shadow does
not smear onto the wall behind it, and where no neighbour agrees the answer is "unoccluded"
rather than a guess.

The normal the march lifts its origin along is reconstructed from the depth buffer's own
screen-space derivatives: this is a forward renderer and there is no G-buffer to read one from.

The gating stayed in the mesh shader — `receives_shadow` and the sun-facing test — so a
surface that does not receive shadows is still not darkened by this.

`docs/performance/shadows/02-contact-shadows.md` has the measurements, including why quarter
resolution and a four-step march were both rejected.

The march starts **half a step off the surface along the normal**. Starting on the surface
itself makes the early samples project back onto that same surface, so on a floor the march
reads the floor's own screen-space slope as an occluder; below a certain eye height that
difference lands inside the thickness window across most of the ground at once, and which
part depends on the camera's angle — grey that swims about as you turn.

It needs the camera's depth as a texture. The engine runs a depth prepass and then
`CopyResource`s the depth buffer into a plain sampleable texture. **The alternative was a
read-only DSV** — binding the same resource as depth-write and as an SRV simultaneously —
which avoids the copy but requires every mesh PSO to exist in a second variant with a
read-only depth-stencil state. One copy of a depth buffer per frame is cheaper than doubling
the pipeline count, and much cheaper to reason about.

One consequence worth knowing: the depth prepass forced `depth_stencil_desc()` from `LESS` to
`LESS_EQUAL`, because after a prepass the main pass draws each surface at exactly the depth
already there. That change is why `tests/depth_ordering.rs` now asserts that coplanar surfaces
keep the *last* one drawn.

## The atlas

One `R32_TYPELESS` texture, a `D32_FLOAT` DSV to draw into and an `R32_FLOAT` SRV to sample,
cleared to 1.0. Every cascade and every local view is a rectangle inside it:

```text
tile = one cascade's resolution

+--------+--------+
| casc 0 | casc 1 |    cascades take slots in order; local views fill the
+--------+--------+    slots left over, then rows beneath, each slot
| casc 2 |LLLL LLLL|   subdivided into 512-texel local tiles
+--------+--------+
```

### Why an atlas rather than a texture per light

§24 of the plan asked for this decision to be justified rather than assumed. For **this**
backend it is an atlas, for three reasons that are properties of this renderer and not general
truths:

* **One shader-visible descriptor heap.** The renderer binds a single CBV/SRV/UAV heap per
  frame and never switches it. A texture per light is a descriptor per light and a table bind
  per light in the lighting pass; the atlas is one SRV bound beside the material table.
* **Root arguments are scarce.** Per-light maps need a table per light or an unbounded array.
  The atlas needs neither: a light's tile is four floats of UV rectangle in a constant buffer
  the pass already binds.
* **One barrier.** The stage ends with a single `DEPTH_WRITE → PIXEL_SHADER_RESOURCE`
  transition instead of one per map.

The cost: every view shares one format and one resolution class, and a cascade cannot be
resized without recreating the whole atlas. Neither bites — the cascades are one size by
construction, and a resolution change already recreates the resource and stalls once.

The layout is **fixed, not packed**. A packer would waste less space and would also move a
light's tile between frames as the casting set changed; a tile that moves is a tile whose
filtering bleeds across its neighbour's edge for a frame. A view's rectangle depends only on
its index.

## Quality presets

`ShadowPreset` in `shadow/settings.rs` is the only place a quality level becomes numbers.
Every setting the player can choose is implemented — there is no preset field that the
renderer ignores.

| | Off | Low | Medium | High (default) | Ultra |
| --- | --- | --- | --- | --- | --- |
| cascades | – | 2 | 3 | 3 | 4 |
| resolution | – | 1024 | 1024 | 2048 | 2048 |
| PCF | – | 1 tap | 4-tap disc | 4-tap disc | 6-tap disc |
| local views | – | 0 | 4 (no point lights) | 8 | 16 |
| contact shadows | – | off | off | on, 8 steps, 15 m | on, 8 steps, 20 m |
| atlas | 0 | 2048×1024, 8 MB | 2048×2048, 16 MB | 4096×4096, 64 MB | 4096×5120, 80 MB |

Medium turns point lights **off** rather than rationing them: one point light is six views, so
a four-view budget with points enabled is one torch and nothing else. High's eight views fit
in the quarter of the atlas three cascades leave empty, so they cost no extra memory at all.
A test (`a_preset_that_allows_point_lights_can_afford_one`) fails any preset that permits
point lights on a budget smaller than six — the bug it exists to prevent is a setting that is
silently never applied, which is what High shipped with for an afternoon.

`ShadowSettings::validated()` repairs rather than rejects: out-of-range values are clamped,
resolutions are rounded to a power of two, a hand-edited cascade list is re-sorted. A shadow
configuration is never worth failing a frame over.

## Settings and controls

The player's setting is `graphics.shadows` in `settings.cfg`, one of `off`, `low`, `medium`,
`high`, `ultra`, persisted by the existing settings system and validated on load.

| Control | What it does |
| --- | --- |
| `F6` | cycles the quality and **saves it**, the same as changing it on a settings screen |
| `F7` | cycles the debug view: off → cascade tint → shadow factor → the atlas → caster bounds |
| `F5` | the developer overlay, which carries the shadow statistics block |
| `--shadows <level>` | overrides the setting for one run **without** writing it back |

`--shadows` deliberately does not persist: it is a measurement control, and a benchmark run
must not change what the player sees next time they start the game.

Changing quality recreates the atlas, which means `wait_for_idle()` — a stall. That is why it
belongs on a settings screen and on F6, not on a slider that fires every frame.

## Debugging

* **F7 view 1 — cascade tint.** Each pixel takes the colour of the cascade it sampled, blended
  across the transition. A visible band edge on the ground means blending is off or too narrow.
* **F7 view 2 — shadow factor.** The factor alone on a neutral surface, with albedo and
  lighting out of the way. Acne, peter-panning and filter width are all judged here.
* **F7 view 3 — the atlas.** The depth texture itself, drawn bottom-left with the cascade
  tiles outlined in red. Empty is white. This is where "the third cascade is empty" and "a
  light got no tile" become one glance instead of a debugger session.
* **F7 view 4 — caster bounds.** A yellow sphere at every caster's bounding sphere and a
  small blue one at every receiver's, drawn from the same arrays the culler reads
  (`engine_presentation::shadow_debug`). This is the view for "why is that thing not casting":
  a sphere in the wrong place or wildly the wrong size is a bug in extraction, not in the
  shadow pass. It is CPU geometry rather than a shader mode, which is why the F7 cycle has
  five stops and the renderer only knows about four.
* **F5 overlay.** Quality, cascade count, resolution, atlas size and bytes; CPU ms, GPU ms,
  draws, triangles; casters, skinned draws, dropped-by-LOD, prepass on/off.
* **`--report <seconds>`.** The same numbers on their own log line, plus how many local views
  were granted and how many were refused.

The debug views are not reachable from the game UI, by design: they are developer tools, and
F7 is behind no menu a player opens.

## Testing

Non-GPU logic is unit-tested where it lives: splitting, sphere fitting, stabilisation,
cascade selection and blending, LOD choice, atlas rectangles (including a non-overlap
assertion), light selection and the budget, caster visibility, and every preset.

GPU behaviour is asserted in pixels, offscreen, through the real shadow stage:

* `tests/shadow_render.rs` — 11 tests: a caster darkens the floor; shadows off is evenly lit;
  `receives = false` is not darkened; `casts = false` leaves nothing; no acne at any preset;
  every preset renders a shadow; alpha-tested casters; no cascade seam; spot shadows; point
  shadows; contact shadows do not darken open ground.
* `tests/shadow_course_render.rs` — the shipping shadow course, not a synthetic scene: bay A
  casts a shadow that disappears when shadows are switched off, no pixel gets *brighter* with
  shadows on, bay B's hundred crates are all casters, and the atlas debug view draws the atlas.
  `every_preset_that_draws_a_shadow_draws_it_from_every_angle` sweeps 24 viewpoints across
  three eye heights and bounds High against Medium in both directions — it is what catches a
  bug that only appears at some camera angles at some presets, which both of the bugs above
  were.
* `tests/root_signature_identity.rs` — the mesh, instanced and shadow pipelines share one root
  signature object, which is what lets the shadow pass rebind nothing but a PSO.

The visual test bed is `--scene shadows` (`engine_environment::shadow_course`): six bays on
flat ground — an isolated caster, a hundred crates in a grid, a line of rocks crossing every
cascade split, alpha-tested foliage, spot lights, point lights. Flat ground on purpose: terrain
hides acne by making it look like shading.

## What it costs

Release build, RTX-class desktop GPU, `--scene shadows` (152 objects, ~230–390 casters), mean
over 900 frames:

| Preset | Frame GPU (mean) | Shadow stage CPU | Shadow stage GPU | Shadow draws | Casters |
| --- | --- | --- | --- | --- | --- |
| Off | 0.125 ms | – | – | – | – |
| Low | 0.177 ms | 0.014 ms | 0.016 ms | 31 | 49 |
| Medium | 0.213 ms | 0.030 ms | 0.032 ms | 82 | 215 |
| High | 0.342 ms | 0.037 ms | 0.065 ms | 82 | 215 |
| Ultra | 0.422 ms | 0.045 ms | 0.071 ms | 117 | 371 |

The dungeon at High, which is the heaviest shipping scene: 0.062 ms CPU, 0.079 ms GPU, 252
draws, 151 026 triangles, 393 casters, 6 local views granted and 19 refused.

Two things to read out of that table. First, High roughly doubles the frame's GPU time
against Off — but from 0.145 ms, and most of the increase is the depth prepass and the extra
sampling in the mesh pass rather than the shadow stage itself, which is under a tenth of a
millisecond. Second, the shadow stage's CPU cost is flat in the presets and scales with
casters, which is what the culling and the LOD are for.

The stage allocates nothing per frame: caster lists and batch scratch are owned buffers
reused every frame, and the constants go through the existing upload ring.

## Deliberate limits

* **`MAX_LOCAL_LIGHTS` is 32 per frame.** It was eight, and the lighting pass looped over all
  eight for every pixel; `crates/engine-renderer/src/light_cluster.rs` now assigns each light
  to the clusters of the frustum it reaches and a pixel evaluates only its own cluster's.
  Thirty-two is the cap because a cluster's light set is one `u32` bitmask, which is what
  keeps the whole grid to 13.5 KB and one root SRV. Going past it is a change of
  representation rather than of a constant. See `docs/performance/shadows/01-local-light-culling.md`.
* **`MAX_LOCAL_SHADOW_VIEWS` is 16.** The atlas is laid out for it and the frame constants
  carry sixteen matrices.
* **Cascade count is capped at 4**, which is the four sets of matrices and rectangles in the
  frame constants.
* **No cached/static shadow map.** Every view is redrawn every frame. A scene whose static
  geometry dominates could cache the far cascades and redraw only the movers; nothing measured
  so far justifies the invalidation logic.
* **The atlas debug view lives inside the mesh pixel shader.** It draws only where some
  fragment covers the corner. That is a real constraint, taken knowingly: the alternative is a
  second pipeline, root signature and draw for a developer tool.
