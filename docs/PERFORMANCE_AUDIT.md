# Performance audit — 2026-10

A measured audit of the whole game: launch, frame, GPU, memory, pacing. Everything below was
measured on one machine in one week; nothing is estimated unless it says so. The data files
are in `docs/performance/audit/` and the drivers that produced them in `tools/bench/`.

**Machine.** Ryzen 9 5900X (12 cores, 24 threads), Radeon RX 6700 XT, Windows 10 19045, a
1920x1080 desktop at 240 Hz. Not the 5800X the older sections of `docs/performance.md` were
measured on, so none of their numbers are compared with these: every "before" here was
re-measured on this machine, and every before/after pair was interleaved in one session.

**Every game run is `--hidden`.** No window is on screen, so DWM composites nothing. GPU times
and `Present` are therefore the *uncontended* ones — close to the fullscreen column of
`docs/performance.md`, Milestone 17 — and not what a windowed player on a busy desktop sees.
Windowed and fullscreen runs were deliberately not part of this audit (they take over the
screen); the relative results hold for both, the absolute ones are the hidden case.

---

## Concise summary

```text
BIGGEST CURRENT BOTTLENECKS

1. Command recording on the main thread, above all the shadow stage: 0.12-0.33 ms of CPU a
   frame (cascades, local views and the depth prepass each record their own batches), which
   is what keeps the busier levels CPU-bound at 720p-1080p.
2. GPU: the camera's mesh pass at 1440p and above, about half of it the shadow *sampling*
   (arena at 1080p: 0.31 ms with shadows, 0.16 ms without; the whole frame 0.43 vs 0.19 ms).
3. Launch: D3D12CreateDevice is ~90-100 ms of driver work the engine cannot shorten — now
   ~45% of a 220-260 ms launch, against ~10% of the 1.0 s one.

BIGGEST IMPROVEMENTS IMPLEMENTED

1. Texture processing at load: exact table-driven sRGB mip filtering, row-banded mip and
   BC1/BC5 compression, a word-at-a-time content hash — 690 ms -> ~95 ms of CPU, same bytes.
2. Load ordering: hero, monster and environment models built, decoded, filtered and
   compressed on a thread while the device is created — the main thread never waits for it.
3. Two parallel paths that were slower than serial in the game (UI batching, particles),
   found by measuring against --serial-jobs; plus bone palettes uploaded once a frame instead
   of once per pass per character.

STARTUP (warm, median of 9 interleaved launches)

Before:  empty 945 ms | dungeon 997 ms | graveyard 1022 ms | editor + showcase 1037 ms
After:   empty 223 ms | dungeon 241 ms | graveyard  262 ms | editor + showcase  282 ms
Improvement: 3.7x-4.2x

GAMEPLAY FPS (1920x1080 hidden, monster-playground fight, 21 monsters; avg over 6,000 frames)

Before: 1483
After:  2013   (+36%)

1% LOW

Before: 1087
After:  1199   (+10%)

CPU FRAME (same scene, p50)

Before: 0.672 ms
After:  0.505 ms   (-25%)

GPU FRAME (same scene, p50)

Before: 0.476 ms
After:  0.386 ms   (the GPU work is unchanged; it runs at higher clocks when it is fed
                    faster — see 5.3)

REMAINING BOTTLENECK

Shadow-stage command recording (CPU) in the dense levels, and mesh-pass shadow sampling
(GPU) at 1440p+.

NEXT HIGHEST-VALUE OPTIMIZATION

Record the shadow views in parallel on their own command lists (or stop re-recording static
casters every frame); the job system needs a cheaper wake-up first, because in the game its
workers are asleep and a split pays ~80 us before any work starts (section 14).
```

---

## 1. Executive summary

The game was already fast: every scenario but the synthetic VFX stress test runs at
1,400-6,000 FPS at 1080p on this machine, against the 240/360/480 FPS targets. Two things
were not:

* **Launch took a second**, 70% of it in one function: `create_model` filtering mip chains
  with fifteen `powf` per output texel and block-compressing on one core, all of it after
  the driver had finished starting. Launch is now **~4x faster** (945-1022 ms to 223-262 ms)
  with byte-identical GPU textures (a digest of every uploaded level is unchanged).
* **The heaviest fight lost a quarter of its frame to a parallel path.** The UI batcher split
  itself across the job system above a threshold derived from a benchmark whose workers are
  awake; in the game they are asleep, and the split cost 2.2x the serial batch. The same was
  true of particles. Fixing both and uploading bone palettes once a frame took the fight from
  **0.672 to 0.505 ms** CPU (1483 to 2013 FPS).

Along the way the audit found the game allocating 1-13 times a frame where the docs said
zero, and brought it to effectively zero; found no hitches, no mid-game pipeline creation and
no upload stalls in any steady-state run; and added the instrumentation that made all of it
measurable — run-wide p99.9 and 0.1% lows, a hitch log, run-wide stage means, a seven-pass GPU
split, RAM/VRAM, allocation counting and tracing, a texture-content digest — with no
measurable cost (5 interleaved runs: 0.271 vs 0.269 ms and 0.485 vs 0.482 ms CPU p50, pristine
`HEAD` vs instrumented).

What was *not* changed: no visual feature, no setting, no shader, no gameplay rule, no public
API removed, no architectural boundary crossed (the preload thread lives in `engine-app`; the
renderer gained a pure `prepare_model_textures` that needs no device).

---

## 2. Startup profile

`--features startup-timing`, warm launches, median of 9 interleaved runs per column
(`final-startup.json`). The dungeon:

