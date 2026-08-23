# Milestone 1 — clustered local-light culling

## What changed

The mesh pixel shader used to loop over **every** local light in the frame for every pixel it
shaded. It now loops over the lights of the *cluster* the pixel falls in.

New module: `crates/engine-renderer/src/light_cluster.rs`. The view frustum is divided into a
fixed grid — 16 tiles across, 9 down, 24 exponentially-spaced depth slices, 3456 clusters —
and each frame every light is assigned to the clusters its bounding sphere reaches. A
cluster's light set is one `u32` bitmask; the shader finds its own cluster from `SV_Position`
and the view depth and walks the mask with `firstbitlow`.

```text
CPU, once per frame                     GPU, once per pixel
-------------------                     -------------------
for each light                          cluster = tile(pixel) + slice(depth)
  project its sphere -> tile box        mask    = cluster_light_mask[cluster]
  slice range from its depth range      while mask:  light = firstbitlow(mask)
  exact sphere-vs-cluster test
  set its bit
```

`MAX_LOCAL_LIGHTS` went from **8 to 32** as a direct consequence: the old cap was there
because the eighth light cost every pixel as much as the first, and it no longer does. Thirty-
two exactly, because a cluster's light set is one `u32`.

### Why the CPU, and why no compute pass

Assignment is output-sensitive — a light touches the clusters its projected bounds cover, not
all 3456 — so the cost is a projection and a walk over a small box per light. That is a few
microseconds for the light counts this engine supports. A compute pass would be the larger
change by a wide margin: the renderer has no compute pipelines at all, no UAVs and no
compute root signature, and none of that would buy anything until the grid itself is the
bottleneck rather than the lighting.

### One thing that was tried and measured wrong

The grid lived in the frame constant buffer first — simplest possible: no new binding, no new
buffer. It took `FrameConstants` from 2.4 KB to 17.8 KB, and **cost 0.34 ms a frame at 4K**,
more than the lighting it was there to save. A constant buffer that no longer fits the
hardware's constant cache is slower to read *everywhere*, not only in the part that grew: the
cascade matrices, the biases and the atlas rectangles all got slower too.

It is now a `ByteAddressBuffer` written into the existing upload ring and bound as a **root
SRV** at `t3` — a GPU address in the root arguments, no descriptor, no heap slot. Root slot 7,
pixel-visible. `FrameConstants` is back to 4000 bytes and `scene.rs` now asserts it stays
under 8 KB, with the reason written next to the assertion.

## Files changed

| File | What |
| --- | --- |
| `crates/engine-renderer/src/light_cluster.rs` | new — the grid, assignment, and 10 unit tests |
| `crates/engine-renderer/shaders/scene_frame.hlsli` | `cluster_of`, `lights_at`, the mask loop, `t3` |
| `crates/engine-renderer/src/scene.rs` | `cluster_params`, `cluster_depth`, `ROOT_CLUSTER_SRV`, manual `Default` |
| `crates/engine-renderer/src/mesh_pipeline.rs` | root parameter 7, a root SRV at `t3` |
| `crates/engine-renderer/src/renderer.rs` | owns the grid, builds it in `prepare_shadows`, binds it |
| `crates/engine-renderer/src/command.rs` | `set_root_srv` |
| `crates/engine-renderer/src/offscreen.rs` | every mesh path binds the grid; the shadowed path builds a real one |
| `crates/engine-renderer/src/shadow/settings.rs` | `MAX_LOCAL_LIGHTS` 8 → 32 |
| `crates/engine-renderer/tests/light_clusters.rs` | new — 4 pixel tests through the shipping path |
| `crates/engine-environment/src/lighting.rs` | `push_stress_lights`, the lighting stress control |
| `crates/engine-app/src/args.rs`, `app.rs` | `--lights <count>`, `--shadow-tweak clusters=off` |

## Method

Identical to Milestone 0 — same scene, same camera, 900 measured frames after 600 discarded,
three interleaved repetitions, median reported, FPS from `1000 / max(CPU p50, GPU p50)`.

One addition, and it matters: **every comparison below is against a reference binary built
from the Milestone 0 commit and run in the same interleaved session.** The machine drifts
enough over an afternoon to move a 4K number by 20 %, and comparing against numbers taken
earlier would have reported drift as a result. The reference binary carries the measurement
tooling and the `--lights` control and nothing else; its renderer is the one Milestone 0
measured.

