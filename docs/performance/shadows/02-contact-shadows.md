# Milestones 2 and 3 — half-resolution contact shadows, and the march

Two milestones in one document because they are one change measured twice: first the march
moved out of the mesh shader into a half-resolution pass of its own, then the march itself was
made shorter.

## What changed

### Milestone 2 — a pass instead of a function

`contact_shadow` was called from `shade_surface`, so every shaded pixel marched twelve samples
through the depth buffer. At 4K that was **1.31 ms of a 2.50 ms frame** — the largest single
item in it.

It is now `shaders/contact.hlsl`: a fullscreen triangle drawn into a target of its own at
`ContactShadowSettings::divisor` on each axis. Half by default, so a quarter of the rays.

```text
depth prepass ──► CopyResource ──► scene_depth (full resolution)
                                        │
                                        ▼
                       contact pass, 1/2 width and height
                                        │
                            R16G16_FLOAT: r = shadow factor
                                          g = view distance
                                        │
                                        ▼
                      mesh pass, depth-weighted reconstruction
```

Two details carry the quality:

* **The green channel.** Each texel records the view distance its ray started from. The
  lighting pass reads it, compares it against the depth of the pixel being shaded, and weights
  each of the four neighbours by how well they agree. A plain bilinear upsample smears a
  foot's contact shadow onto the wall behind it; this does not. Where every neighbour
  disagrees — a one-pixel silhouette — the answer is "unoccluded" rather than a guess, because
  contact shadows only ever darken and failing towards 1 fails towards the picture without
  them.
* **The normal comes from the depth buffer.** This is a forward renderer with no G-buffer, so
  the pass reconstructs the surface normal from screen-space derivatives of the reconstructed
  world position, flipping it if it comes out facing away from the camera at a silhouette. The
  normal is used only to lift the ray's origin half a step off the surface, so a wrong normal
  along a silhouette costs a texel of a term that is being reconstructed with a depth-aware
  filter anyway.

The gating did **not** move: `receives_shadow` and the sun-facing test are still applied in the
mesh shader, so a surface that does not receive shadows is still not darkened.

### Milestone 3 — a shorter march

`12 steps / 25 m` became `8 steps / 20 m` at Ultra and `8 steps / 15 m` at High.

## Files changed

| File | What |
| --- | --- |
| `crates/engine-renderer/shaders/contact.hlsl` | new — the pass: fullscreen triangle, depth reconstruction, the march |
| `crates/engine-renderer/src/shadow/contact.rs` | new — the target, its views, and the pipeline state |
| `crates/engine-renderer/shaders/scene_frame.hlsli` | `contact_shadow_at` (the reconstruction), `t4`, `contact_target` |
| `crates/engine-renderer/src/shadow/settings.rs` | `ContactShadowSettings::divisor`, `target_size`, retuned defaults |
| `crates/engine-renderer/src/scene.rs` | `contact_target`, `set_contact_target`, `ROOT_CONTACT_TABLE` |
| `crates/engine-renderer/src/mesh_pipeline.rs` | root parameter 8, a table at `t4` |
| `crates/engine-renderer/src/shadow/renderer.rs` | owns the target, `record_contact` |
| `crates/engine-renderer/src/renderer.rs` | frame constants built **before** the shadow stage, so the pass can read them |
| `crates/engine-renderer/src/offscreen.rs` | the offscreen path runs the pass too |
| `crates/engine-renderer/tests/contact_shadows.rs` | new — 4 pixel tests |
| `crates/engine-renderer/build.rs` | `CONTACT_VS`, `CONTACT_PS` |

One ordering change worth naming: the frame constants are now built and uploaded **before**
the shadow stage rather than after it, because the contact pass reads them. That is why
`record_shadow_stage` takes a GPU address.

## Method

Identical to Milestones 0 and 1. Every "before" column is the Milestone 1 binary run in the
same interleaved session, so nothing here is a comparison against an older machine state.

## Results — Milestone 2, the resolution

`--shadows ultra`, GPU p50, three interleaved repetitions.