| Stage | Before | After | What it is |
| --- | ---: | ---: | --- |
| window init | 2.6 | 2.6 | `CreateWindowExW`, hidden |
| D3D12 device | 119.7 | 127.7 | factory 1.8, adapters 0.1, **`D3D12CreateDevice` ~88-100**, queue 2.6-4.5, swap chain ~9 |
| renderer objects | 45.4 | 45.3 | UI state 16-20 (pipeline, atlas, a synchronous flush), shadow renderer ~10, upload ring ~4, depth ~2.6, the rest |
| GPU upload: shared | 9.6 | 7.1 | character, decal, VFX and debug meshes |
| GPU upload: environment | 34.3 | 7.0 | 29 models; authored in code and prepared on the preload thread after |
| GPU upload: heroes | 698.9 | 10.5 | before: import ~40, then mips 385, BC 264, hash 46 on this thread; after: upload only |
| GPU upload: monsters | 54.4 | 11.5 | 7 procedural models |
| dungeon map | 10.1 | 8.9 | the baked map picture |
| GPU flush, frame 1 | 6.6 | 6.4 | one submission and one wait for the whole startup batch; first frame |
| **TOTAL** | **996.5** | **240.6** | |

After, in parallel on the preload thread (not on the critical path; the main thread waited
**0.0 ms** for it in every run): hero import ~42 ms (two 2048x2048 JPEGs), monsters 13 ms,
texture preparation ~85 ms (8 bands, see 12.4-12.7), environment library ~21 ms.

| Scene | Before | After |
| --- | ---: | ---: |
| Empty (`--scene character`) | 945.4 ms | 222.9 ms |
| Dungeon | 996.5 ms | 240.6 ms |
| Graveyard | 1022.4 ms | 261.6 ms |
| Editor + `showcase.json` (pristine `HEAD` build vs final, 7 runs) | 1037.3 ms | 282.0 ms |

**Cold launches.** The first launch of a freshly built or copied executable measured 640-860 ms
for the final build and 1.5-1.6 s for the old one in some sessions, and as fast as a warm
launch in others. The extra time is not in any stage the engine controls (it appears before
and around the device), and repeats only for a new executable path — consistent with on-access
antivirus scanning and the driver's per-application caches. It is reported separately by
`startup_matrix.ps1` ("cold") and was not optimized.

## 3. CPU profile

Run-wide stage means, 1080p, final build (`final-pacing.json`, 3 runs each, ms per frame):

| Scenario | update | of which UI | VFX | extract (animation) | render | present | draws (skinned) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Empty | 0.025 | 0.016 | 0.000 | 0.004 (0.003) | 0.213 | 0.037 | 35 (1) |
| Arena | 0.042 | 0.027 | 0.002 | 0.005 (0.003) | 0.409 | 0.038 | 346 (1) |
| Fight (21 monsters) | 0.198 | 0.104 | 0.065 | 0.022 (0.018) | 0.256 | 0.041 | 517 (95) |
| Dungeon (24 monsters) | 0.051 | 0.017 | 0.001 | 0.025 (0.020) | 0.301 | 0.034 | 580 (109) |
| Graveyard (38 characters) | 0.084 | 0.015 | 0.004 | 0.038 (0.030) | 0.409 | 0.038 | 1055 (173) |
| Encounter (10 enemies) | 0.056 | 0.036 | 0.001 | 0.014 (0.011) | 0.211 | 0.038 | 248 (46) |
| VFX stress | 0.552 | 0.063 | 0.467 | 0.013 (0.003) | 0.771 | 0.041 | 858 (1) |
| Environment stress | 0.053 | 0.024 | 0.001 | 0.005 (0.004) | 0.607 | 0.040 | 1617 (1) |

Simulation is ~0.0005 ms a frame averaged (one 60 Hz tick every few frames), the camera's
collision sweeps 0.001-0.004 ms, audio 0.0004 ms. **Gameplay is not the frame's problem
anywhere.** The frame is the render stage.

**The render stage, split** (`run render` line; dungeon, 1080p): 0.29-0.30 ms = GPU wait 0.05 +
cull 0.003 + group 0.008 + shadow prep 0.013 + **recording 0.20-0.22**. Recording, split by
section with temporary timers (dungeon / fight / graveyard / environment stress, before the
palette change):

| Section | Dungeon | Fight | Graveyard | Env stress |
| --- | ---: | ---: | ---: | ---: |
| Shadow stage (atlas cascades, local views, prepass) | 0.133 | 0.121 | 0.250 | 0.326 |
| Opaque camera batches (instanced) | 0.013 | 0.011 | 0.027 | 0.020 |
| Skinned characters | 0.059 | 0.050 | 0.089 | 0.002 |
| Finish: particles, UI vertex upload, close, execute | 0.026 | 0.076 | 0.030 | 0.027 |

A shadow draw costs ~0.35 us of CPU against ~0.1 us for an instanced camera batch: each view
re-culls, re-groups, re-uploads its instances and re-records its batches, every frame.

**Skeletal animation** is ~0.75-0.8 us per character in the game and in `dungeon_bench`'s new
animated scenarios (extraction 1.9 -> 19.1 us for 22 posed monsters). Not a bottleneck: the
plan's trigger for throttling off-screen animation (>5% of the frame) was not met.

**Where the hidden runs are bound.** CPU at 720p everywhere, and at 1080p in the fight, the
graveyard and the dungeon (the fence wait inside `begin_recording` is ~0-0.05 ms); GPU at 1440p
and above everywhere, and at 1080p in the arena (0.27 ms of fence wait).

## 4. GPU profile