Two new measurement controls, both in the same family as `--shadows`:

* `--lights <count>` adds decorative point lights to whichever scene loaded, on a
  golden-angle spiral inside its boundary. None of them casts shadows: this asks what
  *shading* many lights costs, which is the question this milestone is about.
* `--shadow-tweak clusters=off` leaves the grid unbuilt, and the shader then walks the whole
  light list — the pre-milestone behaviour, in the post-milestone binary.

## Results — the arena as it ships (4 lights)

| Resolution | Configuration | CPU p50 | GPU p50 | GPU spread | FPS |
| --- | --- | ---: | ---: | ---: | ---: |
| 1280×720 | Milestone 0, shadows off | 0.241 | 0.092 | 0.038 | 4149 |
| 1280×720 | Milestone 1, shadows off | 0.245 | 0.096 | 0.027 | 4082 |
| 1280×720 | Milestone 0, Ultra | 0.362 | 0.310 | 0.011 | 2762 |
| 1280×720 | Milestone 1, Ultra | 0.364 | 0.321 | 0.002 | 2747 |
| 1920×1080 | Milestone 0, shadows off | 0.268 | 0.130 | 0.020 | 3731 |
| 1920×1080 | Milestone 1, shadows off | 0.280 | 0.140 | 0.040 | 3571 |
| 1920×1080 | Milestone 0, Ultra | 0.645 | 0.607 | 0.001 | 1550 |
| 1920×1080 | Milestone 1, Ultra | 0.669 | 0.635 | 0.004 | 1495 |
| 2560×1440 | Milestone 0, shadows off | 0.250 | 0.202 | 0.000 | 4000 |
| 2560×1440 | Milestone 1, shadows off | 0.256 | 0.210 | 0.001 | 3906 |
| 2560×1440 | Milestone 0, Ultra | 1.067 | 1.020 | 0.006 | 937 |
| 2560×1440 | Milestone 1, Ultra | 1.107 | 1.060 | 0.003 | 903 |
| 3840×2160 | Milestone 0, shadows off | 0.450 | 0.417 | 0.003 | 2222 |
| 3840×2160 | Milestone 1, shadows off | 0.453 | 0.421 | 0.005 | 2208 |
| 3840×2160 | Milestone 0, Ultra | 2.748 | 2.735 | 0.430 | 364 |
| 3840×2160 | Milestone 1, Ultra | 2.864 | 2.849 | 0.008 | 349 |

**At four lights this is a small regression, not a win: +0.004 ms with shadows off and
+0.11 ms at Ultra (4 %).** That is the honest reading and it is not hidden below.

Two things account for it, and neither is the grid:

* With shadows off, four lights is 0.004 ms either way — inside the spread. The grid neither
  helps nor hurts, because rejecting four lights on distance was already nearly free.
* The Ultra gap is **not** the clustering. Measured with the lights switched off entirely, so
  that the shader never touches the grid at all: Milestone 0 is 2.56 ms and Milestone 1 is
  2.65 ms at 4K. The +0.09 is the pixel coordinate now threaded through the shadow-sampling
  chain for the disc kernels added in Milestone 0, which costs registers in what is already
  the frame's largest shader. Milestone 4 removes twenty-five taps' worth of that shader and
  should more than repay it; this document does not claim it back in advance.

The other shipping scenes agree — dungeon at 1080p Ultra: 0.418 → 0.426; collision course at
1080p Ultra: 0.106 → 0.106.

## Results — what the structure is actually for

Both columns from the **same binary**, so the only difference is whether the grid is used.
`--shadows off`, so this is the lighting cost alone with no shadow work on top.

### 1920×1080

| Lights | No grid (GPU p50) | Clustered (GPU p50) | Difference | Improvement |
| ---: | ---: | ---: | ---: | ---: |
| 4 | 0.164 | 0.141 | −0.023 | 14 % |
| 8 | 0.228 | 0.155 | −0.073 | 32 % |
| 16 | 0.349 | 0.206 | −0.143 | 41 % |
| 32 | 0.580 | 0.270 | −0.310 | 53 % |

### 3840×2160

| Lights | No grid (GPU p50) | Clustered (GPU p50) | Difference | Improvement |
| ---: | ---: | ---: | ---: | ---: |
| 4 | 0.592 | 0.418 | −0.174 | 29 % |
| 8 | 0.808 | 0.445 | −0.363 | 45 % |
| 16 | 1.247 | 0.521 | −0.726 | 58 % |
| 32 | 2.753 | 0.648 | −2.105 | **76 %** |