| Resolution | Configuration | GPU p50 | Contact pass GPU p50 | FPS |
| --- | --- | ---: | ---: | ---: |
| 1920×1080 | Milestone 1, 5×5 + contact (inline, full res) | 0.605 | – | 1577 |
| 1920×1080 | Milestone 2, 5×5 + contact **half** | **0.483** | 0.124 | 1942 |
| 1920×1080 | Milestone 2, 5×5 + contact full | 0.606 | 0.229 | 1555 |
| 1920×1080 | Milestone 2, 5×5 + contact quarter | 0.472 | 0.118 | 1976 |
| 1920×1080 | Milestone 1, 3×3 + contact | 0.549 | – | 1727 |
| 1920×1080 | Milestone 2, 3×3 + contact **half** | **0.397** | 0.123 | 2283 |
| 2560×1440 | Milestone 1, 5×5 + contact | 1.026 | – | 928 |
| 2560×1440 | Milestone 2, 5×5 + contact **half** | **0.790** | 0.184 | 1189 |
| 2560×1440 | Milestone 2, 5×5 + contact full | 1.017 | 0.375 | 928 |
| 2560×1440 | Milestone 1, 3×3 + contact | 0.927 | – | 1016 |
| 2560×1440 | Milestone 2, 3×3 + contact **half** | **0.645** | 0.183 | 1488 |
| 3840×2160 | Milestone 1, 5×5 + contact | 2.733 | – | 364 |
| 3840×2160 | Milestone 2, 5×5 + contact **half** | **1.638** | 0.356 | 589 |
| 3840×2160 | Milestone 2, 5×5 + contact full | 2.714 | 0.781 | 368 |
| 3840×2160 | Milestone 2, 5×5 + contact quarter | 1.594 | 0.362 | 595 |
| 3840×2160 | Milestone 1, 3×3 + contact | 1.951 | – | 407 |
| 3840×2160 | Milestone 2, 3×3 + contact **half** | **1.335** | 0.352 | 740 |

"Contact pass GPU p50" is the shadow stage's timer, which now brackets the atlas *and* the
contact pass; the atlas alone is 0.030 ms, so almost all of it is the pass.

Read out of that:

* **A full-resolution pass is worth nothing.** 2.714 ms against 2.733 inline at 4K. The
  saving is entirely the resolution, not the restructuring — which is worth knowing, because
  the restructuring is what made the resolution changeable.
* **Half resolution is the whole win**: −40 % of the frame at 4K, −23 % at 1440p, −20 % at
  1080p.
* **Quarter resolution buys almost nothing more** — 1.594 against 1.638 at 4K, 3 % — and it is
  a visibly coarser term. Half is where the curve bends.

## Results — Milestone 3, the march

At 3840×2160, on top of the half-resolution pass:

| Configuration | GPU p50 | Contact pass GPU p50 | Frame vs 12/25 m |
| --- | ---: | ---: | ---: |
| 12 steps, 25 m (Milestone 2 default) | 1.634 | 0.356 | – |
| **8 steps, 20 m** | **1.578** | 0.303 | −0.056 |
| 8 steps, 15 m | 1.557 | 0.295 | −0.077 |
| 6 steps, 15 m | 1.538 | 0.278 | −0.096 |
| 6 steps, 12 m | 1.524 | 0.278 | −0.110 |
| 4 steps, 12 m | 1.508 | 0.257 | −0.126 |
| contact off | 1.201 | 0.030 | −0.433 |

Once the pass is at half resolution the step count barely matters: twelve steps to four is
0.126 ms of a 1.6 ms frame. The pass has become bound by its own setup and bandwidth rather
than by how often it samples.

### Quality of each step count

Measured rather than guessed. The same 256×256 test frame, shadow-factor view, coarse cascade,
counting pixels the contact term darkens by more than 0.05 and the mean absolute difference
against a 32-step full-resolution reference:

| | 0.35 m ray (shipping) | 0.6 m ray | 1.2 m ray |
| --- | --- | --- | --- |
| 12 steps, full res | 105 px, 0.00088 | 177 px, 0.00158 | 367 px, 0.00324 |
| 12 steps, half | 76 px, 0.00042 | 175 px, 0.00104 | 460 px, 0.00284 |
| **8 steps, half** | **76 px, 0.00040** | 146 px, 0.00088 | 238 px, 0.00134 |
| 6 steps, half | 65 px, 0.00038 | 155 px, 0.00095 | 277 px, 0.00170 |
| 4 steps, half | 57 px, 0.00032 | 98 px, 0.00054 | 181 px, 0.00108 |

**At the shipping ray length, twelve steps and eight are the same picture** — 76 pixels
darkened either way, a mean difference of 5 %. Six loses 14 % of the darkened pixels and four
loses 25 %. The step count only starts to matter once the ray is long enough that the samples
are far apart: at 1.2 m, twelve steps is a sample every 10 cm and eight is a sample every
15 cm, and thin occluders start to fall between them.

That is why **eight** is the choice rather than the fastest one. Four steps would have bought
another 0.07 ms at 4K and given up a quarter of the effect.

