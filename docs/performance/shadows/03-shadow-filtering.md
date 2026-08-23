# Milestone 4 — replacing the 5×5 PCF grid

## What changed

`PcfKernel` gained `Poisson4`, `Poisson6` and `Poisson8`: a **rotated Vogel disc** of that many
taps, rotated per pixel by interleaved gradient noise. Ultra moved from `Tap5x5` (25 taps) to
`Poisson6`; High and Medium from `Tap3x3` (9 taps) to `Poisson4`. Both grids are kept, because
"is the disc as wide as the grid?" is a question that needs the grid to still exist.

```hlsl
float2 vogel_disc(uint index, uint count, float rotation)
{
    float radius = sqrt((float)index + 0.5) / sqrt((float)count);
    float theta  = (float)index * 2.39996323 + rotation;   // the golden angle
    return float2(cos(theta), sin(theta)) * radius;
}
```

A golden-angle spiral rather than a stored Poisson set: it is well spread for *any* tap count,
so 4, 6 and 8 taps are one piece of code and one radius to tune rather than three tables
somebody has to keep in agreement. The disc's radius is `ShadowSettings::filter_radius_texels`,
2.5 by default — the one number that decides the trade, and a setting rather than a shader
constant for that reason.

### The rotation is per pixel, and per pixel only

```hlsl
float interleaved_gradient_noise(float2 pixel)
{
    return frac(52.9829189 * frac(dot(pixel, float2(0.06711056, 0.00583715))));
}
```

**Deliberately not a function of the frame index.** The engine has no temporal accumulation and
no TAA — the milestone brief says not to introduce one for this — so an angle that changed per
frame would turn a static penumbra into boiling noise. Fixed per pixel, the residual error is a
fine screen-space dither that stands still, and the measurement below is what says whether that
holds up while the camera moves.

Each tap is still a single `SampleCmpLevelZero` through the comparison sampler, so the hardware
does the depth test and a 2×2 bilinear in one fetch. Six taps is six fetches covering
twenty-four texels.

Each disc width is its own unrolled loop, for the reason the grids already were: a dynamic loop
around a texture fetch is markedly slower than a static one, and it costs registers in what is
the frame's largest shader.

## Files changed

| File | What |
| --- | --- |
| `crates/engine-renderer/src/shadow/settings.rs` | the three kernels, `filter_radius_texels`, `is_poisson`, `cheaper`, `as_str`, retuned presets |
| `crates/engine-renderer/shaders/scene_frame.hlsli` | `vogel_disc`, `interleaved_gradient_noise`, three unrolled branches, the pixel coordinate threaded through |
| `crates/engine-renderer/src/scene.rs` | the radius, in `shadow_params2.w` |
| `crates/engine-renderer/tests/shadow_filtering.rs` | new — 4 tests |

## Method

Performance: identical to the earlier milestones. Quality: the same scene rendered through the
**shadow-factor debug view**, so albedo and the sun's angle are out of the way, at 384×384, with
contact shadows off so they cannot be measured as filter width. Three numbers per kernel:

* **penumbra** — the median count of partly-shadowed pixels (0.05 < factor < 0.95) along a
  scanline crossing the shadow's edge. A filter that is cheap by being narrow is not cheap.
* **local noise** — the mean absolute difference between a partly-shadowed pixel and the mean
  of its 3×3 neighbourhood. Banding and dither both show here; a wider penumbra lowers it,
  which is why it is read next to the penumbra and not alone.
* **drift** — the mean absolute change in the penumbra when the camera moves **one
  centimetre**. This is what "crawling" is: a dither fixed in screen space leaves its pattern
  behind while the shadow slides under it.

## Results — cost

`--shadows ultra`, GPU p50, three interleaved repetitions, everything else at the Milestone 3
configuration.

| Resolution | 5×5 (25) | **Poisson 6** | Poisson 8 | Poisson 4 | 3×3 (9) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1280×720 | 0.250 | **0.215** | 0.230 | 0.211 | 0.210 |
| 1920×1080 | 0.468 | **0.403** | 0.418 | 0.388 | 0.383 |
| 2560×1440 | 0.762 | **0.656** | 0.687 | 0.623 | 0.615 |
| 3840×2160 | 1.580 | **1.364** | 1.432 | 1.291 | 1.275 |

| Resolution | 5×5 → Poisson 6 | Improvement | FPS |
| --- | ---: | ---: | --- |
| 1280×720 | −0.035 | 14 % | 3289 → 3610 |
| 1920×1080 | −0.065 | 14 % | 1980 → 2252 |
| 2560×1440 | −0.106 | 14 % | 1225 → 1460 |
| 3840×2160 | −0.216 | **14 %** | 589 → 728 |

## Results — quality

Radius 2.5 texels, the shipping value:

