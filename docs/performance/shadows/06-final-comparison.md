# Milestone 8 — the final comparison

Everything, measured once more in one interleaved session against the binary the work started
from.

## Method

Identical to Milestone 0 and unchanged since: `--scene combat`, no input so the camera stands
where the scene put it, 900 measured frames after 600 discarded, three interleaved repetitions,
median reported, windowed and without vsync, FPS derived as `1000 / max(CPU p50, GPU p50)`.
GPU spread was ≤ 0.014 ms in every cell except one, so a GPU difference of 0.03 ms or more is
real.

Four configurations:

| Row | What it is |
| --- | --- |
| **pre-shadow baseline** | `--shadows off --shadow-tweak local-lights=off` — no shadow work and no brazier lights. A *proxy*: it is the optimized binary with both features switched off, not a build of the commit before the shadow work. It is the honest available answer to "what did the frame cost before any of this", and it is labelled as a proxy everywhere it appears. |
| **current HEAD** | the Milestone 0 binary — the shadow implementation as it stood before this work — at Ultra and at the default High. |
| **optimized** | this branch, same presets. |
| **shadows disabled** | this branch at `--shadows off`, so the braziers are still lit. |

## The table

GPU p50 in milliseconds; FPS from the slower of CPU p50 and GPU p50.

| Resolution | Configuration | CPU p50 | GPU p50 | FPS |
| --- | --- | ---: | ---: | ---: |
| 720p | pre-shadow baseline (proxy) | 0.229 | 0.081 | 4367 |
| 720p | shadows disabled | 0.244 | 0.095 | 4098 |
| 720p | current HEAD, High | 0.292 | 0.231 | 3425 |
| 720p | current HEAD, Ultra | 0.357 | 0.307 | 2801 |
| 720p | **optimized, High (default)** | 0.289 | **0.190** | 3460 |
| 720p | **optimized, Ultra** | 0.299 | **0.216** | 3344 |
| 1080p | pre-shadow baseline (proxy) | 0.262 | 0.113 | 3817 |
| 1080p | shadows disabled | 0.269 | 0.132 | 3717 |
| 1080p | current HEAD, High | 0.488 | 0.454 | 2049 |
| 1080p | current HEAD, Ultra | 0.639 | 0.606 | 1565 |
| 1080p | **optimized, High (default)** | 0.403 | **0.358** | 2481 |
| 1080p | **optimized, Ultra** | 0.438 | **0.396** | 2283 |
| 1440p | pre-shadow baseline (proxy) | 0.209 | 0.159 | 4785 |
| 1440p | shadows disabled | 0.256 | 0.210 | 3906 |
| 1440p | current HEAD, High | 0.793 | 0.758 | 1261 |
| 1440p | current HEAD, Ultra | 1.113 | 1.008 | 898 |
| 1440p | **optimized, High (default)** | 0.619 | **0.583** | 1616 |
| 1440p | **optimized, Ultra** | 0.660 | **0.633** | 1515 |
| 4K | pre-shadow baseline (proxy) | 0.370 | 0.322 | 2703 |
| 4K | shadows disabled | 0.446 | 0.416 | 2242 |
| 4K | current HEAD, High | 1.906 | 1.598 | 525 |
| 4K | current HEAD, Ultra | 2.768 | 2.754 | 361 |
| 4K | **optimized, High (default)** | 1.305 | **1.201** | 766 |
| 4K | **optimized, Ultra** | 1.895 | **1.327** | 528 |

## Improvement over the starting point

| Resolution | Ultra | High (the default) |
| --- | ---: | ---: |
| 720p | 0.307 → 0.216, **30 %** | 0.231 → 0.190, **18 %** |
| 1080p | 0.606 → 0.396, **35 %** | 0.454 → 0.358, **21 %** |
| 1440p | 1.008 → 0.633, **37 %** | 0.758 → 0.583, **23 %** |
| 4K | 2.754 → 1.327, **52 %** | 1.598 → 1.201, **25 %** |

