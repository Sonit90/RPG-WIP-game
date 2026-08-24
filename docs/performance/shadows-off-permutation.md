# A shadows-off shader permutation, and where the 2400-FPS frame actually goes

Two questions, one measurement session:

1. The player runs with shadows off. Does the renderer still pay for shadow code that is
   compiled into the mesh pixel shader but never executed — enough to justify a second,
   shadowless pipeline permutation?
2. `test-branch-1208` renders the same scene at ~3200 FPS; `phase-5` HEAD renders it at
   ~2400 FPS. Where did the difference go?

**Answers.** (1) Yes, but only 0.023–0.026 ms — under the 0.03 ms bar agreed before the
experiment, so no permutation was built and no renderer code was changed. (2) All of it is
in the mesh pixel shader. Everything outside that shader — passes, clears, barriers, the
shadow stage itself, UI, uploads, culling — costs the same 0.080 ms in both builds, to
0.001 ms. The frame got slower because the surface shading model became PBR, not because a
system was added around it.

## What was measured

| | |
| --- | --- |
| git commit | `6f94dba` (`phase-5` HEAD); comparison build `bb78025` (`test-branch-1208`) |
| build | `cargo build --release --bin mygame`, no validation layer |
| GPU | AMD Radeon RX 6700 XT, 12 GiB |
| CPU/OS | Windows 10 Pro 19045 |
| resolution | 2560×1440 |
| scene | `--scene combat` |
| flags | `--width 2560 --height 1440 --scene combat --no-audio --frames 1500 --warmup 600 --shadows off --hidden`, plus `--shadow-tweak local-lights=false` where a row says lights off |
| statistic | GPU p50 from `final GPU window:` |
| repetitions | 2–3 per cell, **interleaved** across binaries, median reported |

Every variant below was built in a throwaway `git worktree`; the working tree was never
modified. All worktrees were removed afterwards.

### Why 1440p and not 720p

At 720p the same binary spreads 0.080–0.137 ms GPU p50 between runs: at ~0.1 ms frames the
GPU never leaves its low clock state. At 1440p repetitions of the same binary agreed to
0.000–0.002 ms throughout this session, which is what makes a 0.02 ms step readable.

### Why release only

The dev profile is `opt-level = 1` **and** turns on the D3D12 debug layer
(`crates/engine-renderer/src/lib.rs:139`). Dev numbers are 2–3× worse and are not renderer
measurements.

## Question 1 — the shadows-off permutation

The three shadow call sites in `shade_surface`
(`crates/engine-renderer/shaders/scene_frame.hlsli`) are already guarded at runtime:
`directional_shadow` + `contact_shadow_at` behind `receives_shadow > 0.5 && lambert > 0.0`,
and `local_light_shadow` behind `receives_shadow > 0.5` inside the local-light loop. With
shadows off they execute almost nothing — but a shader's occupancy is set by its worst path,
not the path a pixel takes, so they may still cost. The diagnostic replaced all three with
`float shadow = 1.0;`, which is exactly what a `SHADOWS_OFF` preprocessor variant would
produce: dxc then drops `directional_shadow`, `shadow_pcf`, `contact_shadow` and the rest.

| build | shadows off, lights on | shadows off, lights off |
| --- | ---: | ---: |
| HEAD | 0.405 | 0.351 |
| HEAD, shadow code compiled out | 0.379 | 0.328 |
| **win** | **0.026** | **0.023** |

The effect is real and perfectly repeatable — three interleaved repetitions per cell agreed
to 0.000 ms — and it is what the occupancy hypothesis predicted. It is also small: 6% of the
frame, against a 0.03 ms bar.

For scale, the shadow code is most of the shader by size and almost none of it by time:

| mesh `PSMain` DXIL | bytes |
| --- | ---: |
| with shadow code | 45 635 |
| without | 16 411 |

**Decision: not built.** 0.023–0.026 ms does not pay for six extra PSOs, a per-frame variant
switch on `shadow.settings().enabled`, two shader variants that can drift, and the equality
test needed to keep them from drifting.

### The plumbing costs nothing

A second ablation removed the shadow stage entirely — `record_shadow_stage` skipped,
`bind_shadow_tables` a no-op — on top of the shader change:

| build (shadows off, lights off) | GPU p50 |
| --- | ---: |
| HEAD | 0.350 |
| shader shadow code compiled out | 0.326 |
| that, **and** no shadow stage, no shadow table binds | 0.327 |

With shadows off the stage already records nothing measurable: `final shadow-stage GPU:`
reads 0.000 ms, and removing the recording and the three descriptor-table binds changes the
frame by less than the noise floor. Whatever a shadows-off frame pays, it pays inside the
mesh pixel shader.

## Question 2 — where the frame went

`bb78025` has no shadow system, no local lights and Lambert shading; HEAD has all three.
The scene is not the difference — the two builds draw the same frame:

| | `bb78025` | HEAD |
| --- | ---: | ---: |
| draws | 33 (4 skinned) | 33 (4 skinned) |
| triangles | 2 884 | 3 186 |
| visible entities | 93 | 96 |
| UI | 231 quads / 91 cmds | 334 quads / 129 cmds |

Replacing all three mesh pixel shaders with `return float4(0.5, 0.5, 0.5, 1.0);` in **both**
builds prices everything that is not surface shading:

| build, mesh PS returning a constant | GPU p50 |
| --- | ---: |
| `bb78025` | 0.079 |
| HEAD (shadows off, lights off) | 0.080 |

Identical. Turning the UI off (`--no-ui`) moves each build by 0.003 ms. The regression is
100% mesh pixel shader.

### The ladder

Built at HEAD, each variant adding one thing to the previous one, all with shadows off,
local lights off, dead shadow code compiled out except in the last row:

| mesh pixel shader | GPU p50 | step |
| --- | ---: | ---: |
| constant colour | 0.080 | — |
| + base-colour fetch, Lambert, ambient, emissive (= `bb78025`'s shader) | 0.090 | +0.010 |
| + ORM and emissive map fetches | 0.106 | +0.016 |
| + normal-map fetch and the TBN transform | 0.177 | +0.071 |
| + the PBR BRDF and the full shading path | 0.327 | +0.150 |
| + shadow code, compiled in but switched off | 0.351 | +0.023 |

The second row lands on 0.090 — the `bb78025` build's own number, to three decimals. The two
builds' floors and their Lambert shaders are the same thing measured twice, which is the
check that the ladder is measuring what it claims to.

### The ~0.03 ms that was previously unaccounted for

An earlier single-feature investigation priced HEAD's PBR at 0.205 ms (BRDF 0.105,
TBN/normal mapping 0.057, three extra fetches 0.019 — summing to 0.181), which left roughly
0.03 ms of the frame unexplained and looked like it might be shadow plumbing. It is not.
The ladder attributes 0.247 ms to the same features, and the difference is **interaction**:
each feature is more expensive in the presence of the others. Measured alone, TBN costs
0.057; measured on top of the extra fetches, 0.071. The BRDF alone costs 0.105; on top of
normal mapping, 0.150. This is the same occupancy effect that makes the dead shadow code
cost 0.023 ms — the pixel shader's register pressure is a property of the whole shader, and
single-feature diffs systematically under-count it.

There is no missing system, no CPU-side regression (the CPU−GPU gap is constant across every
build measured), and no plumbing overhead to reclaim.

## Frame budget today

Shadows off, local lights off, 1440p, 0.351 ms:

| | ms | share |
| --- | ---: | ---: |
| everything but the mesh pixel shader | 0.080 | 23% |
| material fetches (4 maps) + Lambert | 0.026 | 7% |
| normal mapping (TBN) | 0.071 | 20% |
| PBR BRDF | 0.150 | 43% |
| shadow code, switched off | 0.023 | 7% |

With the four brazier lights on, the frame is 0.405 ms; the lights add 0.054 ms.

## What was not done

* **The permutation.** Step 1 measured under the bar, so `scene_frame.hlsli`, `build.rs`,
  `mesh_pipeline.rs` and `renderer.rs` are untouched.
* **The BRDF micro-optimisations.** Hammon visibility and dropping a redundant `normalize`
  measured 0.405 → 0.392 in an earlier session. Deliberately out of scope here: one variable,
  one measurement. They are the obvious next thing to look at, since the BRDF is 43% of the
  shadows-off frame.