The distance was cut to 20 m rather than 15 or 12 because the range is a *gameplay* number
rather than a performance one: a third-person camera sits 4–6 m behind the character and a
fight spreads ten metres in front of that, so 20 m covers the fight with the fade to spare and
15 m starts to switch the effect off at the far edge of one. High takes 15 m, which is the
right trade for a preset that is already spending less everywhere else.

## The chosen configuration

| Preset | Contact shadows |
| --- | --- |
| Low, Medium | off |
| High | on, 8 steps, 0.35 m ray, 15 m range, **half resolution** |
| Ultra | on, 8 steps, 0.35 m ray, 20 m range, **half resolution** |

## Results — the milestone as a whole

Milestone 1 binary against Milestone 3, `--shadows ultra`, same session:

| Resolution | Before (GPU p50) | After (GPU p50) | Difference | Improvement | FPS before → after |
| --- | ---: | ---: | ---: | ---: | --- |
| 1280×720 | 0.309 | 0.249 | −0.060 | 19 % | 2857 → 3356 |
| 1920×1080 | 0.606 | 0.470 | −0.136 | 22 % | 1565 → 1992 |
| 2560×1440 | 1.025 | 0.764 | −0.261 | 25 % | 923 → 1221 |
| 3840×2160 | 2.735 | 1.579 | **−1.156** | **42 %** | 364 → 591 |

And what contact shadowing itself now costs, against the same frame with `contact=off`:

| Resolution | Contact cost, Milestone 0 | Contact cost now | Improvement |
| --- | ---: | ---: | ---: |
| 1920×1080 | 0.237 | 0.096 | 59 % |
| 2560×1440 | 0.437 | 0.171 | 61 % |
| 3840×2160 | 1.307 | 0.377 | **71 %** |

Contact shadows were 52 % of the 4K frame at Milestone 0. They are 24 % of it now, and the
frame they are 24 % of is 42 % smaller.

## Visual quality

`tests/contact_shadows.rs`, all through the shipping pass:

* **the seam is still there** — the contact term darkens more than 300 pixels of a 256×256
  frame where three crates meet the floor;
* **nothing is ever brightened** — zero pixels of the frame are lighter with contact shadows
  than without, which is the failure a bilinear upsample produces and the depth weighting
  exists to prevent;
* **half looks like full** — the mean difference in the shadow factor between the
  half-resolution and full-resolution passes is under 0.02, while the half-resolution pass
  still darkens the same seam;
* **the divisor means what it says** — 3840×2160 → 1920×1080, and odd sizes round up so the
  last column has somewhere to be reconstructed from.

Inspected against the shipping settings on the arena and the shadow course: character and
enemy feet keep their grounding, crates keep their seam against the floor, and the transition
where the effect fades out at its range limit is a gradient rather than a circle. The places a
half-resolution screen-space term fails — a thin silhouette against a distant background —
fail towards *no* contact shadow rather than towards a smeared one.

One honest note on magnitude: at the shipping 2048-texel cascade the contact term is a small
effect in absolute terms, because the cascade already resolves most of what it exists for. The
tests use a 256-texel cascade and a 2 m ray to make it large enough to assert on, and say so.

## Regressions

1. **The shadow stage's GPU timer now includes the contact pass**, so the "shadow stage GPU"
   column jumps from 0.03 ms to 0.30 ms at 4K. That is a change of what is measured, not of
   what is spent: the same work used to be inside the mesh pass.
2. **One more render target**: `R16G16_FLOAT` at half the camera's size, 4 MB at 4K, and one
   more root parameter. It collapses to 1×1 when contact shadows are off.
3. The frame constants are built before the shadow stage now. That is a reordering inside one
   function, but it means anything added to the constants later has to be available at that
   point.

## Conclusion

**Keep both, enabled.** Half resolution is the single largest win in the whole sequence —
1.16 ms of a 2.7 ms frame at 4K — and the depth-weighted reconstruction keeps the term
looking like the full-resolution one. The shorter march is a smaller, free improvement that
the quality measurement says costs nothing at the shipping ray length.

## Reproducing

```powershell
# the resolution experiment
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Resolutions "1920x1080,2560x1440,3840x2160" `
  -Configs "M1 5x5+contact|ultra||target/release/mygame-head.exe|;M2 5x5 half|ultra||;M2 5x5 full|ultra|contact-divisor=1|;M2 5x5 quarter|ultra|contact-divisor=4|"

# the march experiment
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Resolutions "3840x2160" `
  -Configs "12 steps 25 m|ultra|contact-steps=12,contact-distance=25|;8 steps 20 m|ultra||;8 steps 15 m|ultra|contact-steps=8,contact-distance=15|;6 steps 15 m|ultra|contact-steps=6,contact-distance=15|;no contact|ultra|contact=off|"
```