Frame rate at Ultra: 361 → 528 FPS at 4K, 898 → 1515 at 1440p, 1565 → 2283 at 1080p.

The default preset improved less than Ultra because Milestone 5 also *raised* what it does —
its contact-shadow range and its filter both changed — while dropping its cascade from 2048 to
1536. It is 25 % faster at 4K and 28 MiB smaller.

## Regression against the pre-shadow proxy

How many times the shadowless frame the optimized configurations cost:

| Resolution | current HEAD, Ultra | optimized, Ultra | optimized, High |
| --- | ---: | ---: | ---: |
| 720p | 3.8× | 2.7× | 2.3× |
| 1080p | 5.4× | 3.5× | 3.2× |
| 1440p | 6.3× | 4.0× | 3.7× |
| 4K | **8.6×** | **4.1×** | **3.7×** |

At 4K the shadow system went from costing 7.6 shadowless frames' worth of extra work to 3.1.
In absolute terms the whole shadowing-and-local-lighting bill at 4K Ultra is 1.005 ms against
2.432, and at the default preset 0.879 ms.

## Where the remaining frame goes

Optimized, 4K, Ultra. Every row is a measured difference between two runs, and they are
**additive** — each is the step from the row above to the row below.

| Item | GPU p50 | Share |
| --- | ---: | ---: |
| the scene with no shadows and no local lights | 0.322 | 24 % |
| + the four brazier lights (lighting only) | 0.094 | 7 % |
| + all shadow sampling in the mesh pass, and the atlas draws | 0.489 | 37 % |
| + the contact-shadow pass and its reconstruction | 0.422 | 32 % |
| **= optimized Ultra** | **1.327** | |

The same decomposition at Milestone 0, for the same scene at the same resolution:

| Item | Milestone 0 | Now | Improvement |
| --- | ---: | ---: | ---: |
| scene with no shadows and no local lights | 0.318 | 0.322 | – |
| four brazier lights, lighting only | 0.109 | 0.094 | 14 % |
| shadow sampling and atlas draws | 0.764 | 0.489 | **36 %** |
| contact shadows | 1.307 | 0.422 | **68 %** |
| **total** | **2.498** | **1.327** | **47 %** |

Two further splits inside those rows, each its own pair of runs:

* **The atlas draws are 0.030 ms** — 2 % of the frame — and were never the problem. The shadow
  stage's GPU timer reads 0.307 today because it now brackets the contact pass as well; 0.030
  is what it reads with contact shadows off. At Milestone 0 the same timer read 0.166 because
  it then bracketed the atlas *and* the depth prepass, which the contact pass now owns. The two
  timer readings are not comparable; 0.030 is the number to read.
* **Local-light shadow lookups are 0.059 ms.** Switching the braziers off entirely saves
  0.153 ms at Ultra, of which 0.094 is the lighting measured with shadows off; the remainder is
  what the atlas lookups for the two shadow-casting braziers cost.

## What each milestone contributed

Every figure is GPU p50 at 4K, measured against the binary immediately before it.

| Milestone | Change | 4K effect |
| --- | --- | ---: |
| 1 | clustered local-light culling | +0.11 at 4 lights, **−76 %** of the lighting at 32 |
| 2 | half-resolution contact-shadow pass | **−1.10** |
| 3 | 8-step, 20 m march | −0.06 |
| 4 | 6-tap rotated disc instead of 5×5 | **−0.22** |
| 5 | High at 1536, Low at 35 m | −0.40 at High, −0.04 elsewhere |
| 6 | filter level of detail | −0.02 |
| 7 | importance-ranked local shadow budget | 0 in this scene; **−2.7 %** where 32 extra lights ask |

Two of them did the work: half-resolution contact shadows and the disc filter, together 1.32 ms
of the 1.43 saved at Ultra. Milestone 1 is not on that list and belongs there anyway — it costs
0.11 ms in a four-light arena and saves 2.1 ms in a thirty-two-light one, and it removed a cap
that was silently discarding twelve of the dungeon's twenty torches every frame.