GPU timestamps per pass, p50 ms, final build. The shadow stage is now split into its three
parts (`gpu_timer::SHADOW_ATLAS_PASS`, `DEPTH_PREPASS`, `CONTACT_PASS`), which are inside it,
not added to it.

| Scenario | Res | Frame | Shadow stage | atlas | prepass | contact | Mesh pass | Particles | UI |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Arena | 1080p | 0.432 | 0.096 | 0.018 | 0.035 | 0.035 | 0.308 | 0.000 | 0.008 |
| Arena, shadows off | 1080p | 0.185 | — | — | — | — | 0.156 | 0.000 | 0.008 |
| Arena | 4K | 1.536 | 0.286 | 0.022 | 0.142 | 0.115 | 1.177 | 0.001 | 0.008 |
| Arena, shadows off | 4K | 0.627 | — | — | — | — | 0.553 | 0.001 | 0.009 |
| Fight | 1080p | 0.313 | 0.091 | 0.020 | 0.030 | 0.034 | 0.168 | 0.000 | 0.033 |
| Dungeon | 1080p | 0.337 | 0.099 | 0.023 | 0.034 | 0.035 | 0.209 | 0.000 | 0.007 |
| Graveyard | 1080p | 0.477 | 0.123 | 0.038 | 0.044 | 0.035 | 0.324 | 0.000 | 0.007 |
| Graveyard | 4K | 1.483 | 0.312 | 0.041 | 0.151 | 0.114 | 1.099 | 0.000 | 0.008 |
| VFX stress | 1440p | 1.501 | 0.199 | 0.029 | 0.104 | 0.059 | 0.407 | 0.837 | 0.009 |
| VFX stress | 4K | 3.659 | 0.508 | 0.029 | 0.317 | 0.148 | 1.239 | 1.788 | 0.010 |
| Env stress | 4K | 2.262 | 0.305 | 0.041 | 0.146 | 0.112 | 1.880 | 0.000 | 0.008 |

Readings:

* **The mesh pass is the GPU frame** at every resolution, and it scales with pixels (arena
  0.150 -> 1.177 ms from 720p to 4K): pixel-bound, not vertex- or draw-bound.
* **Shadows cost the GPU 57-59% of the arena's frame** (0.432 vs 0.185 ms at 1080p, 1.536 vs
  0.627 at 4K). The shadow *stage* is the smaller half (0.096 ms at 1080p: the atlas is cheap,
  0.018; the prepass and the half-resolution contact march 0.035 each); the larger half is the
  mesh pass doing PCF, the cascade blend, the contact read and local-light shadows per pixel
  (0.308 vs 0.156 ms). Phase 7b already optimized both halves; this audit changed neither — no
  shader edit is justified without an overdraw measurement first (section 15).
* **Particles at 1440p+ under the synthetic stress** are fill-rate: 0.84 ms at 1440p and
  1.79 ms at 4K for ~10,000 overlapping billboards. Real combat peaks at a few hundred.
* The UI is 0.007-0.059 ms at every resolution; the atlas fill 0.01-0.04 ms.

Not measured: per-pass draw/triangle/pixel counts, barriers and descriptor changes per pass,
overdraw. The frame totals are in section 13; per-pass counts and a pipeline-statistics query
(pixel-shader invocations against screen pixels) are the next GPU instrumentation step.

## 5. Frame-time profile

### 5.1 Distribution, 1920x1080 hidden, before vs after

The unoptimized and the final build interleaved in one session, 3 x 6,000 measured frames
each after 600 warm-up (`final-pacing.json`). "1% low" is `1000 / p99`, "0.1% low" is
`1000 / p99.9`, over the whole measured run.

| Scenario | Build | CPU mean | CPU p50 | CPU p99 | CPU p99.9 | max | avg FPS | 1% low | 0.1% low | GPU p50 | GPU p99 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Empty | before | 0.277 | 0.271 | 0.354 | 1.018 | 1.130 | 3616 | 2821 | 983 | 0.229 | 0.235 |
| Empty | after | 0.276 | 0.271 | 0.351 | 1.044 | 1.089 | 3622 | 2845 | 958 | 0.230 | 0.237 |
| Arena | before | 0.488 | 0.481 | 0.689 | 1.195 | 1.324 | 2048 | 1451 | 837 | 0.438 | 0.457 |
| Arena | after | 0.489 | 0.482 | 0.686 | 1.262 | 1.320 | 2043 | 1458 | 792 | 0.438 | 0.458 |
| Arena, shadows off | before | 0.229 | 0.225 | 0.315 | 0.914 | 1.035 | 4367 | 3175 | 1094 | 0.188 | 0.194 |
| Arena, shadows off | after | 0.228 | 0.224 | 0.293 | 0.988 | 1.058 | 4378 | 3407 | 1013 | 0.188 | 0.194 |
| **Fight** | before | 0.674 | 0.672 | 0.920 | 1.025 | 1.161 | 1483 | 1087 | 976 | 0.476 | 0.603 |
| **Fight** | after | **0.497** | **0.505** | **0.834** | 1.034 | 1.313 | **2013** | **1199** | 967 | 0.386 | 0.566 |
| Dungeon | before | 0.392 | 0.385 | 0.612 | 1.164 | 1.321 | 2550 | 1633 | 859 | 0.340 | 0.351 |
| Dungeon | after | 0.388 | 0.381 | 0.558 | 1.063 | 1.239 | 2577 | 1794 | 941 | 0.339 | 0.348 |
| Graveyard | before | 0.540 | 0.525 | 0.823 | 0.958 | 1.129 | 1852 | 1215 | 1044 | 0.478 | 0.638 |
| Graveyard | after | 0.532 | 0.509 | 0.808 | 1.089 | 1.365 | 1881 | 1238 | 918 | 0.477 | 0.490 |
| Encounter (10) | before | 0.305 | 0.299 | 0.439 | 1.057 | 1.140 | 3276 | 2279 | 946 | 0.259 | 0.270 |
| Encounter (10) | after | 0.305 | 0.298 | 0.466 | 1.085 | 1.263 | 3274 | 2147 | 922 | 0.257 | 0.269 |
| VFX stress | before | 1.394 | 1.134 | 3.167 | 3.738 | 3.883 | 717 | 316 | 268 | 1.035 | 3.059 |
| VFX stress | after | 1.390 | 1.059 | 3.016 | 3.507 | 3.834 | 719 | 332 | 285 | 0.992 | 2.917 |
| Env stress | before | 0.699 | 0.692 | 0.979 | 1.455 | 1.538 | 1431 | 1022 | 687 | 0.649 | 0.835 |
| Env stress | after | 0.701 | 0.693 | 1.011 | 1.455 | 1.597 | 1426 | 989 | 687 | 0.650 | 0.836 |