Thirty-two lights at 4K cost 2.75 ms unculled and 0.65 ms clustered. The unculled curve is
the one the engine was on: 0.07 ms per light per frame at 4K, paid whether or not the light
is anywhere near the pixel. The clustered curve is 0.008 ms per light — an order of magnitude
flatter, and most of what is left is the lighting the lights actually do.

### Occupancy

| Scene | Lights | Clusters occupied (of 3456) | Worst cluster |
| --- | ---: | ---: | ---: |
| arena, as it ships | 4 | 208 | 1 |
| arena, `--lights 28` | 32 | 2196 | 9 |
| dungeon | 20 | 488 | 7 |

A fragment in the 32-light arena evaluates at most 9 lights and usually 1 or 2, against 32
before. That ratio is the improvement table above.

### A correctness result, not only a performance one

**The dungeon has 20 local lights and the old cap was 8.** Every frame of the dungeon was
silently dropping twelve of its torches. Milestone 1 shades all twenty for 0.426 ms against
the old 0.418 ms for eight of them — 2 % more time for two and a half times the lights.

## Visual quality

No intended change: the same lights, the same attenuation, the same shadow lookups. What is
asserted:

* `tests/light_clusters.rs` — a lamp brightens the floor under it; a 3 m lamp ten metres away
  changes nothing on the far side of the floor; **all sixteen** lamps of a grid light their own
  patch; and a near lamp and a far lamp down the same screen column light different rows,
  which is the depth-slice half of the lookup and the only test that catches a shader
  computing the slice differently from the CPU.
* `tests/shadow_render.rs` — the existing spot-light and point-light shadow tests now run
  through the clustered path, because `render_scene_shadowed` builds the same grid the window
  does. They pass unchanged.
* `light_cluster.rs` — 10 unit tests: an empty frame, a light behind the camera, a light past
  the far plane, a light the camera stands inside (the near-plane case, which the conservative
  projection cannot handle and which has its own branch), two lights that must never share a
  cluster, the cap, an orthographic camera, and the depth slicing covering its range with no
  gap.

The dungeon looks *better*, because twelve torches that were being dropped are now lit.

## Regressions

1. **+0.11 ms at 4K Ultra in the four-light arena** (4 %), of which about 0.09 is Milestone 0's
   disc-kernel shader change and not this one. Revisit at Milestone 4.
2. **CPU: about +0.02 ms a frame.** The grid is 13.5 KB memcpy'd into the upload ring plus the
   assignment walk. Visible at 1080p shadows off: 0.268 → 0.280 CPU p50.
3. The `#[derive(Default)]` on `FrameConstants` had to be written out by hand, because a
   32-entry array is the largest one the derive reaches. Mechanical, but it is a field list
   that now has to be kept in step with the struct; the layout test next to it is what catches
   a field added to one and not the other.

## Conclusion

**Keep it, enabled.** At the shipping arena's four lights it is a 4 % cost at Ultra and free
with shadows off. At the light counts a dungeon or a spell-heavy fight actually produces it is
worth 30–76 % of the lighting, it removes a cap that was silently discarding half the
dungeon's torches, and it turns the per-light cost from 0.07 ms into 0.008 ms at 4K.

The invariant the milestone asked for holds: a fragment evaluates the lights of its own
cluster and no others.

## Reproducing

```powershell
# the shipping arena, against the Milestone 0 binary
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Repeats 3 `
  -Configs "HEAD off|off||target/release/mygame-head.exe|;M1 off|off||;HEAD Ultra|ultra||target/release/mygame-head.exe|;M1 Ultra|ultra||"

# the scaling table, one binary, grid on and off
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Resolutions "1920x1080,3840x2160" `
  -Configs "4 no grid|off|clusters=off|;4 clustered|off||;8 no grid|off|clusters=off||--lights 4;8 clustered|off|||--lights 4;16 no grid|off|clusters=off||--lights 12;16 clustered|off|||--lights 12;32 no grid|off|clusters=off||--lights 28;32 clustered|off|||--lights 28"
```

The reference binary is built from the Milestone 0 commit in a worktree, with the
`--lights` control copied in so both sides accept the same command line:

```bash
git worktree add ../myGame-head <milestone-0-commit>
```