## Scaling, which is the other half of the result

The frame above is one scene with four lights. What changed underneath it, GPU p50 with
`--shadows off` so this is the lighting alone:

| Local lights | Before (4K) | After (4K) |
| ---: | ---: | ---: |
| 4 | 0.592 | 0.418 |
| 8 | 0.808 | 0.445 |
| 16 | 1.247 | 0.521 |
| 32 | 2.753 | 0.648 |

0.070 ms per light became 0.008. And with all thirty-six of them asking for shadow maps, the
budget holds the shadow cost to +0.042 ms rather than to eight times the shadow stage.

## Acceptance

| | |
| --- | --- |
| Dynamic shadows remain enabled | yes — every preset above Off, with contact shadows at High and Ultra |
| Brazier/local-light performance substantially improved | yes — 0.070 ms per light to 0.008 at 4K |
| Contact shadows no longer dominate GPU time | yes — 52 % of the frame to 32 %, and 1.307 ms to 0.422 |
| Filtering uses significantly fewer samples | yes — 25 taps to 6, at the same penumbra width |
| Quality scales with distance and preset | yes — geometry LOD per cascade, filter LOD past the near cascade, four tiers of cascade count, resolution, range and filter |
| Local shadow casters have a budget and an LOD | yes — importance ranking, a view budget, and an importance floor |
| Visually convincing | yes, with the caveats each milestone document records |
| Architecturally maintainable | `ShadowSettings`, `ContactShadowSettings`, `LocalShadowSettings`, `ShadowLodSettings` and `LightClusters` own their own numbers; nothing quality-related is hard-coded in a shader |
| Every milestone has measured results | six documents, all with real numbers |
| Methodology identical between milestones | one script, unchanged since Milestone 0 except for optional fields |
| No optimization specific to the benchmark scene | none; the scene controls (`--lights`, `--shadow-lights`) add load rather than remove it |

## What is left

In the order they are worth doing:

1. **The mesh pass is the frame.** With contact shadows and filtering brought down, 61 % of the
   4K Ultra frame is directional shadowing plus local-light lookups in the mesh pixel shader,
   and the shadow *stage* is 2 %. Any further shadow work is shader work.
2. **A cached static cascade.** Every cascade is redrawn every frame. The far cascades are
   almost entirely static geometry; caching them and redrawing only the movers would take most
   of the 0.030 ms atlas pass and, more importantly, most of the 767 caster draws. `docs/shadows.md`
   already lists this as a deliberate limitation with no measurement justifying the invalidation
   logic — that is still true, and the number to beat is now smaller than it was.
3. **A depth pre-culled cluster grid.** The light grid does not use the depth buffer, so a
   cluster behind a wall still lists the light in front of it. It needs a compute pass, and it
   buys nothing until scenes routinely carry more than thirty-two lights.
4. **Per-light shadow-map resolution**, in the tiered-slot shape Milestone 7 describes, if a
   scene ever wants one dominant light much sharper than the rest.
5. **Not shadow work at all**: the CPU p95 in every table above is dominated by the desktop
   compositor, not by the engine. A fullscreen measurement path would make the CPU column as
   trustworthy as the GPU one.

## Reproducing

```powershell
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 -Repeats 3 `
  -Out docs/performance/shadows/data/06-final.json `
  -Configs "pre-shadow proxy|off|local-lights=off|;shadows disabled|off||;current HEAD Ultra|ultra||target/release/mygame-head.exe|;current HEAD High|high||target/release/mygame-head.exe|;optimized Ultra|ultra||;optimized High|high||;optimized Ultra no contact|ultra|contact=off|;optimized Ultra no local lights|ultra|local-lights=off|"
```

The `mygame-head.exe` reference is built from the Milestone 0 commit in a worktree, with the
`--lights`/`--shadow-lights` controls copied in so both binaries accept the same command line.