The empty scene, the arena, the encounter and the environment stress are the controls: none
of the runtime changes touch their frame (their UI and particle counts are below every
threshold, and they have one skinned character), and they did not move.

### 5.2 Every frame against the 240 FPS budget

No steady-state run of either build had a single frame over 4.17 ms (one refresh at 240 Hz),
or a hitch by the log's definition (over 4x the running median *and* over 2 ms), in 2 builds x
9 scenarios x 3 runs x 6,000 frames — except the VFX stress test, whose p99 of ~3 ms is the
GPU drawing 10,000 particles and is not a hitch either.

### 5.3 Why the fight's GPU time fell

The fight's GPU *work* is the same in both builds — the same draws, the same particles, the
same passes. Its timestamps fell from 0.476 to 0.386 ms because the GPU was fed sooner: at
2,000+ FPS the GPU idles between frames, and an RX 6700 XT that idles less holds higher clocks.
The same effect is visible run to run in the stress scene. GPU time is a property of the
machine's state as well as of the work, which is why every comparison here is interleaved.

### 5.4 The p99.9 floor

Every scene, including the empty one, has a p99.9 around 1.0-1.2 ms — four times its median —
in both builds. The hitch log never attributed one of these frames to a pipeline, a resource
or an upload, and their size does not move with the scene. They read as environmental
(scheduler quanta, driver housekeeping) rather than engine work; they are below the 2 ms hitch
floor and were not chased further.

## 6. Memory profile

| Scenario | RAM working set | Private bytes | VRAM (local) |
| --- | ---: | ---: | ---: |
| Empty | 163 MiB | 263 MiB | 120 MiB |
| Arena | 162 MiB | 260 MiB | 122 MiB |
| Arena, shadows off | 162 MiB | 262 MiB | 74 MiB |
| Fight | 166 MiB | 264 MiB | 120 MiB |
| Dungeon | 162 MiB | 262 MiB | 124 MiB |
| Graveyard | 180 MiB | 293 MiB | 138 MiB |

From the report's new memory line (`GetProcessMemoryInfo`, `IDXGIAdapter3::QueryVideoMemoryInfo`,
two kernel calls per report interval). Unchanged by the audit. VRAM is dominated by the shadow
atlas and its targets (48 MiB between shadows on and off at High) and the textures; the
budget the OS grants is 11.5 GB. The peak working set is reached during the launch, while
model textures and their mip chains are resident on the CPU: 174-202 MiB before, 177-202 MiB
after — the preload thread holds prepared textures alongside their models for a few tens of
milliseconds, which costs at most ~4 MiB of peak.

**Allocations.** Counted in the full game for the first time (`--features alloc-count`):

| Scenario | Before (allocs/frame) | After | Bytes/frame after |
| --- | ---: | ---: | ---: |
| Empty | 1.021 | 0.019-0.021 | 1 |
| Arena | 3.028 | 0.029 | — |
| Dungeon | 3.022 | 0.019-0.022 | 1 |
| Graveyard | 3.024 | 0.024 | — |
| Encounter | — | 0.024 | — |
| Fight | 13.056 | 0.050-0.053 | ~450 |
| VFX stress | 14.648 | 0.059 | — |

Found with the new allocation tracer (`MYGAME_ALLOC_TRACE=<n>` captures the backtrace of the
first *n* allocations after warm-up and groups them by call site): one `Vec` per frame in
`shadow::cascade::split_cascades`, two `String`s per frame in `DungeonMap::push_minimap`, the
job system's per-call boxes whenever a pass split, and a half-window merge buffer in every
`FrameStatsWindow::stats` (a stable sort, four times a second). In the static scenes what is
left is the exit path — the HUD layout saved and the closing report's log records, ~115
allocations that land inside the measured window — so a steady-state frame allocates nothing.
The fight's residue is high-water-mark growth of effect and render-list buffers.

## 7. Loading profile

The asset pipeline, for the one asset that dominated it — the Fire Mage's export
(`firemage.glb`: 7,245 triangles, 33 bones, 12 clips, two 2048x2048 JPEG textures, 32 MiB as
RGBA8). Startup-timing probes, dungeon launch, before -> after:

| Step | Before | After | Thread |
| --- | ---: | ---: | --- |
| File read + glTF parse + JPEG decode | ~40 ms | ~42 ms | main -> preload |
| Content hash + transparency scan, all textures | 46 ms | 10 ms | main -> preload |
| Mip filtering, all textures (9.1 Mtexels at level 0) | 385 ms | ~29 ms | main -> preload |
| BC1/BC5 compression, all levels | 263 ms | ~57 ms | main -> preload |
| Resource creation + staging copy + SRV | ~25 ms | ~10 ms | main |
| GPU copy | one batch for the whole launch, one 2-4 ms wait | unchanged | main |

The texture bytes reaching the GPU are identical: with `MYGAME_TEXTURE_DIGEST=1` the build
folds every level of every uploaded texture into one digest, and it is `7d1a3a8090104f93`
before, after each change and at the end, in the dungeon and the arena.

No duplicate loads (the content hash deduplicates 71 texture requests to 50 uploads), no
re-parsing, no per-resource GPU round trips (the startup batch from the earlier startup work
holds), no shader compilation at runtime (DXIL is built at compile time), and 7 pipeline
states created at startup in ~2 ms with a warm driver cache.

## 8. Stutter and frame pacing

* **No hitches in steady state** in any scenario (5.2), and **nothing created during
  gameplay**: a run-wide counter of pipelines, resources and GPU-waiting flushes created after
  the first frames read zero in 20,000-frame runs of the dungeon, the fight, the graveyard and
  the encounter.
* **The first ~10 frames after a load** run at ~2 ms (the render stage waiting on a GPU that
  is still settling); once per load, below any visible threshold.
* **Not exercised by a hidden run**, and therefore not verified: the two translucent-character
  pipeline variants (created lazily the first time the camera is pushed into the player's
  character), opening the full map and menus, swapping heroes. With a warm driver cache a
  pipeline is ~0.3 ms; with a cold one (first run after a driver update) it could be tens of
  milliseconds. The run profile's `run created` line will show any of these in an
  interactive run.
* **Logging and the report** run once per report interval, as one record built into a reused
  buffer; with `--report 0` they do not run at all.

## 9. Renderer findings

* **Submission.** Instancing works: the camera's opaque pass records in 0.011-0.027 ms for
  hundreds of objects. PSO changes are few; descriptor binds go through one shader-visible
  heap; the command context drops redundant binds (Milestone 17).
* **The shadow stage records like it did before instancing paid off for the camera**: every
  cascade, every local view and the prepass re-collect, re-group, re-upload and re-record their
  casters each frame — 0.12-0.33 ms of the main thread. The largest remaining CPU item.
* **Bone palettes were uploaded once per pass per character**: 8 KB each (128 bones), 106
  uploads a frame in the dungeon, 126 in the fight, 160 in the graveyard (0.85-1.3 MB of
  writes). Now once per character per frame (12.10).
* **UI vertices**: 48 bytes x 6 per quad, written to the upload heap and read by the GPU across
  PCIe: ~910 KB a frame for the fight's 3,171 quads, and most of the 0.076 ms "finish"
  section. Indexed or instanced quads would cut it 1.5-4x (section 15).
* **Fence waits** are rare in hidden runs at 1080p (the CPU is the limit); `Present` is
  0.033-0.041 ms.
* **Resource barriers** were not re-measured; the shadow atlas transitions once each way per
  frame, the depth copy for the contact pass is one copy.
* **Pipeline creation** is lazy for mesh variants; no variant was created during any measured
  gameplay run.

## 10. Asset pipeline findings

* Mip filtering used `powf` for every sRGB decode (four per channel per output texel) and
  encode. Both are now tables built from the same formulas: a 256-entry decode table, and for
  encode the 255 smallest floats that round to each byte, found by bisection over float bit
  patterns — proven equal to the formula for **all 1,065,353,217 floats in [0, 1]** by an
  ignored test (`cargo test --release -p engine-renderer every_float -- --ignored`, ~1.4 s).
* Mips and BC1/BC5 compression ran on one core. Both are now split into bands of rows on
  scoped threads; each row (or row of 4x4 blocks) depends only on its own inputs, so the output
  is the same at any band count (determinism tests at several band counts).
* The content hash was FNV-1a a byte at a time — a multiply chain sixteen million links long
  per 2048x2048 texture. It is now word-at-a-time with MurmurHash3's finalizer; it is an
  in-process deduplication key only, nothing persists it.
* All of it ran on the main thread after the device was created, although none of it needs
  the device. It now runs on a preload thread during device creation.
* Not done, and not needed at today's content size: an on-disk cache of processed textures, a
  BC7 encoder (alpha-tested colour stays RGBA8), and async loading during play (there is no
  streaming).

## 11. Optimization candidates

Ranked by measured impact x frequency x risk. "Decision" is what this audit did.

**P0 — Texture processing on the launch's critical path**
Problem: launch ~1 s. Evidence: startup probes, 9-run medians. Measured cost: mips 385 ms, BC
263 ms, hash 46 ms of a 997 ms dungeon launch. Root cause: `powf` per conversion, one core, a
byte-serial hash, all after `D3D12CreateDevice`. Proposed: exact tables, row bands, word hash,
then move the work off the critical path. Expected: needed benchmarking. Risk: low with a
content digest and an exhaustive equivalence test. Complexity: medium. Before: 997 ms. After:
241 ms. **Decision: implemented** (12.1-12.7).

**P0 — Parallel UI batching slower than serial in heavy fights**
Problem: fight frame 0.67 ms, UI 0.19-0.21 ms of it. Evidence: `--serial-jobs` A/B (0.672 vs
0.538 ms CPU p50), section timers. Measured cost: 0.133 ms parallel vs 0.061 ms serial batch.
Root cause: worker wake-up (~80 us) plus a serial merge copy (0.041 ms for 913 KB). Proposed:
raise the threshold past anything the game draws. Risk: low (same code path as serial).
Complexity: trivial. Before: 0.677 ms. After: 0.537 ms. **Decision: implemented** (12.8).