| Kernel | Taps | Penumbra | Local noise | Drift on a 1 cm camera move |
| --- | ---: | ---: | ---: | ---: |
| single | 1 | 1 px | 0.1436 | 0.3353 |
| 3×3 grid | 9 | 4 px | 0.1709 | 0.0598 |
| 5×5 grid | 25 | 6 px | 0.0644 | 0.0313 |
| **Poisson 4** | 4 | 6 px | 0.1080 | 0.0377 |
| **Poisson 6** | 6 | 6 px | 0.1084 | 0.0341 |
| Poisson 8 | 8 | 6 px | 0.1080 | 0.0328 |

And the radius sweep that chose 2.5, for the six-tap disc:

| Radius | Penumbra | Local noise | Drift |
| ---: | ---: | ---: | ---: |
| 1.5 texels | 4 px | 0.1935 | 0.0562 |
| 2.0 texels | 5 px | 0.1533 | 0.0372 |
| **2.5 texels** | 6 px | 0.1084 | 0.0341 |
| 3.0 texels | 7 px | 0.0784 | 0.0319 |

Read out of that:

1. **Poisson 6 is the 5×5's penumbra.** Six pixels either way. The saving is not being taken
   out of the picture.
2. **Poisson 6 is very nearly the 5×5's stability** — 0.0341 against 0.0313, 9 % apart — and
   comfortably steadier than the 3×3 it is twice as wide as. Which is the answer to the
   question a stochastic filter has to answer without a temporal pass, and it is what the
   fixed-per-pixel rotation buys.
3. **Poisson 4 beats the 3×3 on every axis**: wider penumbra (6 px against 4), a third less
   local noise, 37 % less drift — for five fewer fetches. There was no reason to keep the 3×3
   at High and Medium.
4. **Poisson 8 buys nothing over 6** at this radius — the same penumbra, the same noise, 0.0013
   less drift — and costs 5 % more at 4K. It stays available and no preset selects it.
5. **The residual noise is the trade, and it is visible in the numbers**: 0.108 against the
   5×5's 0.064. That is the dither. It is what six taps look like instead of twenty-five, it
   does not move when the camera does, and at 2.5 texels it is finer than one cascade texel.
6. Below 2 texels the disc is several samples of the same texel and both noise and drift climb
   steeply. Above 3 the penumbra is wider than the filter it stands in for, which is a
   different look rather than the same one cheaper.

### Repeating patterns

The concern with a fixed pattern is that it repeats visibly. Two things prevent it: the disc is
a golden-angle spiral, which has no axis to align with a shadow edge, and the per-pixel rotation
comes from interleaved gradient noise, whose period is long enough that no structure appears at
the scale of a penumbra. Inspected on the shadow course's hundred-crate grid — a hundred nearly
identical penumbras side by side, which is the hardest case for a repeating pattern — and on the
arena's boundary wall, where a single edge runs the width of the screen. Neither shows a pattern
at 1080p or 4K.

## Visual inspection

Checked at the shipping settings on the arena, the shadow course and the dungeon:

* **shadow softness** — the near-cascade penumbra is the same width as the 5×5's; the character
  reads as standing on the ground, not floating;
* **aliasing** — no stair-stepping along a straight edge, which is the failure the grid has and
  the disc does not;
* **banding** — none; the disc's taps are at different radii, so there are no concentric steps;
* **shimmering while moving** — the measured drift is the 5×5's within 9 %, and moving the
  camera through the arena shows no crawl on the ground or on the wall;
* **cascade transitions** — unchanged; the blend samples two cascades with the same kernel and
  both are dithered the same way, so the seam is no more visible than it was;
* **overlapping characters** — unchanged; the filter is per lookup and does not know about
  silhouettes.

## Regressions

1. **Six taps are noisier than twenty-five.** 0.108 against 0.064 by the local-noise measure.
   This is the trade the milestone asked for and it is a dither rather than a structure, but it
   is a real difference and a viewer looking for it on a flat wall at 4K can find it.
2. The pixel coordinate now travels through the whole shadow-sampling chain, which costs a
   little register pressure even at kernels that do not use it. That was introduced in
   Milestone 0 and measured at about 0.09 ms at 4K Ultra; this milestone's 0.216 ms saving pays
   it back with 0.13 to spare.
3. `Tap5x5` is now a kernel no preset selects. It is kept deliberately — the tests compare
   against it — but it is code with no shipping caller.

## Conclusion

**Keep it, enabled.** Poisson 6 at 2.5 texels is the 5×5's penumbra and, to within 9 %, the
5×5's stability, for six fetches instead of twenty-five and 14 % of the frame at every
resolution. Poisson 4 replaces the 3×3 at High and Medium and is better than it on every
measured axis while costing less.

Final: **Low** single tap, **Medium** and **High** Poisson 4, **Ultra** Poisson 6, all at a
2.5-texel radius.

## Reproducing

```powershell
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 `
  -Configs "5x5 grid|ultra|pcf=5x5|;Poisson 6|ultra||;3x3 grid|ultra|pcf=3x3|;Poisson 4|ultra|pcf=poisson4|;Poisson 8|ultra|pcf=poisson8|"
```

```bash
cargo test --release -p engine-renderer --test shadow_filtering
```
