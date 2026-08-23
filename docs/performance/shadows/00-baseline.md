# Milestone 0 — baseline

The numbers every later milestone is measured against, and the method that produced them.
Nothing in the rendering path was changed to obtain them.

## What was measured

| | |
| --- | --- |
| git commit | `a16521b` plus the measurement-only changes below |
| build | `cargo build --release --bin mygame`, no validation layer |
| GPU | AMD Radeon RX 6700 XT, 12 GiB, feature level 12_1 |
| CPU/OS | Windows 10 Pro 19045 |
| display | 2560×1440; the 4K rows render into a client area larger than the desktop |
| scene | `--scene combat` — the training arena, 196 objects / 240 render entities |
| camera | wherever the scene puts it; the benchmark sends no input, so every run of a scene sees the same frame |
| frames | 900 measured, after 600 discarded warm-up frames |
| repetitions | 3 per cell, **interleaved**, median reported |
| presentation | windowed, no vsync, no `--fullscreen` |
| UI | on (the game's default) |
| audio | `--no-audio` |

### The measurement-only changes

Three things were added to the existing benchmark path rather than building a second one.
None of them changes what the renderer draws:

* **`--warmup <count>`** — discards the opening frames from the statistics window. Driver
  warm-up, pipeline creation and the first uploads all land in the first few hundred frames
  and none of them happens again; a p95 that includes them describes starting the game.
* **`final shadow-stage GPU:`** — the shadow stage's own timestamps, already resolved every
  frame for the overlay, now accumulated into a window and reported at the end alongside the
  frame's. It answers "did the atlas pass get slower, or the mesh pass?".
* **`--shadow-tweak <k=v,…>`** — overrides one shadow field on top of a preset, in the same
  family as the existing `--shadows` and equally never written back to the settings file. It
  is what makes "what does contact shadowing cost?" one run of one binary rather than a
  rebuild.

Two renderer-side additions are present but **not reachable from any preset yet**, so the
baseline is the behaviour of `a16521b`: `PcfKernel::Poisson4/6/8` (Milestone 4) and
`ContactShadowSettings::divisor`, which defaults to 1 — full resolution, the existing march.

### Frame rate

FPS here is `1000 / max(CPU p50, GPU p50)`, not wall clock. The renderer pipelines three
frames, so the frame is the longer of the two halves; and the wall clock of a windowed run
also contains whatever the desktop compositor did, which is what the p95 column is made of.
Every milestone derives it the same way.

### Repeatability

The GPU spread column is `max − min` of the three repetitions' GPU p50. It is ≤ 0.012 ms
everywhere except two cells, so a GPU difference of 0.03 ms or more is a real difference.
CPU p50 is noisier (±15 %) and CPU p95 much noisier: this is a windowed run on a live
desktop. **GPU p50 is the number to read.** An earlier, non-interleaved run of the same
matrix put 20 % of drift on whichever configuration happened to run last, which is why
repetitions are interleaved.

## Shadow configuration

The default `--shadows ultra`, unmodified:

| | |
| --- | --- |
| cascades | 4, at 15 / 50 / 150 / 320 m |
| cascade resolution | 2048², atlas 4096×5120 (80.0 MiB) |
| filter | 5×5 PCF, 25 taps |
| local shadow views | 16 budgeted, 12 granted, 0 refused |
| contact shadows | on, 12 steps, 0.35 m, 0.06 m thickness, 25 m range, full resolution |
| lights in the scene | 1 directional + 4 point (braziers), 2 of them shadow-casting |
| casters | 767 across all views, 0 dropped by LOD |
| shadow draws | 491, 12 924 triangles |

## Results

`shadows off` is `--shadows off`. `no local lights` switches off the four brazier lights'
*shading* only — the scene, the objects and the shadow atlas are untouched.

| Resolution | Configuration | CPU p50 | CPU p95 | GPU p50 | GPU p95 | GPU spread | Shadow stage GPU p50 | FPS |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1280×720 | shadows off | 0.206 | 2.462 | 0.065 | 0.383 | 0.001 | – | 4854 |
| 1280×720 | shadows off, no local lights | 0.219 | 1.688 | 0.052 | 0.373 | 0.000 | – | 4566 |
| 1280×720 | **Ultra (default)** | 0.405 | 7.084 | **0.304** | 3.562 | 0.007 | 0.047 | 2469 |
| 1280×720 | Ultra, no contact | 0.311 | 0.715 | 0.192 | 0.339 | 0.009 | 0.028 | 3215 |
| 1280×720 | Ultra, 3×3 PCF | 0.367 | 7.012 | 0.275 | 2.360 | 0.012 | 0.046 | 2725 |
| 1280×720 | Ultra, 3×3, no contact | 0.315 | 5.110 | 0.144 | 1.148 | 0.005 | 0.027 | 3175 |
| 1280×720 | Ultra, no local lights | 0.398 | 8.976 | 0.283 | 3.701 | 0.008 | 0.047 | 2513 |
| 1920×1080 | shadows off | 0.311 | 0.851 | 0.121 | 0.240 | 0.056 | – | 3215 |
| 1920×1080 | shadows off, no local lights | 0.307 | 1.049 | 0.094 | 0.225 | 0.074 | – | 3257 |
| 1920×1080 | **Ultra (default)** | 0.663 | 2.101 | **0.594** | 0.780 | 0.010 | 0.066 | 1508 |
| 1920×1080 | Ultra, no contact | 0.403 | 0.781 | 0.357 | 0.527 | 0.011 | 0.030 | 2481 |
| 1920×1080 | Ultra, 3×3 PCF | 0.585 | 0.997 | 0.535 | 0.704 | 0.010 | 0.065 | 1709 |
| 1920×1080 | Ultra, 3×3, no contact | 0.301 | 0.650 | 0.252 | 0.416 | 0.007 | 0.030 | 3322 |
| 1920×1080 | Ultra, no local lights | 0.605 | 1.022 | 0.557 | 0.739 | 0.005 | 0.065 | 1653 |
| 2560×1440 | shadows off | 0.245 | 0.642 | 0.200 | 0.393 | 0.004 | – | 4082 |
| 2560×1440 | shadows off, no local lights | 0.195 | 0.403 | 0.150 | 0.155 | 0.004 | – | 5128 |
| 2560×1440 | **Ultra (default)** | 1.073 | 1.573 | **1.015** | 1.245 | 0.003 | 0.093 | 932 |
| 2560×1440 | Ultra, no contact | 0.622 | 1.097 | 0.578 | 0.794 | 0.001 | 0.027 | 1608 |
| 2560×1440 | Ultra, 3×3 PCF | 0.991 | 1.491 | 0.921 | 1.144 | 0.002 | 0.089 | 1009 |
| 2560×1440 | Ultra, 3×3, no contact | 0.445 | 0.886 | 0.400 | 0.612 | 0.002 | 0.027 | 2247 |
| 2560×1440 | Ultra, no local lights | 1.006 | 1.496 | 0.950 | 1.167 | 0.002 | 0.093 | 994 |
| 3840×2160 | shadows off | 0.475 | 1.194 | 0.427 | 0.793 | 0.003 | – | 2105 |
| 3840×2160 | shadows off, no local lights | 0.363 | 1.055 | 0.318 | 0.680 | 0.003 | – | 2755 |
| 3840×2160 | **Ultra (default)** | 2.515 | 3.073 | **2.498** | 2.555 | 0.002 | 0.167 | 398 |
| 3840×2160 | Ultra, no contact | 1.308 | 1.997 | 1.191 | 1.564 | 0.002 | 0.029 | 765 |
| 3840×2160 | Ultra, 3×3 PCF | 2.241 | 2.773 | 1.964 | 2.334 | 0.010 | 0.166 | 446 |
| 3840×2160 | Ultra, 3×3, no contact | 0.882 | 1.672 | 0.832 | 1.209 | 0.001 | 0.029 | 1134 |
| 3840×2160 | Ultra, no local lights | 2.356 | 2.859 | 2.025 | 2.402 | 0.330 | 0.167 | 424 |

## Where the frame goes

Differences of GPU p50, in milliseconds. Each row is one configuration subtracted from
another in the table above.

| Cost | 720p | 1080p | 1440p | 4K | scales with |
| --- | ---: | ---: | ---: | ---: | --- |
| scene with no shadows and no local lights | 0.052 | 0.094 | 0.150 | 0.318 | pixels |
| four brazier lights, shadows off | 0.013 | 0.027 | 0.050 | 0.109 | pixels |
| four brazier lights, shadows on | 0.021 | 0.037 | 0.065 | 0.473 † | pixels |
| directional shadowing, 3×3, no contact | 0.079 | 0.131 | 0.200 | 0.405 | pixels |
| going from 3×3 to 5×5 (no contact) | 0.048 | 0.105 | 0.178 | 0.359 | pixels |
| contact shadows at 5×5 | 0.112 | 0.237 | 0.437 | 1.307 | pixels |
| contact shadows at 3×3 | 0.131 | 0.283 | 0.521 | 1.132 | pixels |
| shadow atlas stage (draws, not shading) | 0.047 | 0.066 | 0.093 | 0.167 | casters, weakly |

Read out of that:

1. **Contact shadows are the single largest item in the frame** — 1.31 ms of a 2.50 ms
   frame at 4K, 52 %. They are a full-resolution screen-space march and they scale exactly
   with pixel count: ×4.1 from 1080p to 4K against a theoretical ×4.
2. **The 5×5 filter is the second** — 0.36 ms at 4K, and it buys a penumbra barely wider
   than the 3×3's.
3. **The four brazier lights cost 0.11 ms at 4K with shadows off**, a quarter of that whole
   frame, and more with shadows on because a lit pixel then also takes a 25-tap atlas lookup
   per shadow-casting light in range. († The 4K "shadows on" figure, 0.47 ms, comes from the
   one cell whose repetitions disagreed by 0.33 ms and should be read as "somewhere between
   0.15 and 0.5"; the shadows-off column is solid.) The loop has no culling structure in
   front of it at all, so this is the item that does not scale: the arena has four lights, a
   dungeon room with twelve would pay three times this for the same pixels, and
   `MAX_LOCAL_LIGHTS` is 8 precisely because the eighth light is as expensive as the first.
4. **The shadow stage itself is not the problem.** Filling the atlas is 0.167 ms at 4K, 7 %
   of the frame, and it is nearly flat in resolution because it draws into a fixed-size
   atlas. Every optimisation worth making is in the *mesh* pass.
5. Ultra costs 5.9× the shadows-off frame at 4K (2.498 vs 0.427) and 4.7× at 720p.

The 4K "Ultra, no local lights" cell has a 0.330 ms spread across its three repetitions —
the only cell where the median should be treated as approximate. The other twenty-seven are
within 0.012 ms.

## Conclusion

The regression is where the profile said it was, and in the proportions the milestone list
assumes: contact shadows first, filtering second, unculled local lights third. The atlas
pass and the caster culling are already cheap and are not worth touching.

Baseline stands. Nothing to enable or disable.

## Reproducing

```powershell
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 `
  -Title "Milestone 0 baseline (combat arena)" `
  -Out docs/performance/shadows/data/00-baseline.json `
  -Configs "shadows off|off|;shadows off, no local lights|off|local-lights=off;Ultra (default)|ultra|;Ultra, no contact|ultra|contact=off;Ultra, 3x3 PCF|ultra|pcf=3x3;Ultra, 3x3, no contact|ultra|pcf=3x3,contact=off;Ultra, no local lights|ultra|local-lights=off"
```