**P1 — Bone palettes uploaded per pass per character**
Evidence: upload counter (106-160 a frame). Measured cost: ~50 us of recording in the
dungeon, ~37 us in the fight. Root cause: each pass built and wrote its own padded copy.
Proposed: one upload per character per frame, shared address. Risk: low (same bytes; visual
check against same-binary variance). Before: fight 0.537 ms. After: 0.500 ms. **Decision:
implemented** (12.10).

**P1 — Parallel particle path slower than serial**
Evidence: VFX split in the stress scene, 10 interleaved pairs. Measured cost: +32 us per frame
in the write, -7 us in integration, VFX stage 0.584 vs 0.430 ms run-wide. Root cause: as the
UI. Before: stress CPU mean 1.438 ms. After: 1.299 ms. **Decision: implemented** (12.9). Only
the synthetic stress test reaches it.

**P1 — Shadow-stage command recording**
Evidence: section timers, 0.12-0.33 ms. Root cause: per-view re-collection and re-recording.
Proposed: parallel command lists per view, or cached static-caster instance buffers. Expected:
needs benchmarking. Risk: medium-high (threading in the renderer, cache invalidation).
**Decision: not implemented**; recommended next (15).

**P1 — Mesh-pass shadow sampling at 1440p+**
Evidence: shadows-off A/B, 0.15 ms at 1080p, 0.62 ms at 4K in the mesh pass. Root cause: PCF,
cascade blend, contact read per pixel. Proposed: measure overdraw first (pipeline statistics);
if there is overdraw, an EQUAL/no-write mesh pass after the prepass. Risk: medium (visual).
**Decision: not implemented**; needs the overdraw measurement.

**P2 — Startup: the remaining main-thread items**
UI state init 16-20 ms (a synchronous flush that could join the startup batch), the editor's
first frame 31 ms, the graveyard's map 27-35 ms. **Decision: not implemented.**

**P2 — UI vertex volume** (~910 KB/frame in the fight). Indexed quads (-33%) or instanced quads
(-75%). Risk: medium (UI shader). **Decision: not implemented.**

**P2 — VFX ground-band rebuilds** (0.056 ms in the fight). Evidence: 9.8 of 18 bands rebuilt a
frame, and a content-keyed lookup would have saved 0.016 of them — the footprints genuinely
move, so the cache is not the problem; the per-piece trigonometry is. **Decision: not
implemented.**

**P3 — Per-draw `HashMap` PSO lookup and COM clone in the recording loops.** Tens of
nanoseconds a draw. **Decision: not implemented.**

**Not a candidate** — skeletal animation throttling (0.75 us a character, under the plan's 5%
trigger); camera collision (1-4 us); simulation (<1 us averaged); audio (0.4 us).

## 12. Implemented optimizations

Each was measured alone against the build before it, interleaved, and kept only on a
measurable win. Byte-identity was proven where it could be (texture digest, exhaustive float
test, determinism tests); where it could not — the palette change — screenshots were compared
against the same binary's own run-to-run variance.

### 12.1-12.3 Texture processing: exact tables, bands, word hash

`engine-renderer/src/mipmap.rs`, `parallel.rs`, `texture_id.rs`; `engine-assets/src/bc.rs`.

| Dungeon launch, 7 runs | Before | Tables + bands | + word hash |
| --- | ---: | ---: | ---: |
| Mip filtering | 384.5 ms | 25.9 ms | 19.9 ms |
| Compression | 262.3 ms | 52.5 ms | 35.4 ms |
| Hashing + scanning | 46.0 ms | 46.1 ms | 9.7 ms |
| **Launch** | **977.9 ms** | **401.0 ms** | **342.1 ms** |

Band count, measured (5 runs each): 4 bands 448 ms, 8 403, 12 392, 16 389, 24 380 — so the
cap is every logical processor (bounded at 32).

### 12.4-12.7 Load ordering: the preload thread

`engine-app/src/app.rs` (`ModelPreload`), `engine-renderer` (`prepare_model_textures`,
`create_model_prepared`, `limit_bands_on_this_thread`).

| Step (9 runs unless noted) | Empty | Dungeon | Graveyard |
| --- | ---: | ---: | ---: |
| Models built/imported on a thread during device creation (7 runs) | 329 -> 271 ms | 355 -> 311 ms | 377 -> 324 ms |
| ...and their textures prepared there | 275 -> 223 ms | 307 -> 255 ms | 331 -> 279 ms |
| ...with that thread capped at 8 bands (5 runs, medians) | — | 256 -> 246 ms | — |
| ...and the environment library too | 216 -> 216 ms | 250 -> 235 ms | 272 -> 256 ms |

The cap exists because the preparation slowed `D3D12CreateDevice` on the main thread
(116 -> 128 ms with 24 bands): 8 leaves the driver its cores and still finishes long before
the uploads ask (`limit_bands_on_this_thread` carries the table).

### 12.8 UI batching threshold

`engine-renderer/src/ui_pipeline.rs`: `MIN_TOTAL_UNITS` 2,048 -> 32,768, `MIN_CHUNK_UNITS`
512 -> 4,096; the parallel tests' fixture grew so they still split.

| 1080p, 3 runs | CPU p50 before | after | avg FPS before | after |
| --- | ---: | ---: | ---: | ---: |
| Fight | 0.677 ms | 0.537 ms | 1488 | 1895 |
| Arena (control) | 0.483 ms | 0.481 ms | 2036 | 2044 |
| VFX stress | 1.085 ms | 1.015 ms | 716 | 788 |

### 12.9 Particle threshold

`engine-vfx/src/particle.rs`: `MIN_TOTAL_PARTICLES` 8,192 -> 32,768 (above the pool's 16,384
capacity). VFX stress, 6 more interleaved pairs after the first 4: CPU mean 1.438 -> 1.299 ms,
p50 1.104 -> 1.033 ms, VFX stage 0.584 -> 0.430 ms, p99 3.21 -> 3.16 ms (unchanged — the tail
is the GPU's). Fight unchanged (control).

### 12.10 Bone palettes once per frame

`engine-renderer/src/palette.rs` (`FramePalettes`): every distinct palette is uploaded, padded
exactly as before, before the shadow stage; every pass binds its address, with the old
per-pass upload as the fallback. Uploads per frame: dungeon 106 -> 24, fight 126 -> 21,
graveyard 160 -> 38.

| 1080p, 4 runs | CPU p50 before | after | avg FPS before | after |
| --- | ---: | ---: | ---: | ---: |
| Fight | 0.537 ms | 0.500 ms | 1895 | 2039 |
| Graveyard | 0.524 ms | 0.510 ms | 1850 | 1865 |
| Dungeon | 0.384 ms | 0.380 ms | 2550 | 2584 |
| Arena (control) | 0.482 ms | 0.481 ms | 2039 | 2047 |

The dungeon gains least because at 1080p its GPU (0.338 ms) is now the limit: its recording
fell from ~0.26 to ~0.21 ms and the saving became fence wait. At 720p it shows (3017 -> 3176
FPS across all changes, section 13).

### 12.11 Per-frame allocations

`shadow/cascade.rs` (fixed array), `ui/dungeon_map.rs` (cached banner title),
`engine-core/src/time.rs` (unstable sort). See section 6.

## 13. Before/after benchmark results

### 13.1 Resolution matrix

The unoptimized and the final build interleaved, 3 runs each, 600 frames after 120 warm-up per
resolution, every resolution in one process (`final-res.json`).

| Scenario | Res | CPU p50 before | after | GPU p50 before | after | FPS before | after |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Fight | 720p | 0.372 | 0.342 | 0.226 | 0.208 | 2646 | 2894 |
| Fight | 1080p | 0.526 | 0.377 | 0.330 | 0.313 | 1862 | 2635 |
| Fight | 1440p | 0.613 | 0.506 | 0.485 | 0.463 | 1608 | 1928 |
| Fight | 4K | 1.062 | 1.033 | 1.011 | 0.991 | 927 | 982 |
| Dungeon | 720p | 0.329 | 0.310 | 0.205 | 0.204 | 3017 | 3176 |
| Dungeon | 1080p | 0.383 | 0.380 | 0.338 | 0.337 | 2564 | 2589 |
| Dungeon | 4K | 1.126 | 1.125 | 1.077 | 1.076 | 876 | 879 |
| Graveyard | 720p | 0.524 | 0.506 | 0.319 | 0.316 | 1900 | 1960 |
| Graveyard | 1080p | 0.525 | 0.508 | 0.476 | 0.477 | 1861 | 1876 |
| Graveyard | 4K | 1.537 | 1.535 | 1.486 | 1.483 | 642 | 645 |
| Encounter (10) | 720p | 0.223 | 0.209 | 0.144 | 0.142 | 4386 | 4727 |
| Encounter (10) | 1080p | 0.300 | 0.296 | 0.257 | 0.256 | 3250 | 3316 |
| Arena | 720p | 0.264 | 0.264 | 0.226 | 0.226 | 3689 | 3674 |
| Arena | 1080p | 0.472 | 0.473 | 0.433 | 0.432 | 2078 | 2082 |
| Arena | 4K | 1.589 | 1.588 | 1.538 | 1.536 | 622 | 624 |
| Arena, shadows off | 1080p | 0.222 | 0.221 | 0.184 | 0.185 | 4465 | 4429 |
| Arena, shadows off | 4K | 0.672 | 0.671 | 0.627 | 0.627 | 1465 | 1464 |
| Empty | 1080p | 0.272 | 0.271 | 0.230 | 0.231 | 3631 | 3631 |
| Env stress | 720p | 0.527 | 0.522 | 0.340 | 0.338 | 1890 | 1912 |
| Env stress | 4K | 2.321 | 2.324 | 2.259 | 2.262 | 430 | 427 |
| VFX stress | 1080p | 0.662 | 0.652 | 0.611 | 0.606 | 1489 | 1524 |
| VFX stress | 4K | 3.350 | 3.734 | 3.270 | 3.659 | 270 | 261 |

The VFX stress row at 4K is noise, not a change: its GPU p50 ranged 2.78-3.91 ms across the
*same* build's three runs (the particle pass alone 1.19-1.84 ms, depending on how many live
effects the 300-frame window caught), and the CPU-side change cannot alter the GPU work.

### 13.2 Startup, editor and allocations

| Measurement | Before | After |
| --- | ---: | ---: |
| Launch, empty scene | 945 ms | 223 ms |
| Launch, dungeon | 997 ms | 241 ms |
| Launch, graveyard | 1022 ms | 262 ms |
| Launch, editor + showcase | 1037 ms | 282 ms |
| Editor frame, CPU p50 (5 runs) | 0.417 ms | 0.340 ms |
| Allocations/frame, fight | 13.06 | 0.05 |
| Allocations/frame, any level | 3.02 | 0.02 (exit path only) |

### 13.3 Benchmark baselines for regression checks

Recorded so the next change has something to be compared against (this machine, `--hidden`,
`frame_matrix.ps1` defaults):

| Scenario (1080p) | CPU p50 | GPU p50 | 1% low | Launch |
| --- | ---: | ---: | ---: | ---: |
| Empty | 0.271 ms | 0.230 ms | 2845 | 223 ms |
| Arena | 0.482 ms | 0.438 ms | 1458 | — |
| Fight | 0.505 ms | 0.386 ms | 1199 | — |
| Dungeon | 0.381 ms | 0.339 ms | 1794 | 241 ms |
| Graveyard | 0.509 ms | 0.477 ms | 1238 | 262 ms |

`dungeon_bench` gained two scenarios with every monster posed by its own skeleton ("full,
animated", "worst, animated"): 140 and 149 us a frame headless, against 122 and 132 us with
the rigid stand-in.

## 14. Remaining bottlenecks

1. **Shadow-stage recording** on the main thread, 0.12-0.33 ms (CPU, dense levels).
2. **Mesh-pass pixel cost**, half of it shadow sampling (GPU, 1440p+).
3. **`D3D12CreateDevice`**, ~90-100 ms of a ~240 ms launch (driver).
4. **The job system's wake-up cost.** Workers sleep between passes; a split pays ~80 us before
   any work starts. This is why every in-frame parallel path loses at the game's sizes, and
   it is the precondition for parallelizing recording (1).
5. **Fill rate under the VFX stress test** at 1440p+ (particles 0.8-1.8 ms).

## 15. Recommended next steps

In order of expected value per unit of risk:

1. **Make the job system cheap to wake** — spin briefly before parking, or keep workers on a
   per-frame barrier while a frame is in flight — and re-measure `--serial-jobs` A/Bs. Every
   parallel split in the frame depends on it.
2. **Record the shadow views in parallel**, one command list per cascade/local view, submitted
   with the main list. The views are independent; the renderer would need per-thread command
   allocators and upload-ring partitions.
3. **Measure overdraw** with a `PIPELINE_STATISTICS` query around the mesh pass (pixel-shader
   invocations / screen pixels). If it is well above 1.0 with the prepass on, an EQUAL,
   no-write mesh pipeline after the prepass removes it; if not, the shadow filter is the lever.
4. **Instanced UI quads**: one 64-96-byte instance per quad instead of 288 bytes of vertices.
5. **The remaining main-thread launch items**: fold the UI atlas and font upload into the
   startup batch (removes a synchronous flush), look at the editor's 31 ms first frame and the
   graveyard's 27-35 ms map load.
6. **Exercise the untested first-use paths** in an interactive session with the run profile on
   (camera through the character, map, menus, hero swap) and pre-create any pipeline variant
   that shows up in `run created`.

---

## Appendix A — Methodology and instrumentation

**Rules followed.** Every comparison interleaves its configurations (A, B, A, B...), reports
medians of repetitions, and states the run count; whole-run distributions rather than the last
1,024 frames; run-wide stage means rather than the periodic report's one-frame sample. These
are the rules `docs/performance.md` already carries; two were added (section 5.3 and the
parallel-path rule in `docs/threading.md`).

**Added to the game** (all cheap or off by default; no measurable overhead, Appendix B):

| Instrument | Where | Cost |
| --- | --- | --- |
| p99.9 and 0.1% low in `FrameStats` | `engine-core/src/time.rs` | none |
| Run profile: whole-run CPU/GPU/per-pass distributions, hitch log, stage means, render split, resources created | `engine-app/src/run_profile.rs` | only with `--frames`; a few additions per frame |
| Camera and animation timers | `app.rs`, `extract.rs`, `monster_visual.rs` | two `Instant`s a stage |
| Seven GPU passes (shadow split in three) | `engine-renderer/src/gpu_timer.rs` | six more timestamps a frame |
| RAM and VRAM in the report | `engine-platform/src/memory.rs`, `device.rs` | two kernel calls per report |
| `alloc-count` feature + allocation tracer | `engine-app/src/alloc_count.rs` | off unless built with it |
| Startup probes for hashing and mips; texture digest | `engine-renderer/src/upload_timing.rs` | only with `startup-timing`; digest only with `MYGAME_TEXTURE_DIGEST` |

**Drivers.** `tools/bench/frame_matrix.ps1` (resolution and pacing modes) and
`tools/bench/startup_matrix.ps1`, both interleaving, both locale-proof, both writing JSON;
`dungeon_bench`'s animated scenarios.

## Appendix B — Correctness and overhead checks

* Texture digest `7d1a3a8090104f93` unchanged through every startup change.
* `every_float_in_range_encodes_as_the_formula_does`: all 1,065,353,217 floats.
* Band determinism tests: mips and BC at several band counts; UI and particle parallel paths
  still split in their tests and still match serial byte for byte.
* Palette change: dungeon and fight screenshots before/after differ by no more than the same
  binary's own run-to-run differences (25-47K pixels slightly, 60-166 by more than 8 levels,
  from animation phase following real frame time).
* `cargo test --workspace`: 3,555 passed, 0 failed. `cargo clippy --workspace --all-targets`
  clean for every changed file (one pre-existing dead-code warning under
  `--no-default-features` is unchanged from `HEAD`). The shipping build
  (`--no-default-features`) compiles.
* Instrumentation overhead, pristine `HEAD` vs `HEAD` + instrumentation, 5 interleaved runs:
  0.271 vs 0.269 ms (empty), 0.485 vs 0.482 ms (arena) CPU p50.
