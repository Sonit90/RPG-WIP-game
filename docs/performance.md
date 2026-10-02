# Performance

## Targets

| Goal | Frame rate | Frame budget @ 1920x1080 |
| --- | --- | --- |
| Minimum | 240 FPS | 4.17 ms |
| Target | 360 FPS | 2.78 ms |
| Stretch | 480 FPS | 2.08 ms |

Reference hardware: Ryzen 7 5800X, Radeon RX 6700 XT, 1920x1080.

**No frame-rate claim appears in this document unless it was measured.** Phase 0's goal
is not to reach 480 FPS; it is to build a measurement system that can be trusted.

**Startup time has its own file**: `docs/startup-performance.md` records what a launch spends
its time on, what was done about it (1.4 s to 0.35 s on the reference machine) and what is
left. This file is about the frame.

**The 2026-10 performance audit is `docs/PERFORMANCE_AUDIT.md`**: a fresh baseline on a
different machine (Ryzen 9 5900X), the instrumentation it added (run-wide p99.9 and 0.1% lows,
a hitch log, run-wide stage means, a seven-pass GPU split, RAM/VRAM, `alloc-count`), the
drivers in `tools/bench/`, and what it changed: launch ~1.0 s to ~0.24 s, the heavy fight
0.67 to 0.50 ms a frame, and steady-state allocations to ~0.02 a frame.

## Methodology

* **CPU frame time** — implemented (M3). `engine_core::time::FrameClock` takes `Instant`
  deltas around the full `process_events -> update -> render -> present` sequence, and
  each stage is timed separately so a cost can be attributed to a stage. `Instant` is
  backed by `QueryPerformanceCounter`, sub-microsecond resolution.
* **Percentiles** — implemented (M3). `FrameStatsWindow` keeps a fixed 1024-sample ring
  and sorts into a pre-allocated scratch buffer, so neither recording nor reporting
  allocates. Nearest rank, not interpolation: a reported p99 is a frame that happened.

Still planned:
* **GPU frame time** — D3D12 timestamp queries resolved into a readback buffer and read
  one or more frames later, so the CPU never blocks on the GPU to obtain them (M5).
  GPU time is never derived from wall-clock timing.

**Windowed and fullscreen are two different measurements.** Every result must say which,
and results that matter should report both — see "Milestone 17 — windowed and fullscreen"
below. A windowed swap chain is composited by DWM, which ignores `ALLOW_TEARING` and
contends for the GPU; our own GPU timestamps read up to 3.4x high because of it.

## Measurement hygiene

* **Do not drag or resize the window during a measured run.** Windows runs a modal
  message loop for the duration of a drag, so no frames are produced and the whole drag
  is recorded as one frame sample — which then distorts `max`, `p99` and the mean for the
  next 1024 frames. Use `--frames N` and leave the window alone.
* Vsync must be off (the default) or the numbers describe the display, not the engine.
* Debug builds run the D3D12 validation layer and are several times slower; only release
  numbers belong in the results table below.
* **Interleave A/B comparisons.** Alternate the configurations run by run rather than
  measuring all of one and then all of the other; machine state drifts on a scale of
  minutes and grouped runs attribute that drift to the change. This rule exists because
  ignoring it produced two retracted conclusions, in Milestones 3 and 6.
* State the number of runs beside every figure.

## Bottleneck attribution

The benchmark must make the limiting factor identifiable:

| Symptom | Likely bottleneck |
| --- | --- |
| CPU frame time >> GPU frame time | CPU: command recording or update cost |
| GPU frame time >= CPU frame time, both near frame time | GPU: fill rate / vertex / bandwidth |
| Both low, frame time high | Synchronization: fence waits, present blocking, vsync |
| CPU time scales with object count, GPU time flat | Draw-call bound (fixed by instancing) |

Milestone 8 compares N draw calls against one instanced draw to quantify the last row.

## Known per-frame allocations

None yet — there is no render loop. This table is maintained from Milestone 3 onward and
must list every allocation that occurs once per frame, with its justification.

| Site | Allocation | Justification |
| --- | --- | --- |
| `Renderer::render_clear` / `Renderer::present` | none | Command list, allocators and the `ExecuteCommandLists` argument are all built once in `Renderer::new`; barriers live on the stack |
| `Window::pump_events` | none in the common case | `EventQueue` pre-reserves 32 events and reuses the buffer; only a burst beyond that grows the `VecDeque` |
| `FrameStatsWindow::record` | none | Ring buffer and sort scratch allocated once in `new` |
| `FrameStatsWindow::stats` | none | Sorts into the pre-allocated scratch; runs on the report interval, not per frame |
| `App` stage timing | none | Plain `f64` counters in a `Copy` struct |
| Overlay geometry | none | Vertex scratch reserved once; overflow is dropped, not grown |
| Overlay vertex upload | none, and skipped entirely unless the text changed | One persistently mapped upload buffer per frame slot |
| Overlay text formatting | allocates, 4x per second | Rebuilt in place into reused `String`s, off the per-frame path |
| Stress scene geometry | none per frame | Built into a reused scratch `Vec` and uploaded to a default heap only when the level changes |

**Measured in the whole game (2026-10 audit, `--features alloc-count`)** — this table had only
ever been checked by the headless benches. The full game allocated 1 time a frame in an empty
scene, 3 in every level and 13 in a big fight; after the audit it is 0.021-0.059 a frame
everywhere (`docs/PERFORMANCE_AUDIT.md`, section 6). What remains, all of it bounded:

| Site | Allocation | Justification |
| --- | --- | --- |
| `FrameStatsWindow::stats` | was one sort buffer per refresh (stable sort) | Fixed: unstable sort into the scratch, which is identical for `f64::total_cmp` |
| `shadow::cascade::build_cascades` | was one `Vec` every frame | Fixed: a fixed array of `MAX_CASCADES` |
| `DungeonMap::push_minimap` | was two `String`s every frame | Fixed: the banner title is cached by `advance` |
| `JobSystem::map_chunks` | a `Vec` of boxed jobs per call | Only when a pass actually splits, which no scene does at the audit's thresholds |
| VFX entity pools, render-list chunks, UI chunk buffers | regrowth when a frame exceeds the previous peak | High-water marks, a few times in a run's first seconds |
| Logging, the periodic report | a few per report interval | Off the per-frame path by design |

Logging allocates while formatting its message, which is why it is banned from the frame
path. `Renderer::resize` allocates (it rebuilds the back-buffer vector), but it runs only
when the window size changes.

## Presentation model

Frame rate can only be measured above the display refresh rate if presentation does not
block on it. The swap chain is therefore flip-discard with `DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING`
when the system supports it, and presents with `Present(0, ALLOW_TEARING)` while vsync is
off. `RendererConfig::vsync` defaults to `false` for exactly this reason: a vsync-locked
swap chain measures the monitor, not the engine.

Three back buffers let the CPU run up to two frames ahead of the GPU before the pacing
fence blocks it.

## Measured results

No frame-time measurements exist yet — Milestone 3 adds the timing system and Milestone 5
adds GPU timestamps. The single figure recorded so far is a raw frame count, kept only as
evidence that presentation is uncapped:

| Date | Build | Resolution | Work | Result |
| --- | --- | --- | --- | --- |
| 2026-08-06 | debug + validation layer | 1920x1080 | clear only | 39,402 frames in ~3.63 s (~10,850 FPS) |
| 2026-08-06 | release | 1920x1080 | clear only | 60,000 frames in 65.154 s (921 FPS avg) |

Hardware: Ryzen 7 5800X, Radeon RX 6700 XT (12242 MiB), feature level 12_1, tearing
supported. No GPU time is included — none is measured until Milestone 5.

### M3 run, in detail

```
cargo run --release --bin mygame -- --frames 60000 --report 1
```

| Metric | Value |
| --- | --- |
| Mean CPU frame | ~1.20 ms (~830 FPS) |
| p50 | 0.076 ms (~13,000 FPS) |
| p95 | 7–10 ms |
| p99 | 16–19 ms |
| min / max | 0.050 ms / ~24 ms |
| Stage means | events 0.000 ms, update 0.000 ms, render ~1.15 ms, present ~0.053 ms |

> **This run did not reproduce and its conclusion has been retracted — see "Retraction"
> below.** It is kept because the retraction is the useful part.

The distribution is **bimodal**: most frames are essentially free, and a minority cost
whole display refresh periods.

The refresh period was measured directly rather than assumed, by running with `--vsync`:

| Date | Build | Mode | Result |
| --- | --- | --- | --- |
| 2026-08-06 | release | `--vsync`, 600 frames | mean 4.206 ms, p50 4.166 ms (238 FPS) |

So the panel is 240 Hz and one refresh is 4.166 ms. Every stall in the uncapped run is an
integer multiple of it: p95 ≈ 2 refreshes, p99 ≈ 4, max ≈ 6.

Attribution, using the table above: CPU work per frame is 0.076 ms at the median and the
`present` call itself is 0.05 ms, so neither the CPU nor the submission path is the
limiter. The cost lands in the per-slot fence wait inside `render`, waiting for a back
buffer to come free. A windowed swap chain under DWM composition returns buffers on the
compositor's schedule, so `ALLOW_TEARING` does not make windowed presentation uncapped.

### Retraction (2026-08-06, Milestone 5)

The paragraph above concluded that windowed DWM composition imposes a synchronization
bottleneck. **That conclusion is withdrawn.** The same command on the Milestone 5 build
produces:

| Run | Frames | Wall clock | Average | CPU p99 |
| --- | --- | --- | --- | --- |
| 1 | 60,000 | 10.25 s | 5856 FPS | 0.684 ms |
| 2 | 60,000 | 10.38 s | 5782 FPS | 0.632 ms |
| 3 | 60,000 | 10.31 s | 5821 FPS | 0.664 ms |
| 4 | 150,000 | 26.14 s | 5739 FPS | 0.458 ms |

No refresh-quantized stalls appear in any of them. It was also checked explicitly that
bringing the window to the foreground changes nothing, so occlusion is not the difference
either. The original 921 FPS run is most plausibly explained by other work contending for
the GPU or compositor while it ran.

What this costs: an attribution table entry that looked authoritative was built on **one**
run. A single run is not a measurement. Every figure in this document from here on states
its run count, and conclusions about bottlenecks require repeated runs that agree.

## Milestone 5 — CPU and GPU frame time

GPU time now comes from D3D12 timestamp queries, never from wall clock. Release build,
1920x1080, clear + one triangle, four runs as tabulated above; representative figures:

| Metric | Value |
| --- | --- |
| CPU frame mean | 0.166–0.184 ms (~5500–6100 FPS) |
| CPU p50 / p95 / p99 | 0.149 / 0.234 / 0.63 ms |
| GPU frame mean | 0.044–0.051 ms |
| GPU p50 / p95 / p99 | 0.044 / 0.048 / 0.052 ms |
| Stage means | events 0.001, update 0.000, render 0.035, present 0.153 ms |
| Timestamp frequency | 100,000,000 ticks/s |

### What this says about the bottleneck

Reading the attribution table above: CPU frame time (0.166 ms) is roughly **3.7x** the GPU
frame time (0.045 ms), so the GPU is idle most of the frame and the workload is CPU-bound
— which is expected when the entire scene is one triangle.

More precisely, the cost is inside `present`: 0.153 ms of the 0.166 ms CPU frame, against
0.035 ms to record and submit the whole command list. The limiter at this scene size is
the `Present` call itself, not rendering. That is the number Milestones 7 and 8 have to
push against: until the scene is heavy enough to exceed ~0.15 ms of GPU work, added
geometry will be nearly free and any FPS comparison will really be measuring `Present`.

GPU frame times are stale by up to `FRAME_COUNT` (3) frames by design — see
`crates/engine-renderer/src/gpu_timer.rs` for why reading them sooner would require
blocking on the GPU and would change the measurement.

## Milestone 6 — the debug overlay costs nothing measurable

Four **interleaved** pairs of 20,000-frame runs, release, 1920x1080. Interleaved means the
two configurations alternate, so drift in machine state hits both equally:

| Configuration | FPS (4 runs) | mean FPS | CPU mean | GPU mean |
| --- | --- | --- | --- | --- |
| Overlay hidden | 5240 / 5363 / 5433 / 5515 | 5388 | 0.193 ms | 0.049 ms |
| Overlay visible | 5438 / 5307 / 5425 / 5545 | 5429 | 0.185 ms | 0.0495 ms |

The difference is inside the noise floor of about ±5%; the visible configuration even
measured nominally faster. ~2,400 extra tiny triangles are negligible for this GPU. The
overlay is confirmed to be drawing during those runs (`draws 2 | tris 2381`).

### Retraction, and a rule

The previous version of this section reported the overlay costing ~17% of frame rate. That
came from three runs with the overlay on followed by three with it off — **grouped, not
interleaved** — so a slow patch on the machine fell entirely into one group. Same mistake
as the Milestone 3 retraction, one milestone later.

**Rule from now on: any A/B performance comparison must interleave its configurations.**
Grouped runs measure the machine's mood as much as the change.

The upload-skipping change made for the same wrong reason is kept only as waste avoidance
(see the allocation table), and is credited with no performance improvement.

## Milestone 7 — triangle count is not the limit

Release, 1920x1080, overlay hidden, 15,000 frames per run, **three interleaved rounds**
(every level once per round):

| Triangles | mean FPS | CPU mean | GPU mean | render | present | Draw calls |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 5557 | 0.180 ms | 0.052 ms | 0.035 ms | 0.147 ms | 1 |
| 100 | 5656 | 0.173 ms | 0.050 ms | — | — | 1 |
| 1,000 | 5586 | 0.177 ms | 0.052 ms | — | — | 1 |
| 10,000 | 5567 | 0.177 ms | 0.055 ms | 0.034 ms | 0.142 ms | 1 |
| 100,000 | 6831 | 0.143 ms | 0.077 ms | 0.032 ms | 0.107 ms | 1 |

Applying the attribution table at the top of this document:

* **Not GPU-bound.** GPU time grows only 0.052 → 0.077 ms across a 100,000x increase in
  triangles. At the top level that is ~1.3 billion triangles/second.
* **Not draw-call-bound.** There is one draw call at every level, and CPU recording time
  is flat at ~0.034 ms. Milestone 8 is what moves this axis.
* **Presentation-bound.** `present` costs 0.107–0.147 ms, three to four times the cost of
  recording the entire frame.

### An anomaly, left unexplained

At 100,000 triangles `present` becomes *cheaper* (0.107 ms vs 0.147 ms), so the heaviest
scene reaches the highest frame rate. This reproduced in all three interleaved rounds, so
it is not drift. No mechanism is claimed — nothing measured here tests one.

Practical consequence: **frame rate through a composed window is not a clean measure of
renderer cost.** GPU frame time and CPU recording time are the numbers to trust, and the
benchmark executable should report those rather than leaning on FPS.

## Milestone 8 — what a draw call costs

`cargo run --release --bin performance_test`. Offscreen (no window, no swap chain, no
present), 1920x1080, 300 measured frames plus 30 warm-up per configuration, modes
interleaved per instance count, **three full runs**:

| Instances | A CPU | B CPU | A GPU | B GPU | CPU ratio |
| --- | --- | --- | --- | --- | --- |
| 1 | 0.016–0.018 ms | 0.016–0.018 ms | 0.034 ms | 0.034–0.035 ms | ~1.0x |
| 100 | 0.017–0.021 ms | 0.015–0.018 ms | 0.040 ms | 0.040 ms | 1.0–1.2x |
| 1,000 | 0.040–0.043 ms | 0.015–0.020 ms | 0.075 ms | 0.054–0.058 ms | 2.1–2.6x |
| 10,000 | 0.252–0.264 ms | 0.018–0.023 ms | 0.415–0.421 ms | 0.080–0.085 ms | 11.5–15.1x |
| 100,000 | 2.654–2.702 ms | 0.014–0.019 ms | 3.98–4.11 ms | 0.076–0.141 ms | 141–188x |

A = one draw call per instance, B = one instanced draw. Both produce byte-identical
images, asserted by a pixel-readback test.

**A draw call costs roughly 27 ns of CPU and 41 ns of GPU** on this machine. Instancing's
cost is flat in the object count on the CPU.

Against the frame budgets at the top of this document: at 240 FPS (4.17 ms) drawing
100,000 objects one call at a time spends 2.65 ms — 64% of the entire frame — on
submission alone, before any simulation. The same picture instanced costs 0.014 ms.

The crossover is around 1,000 objects; below a few hundred the two are indistinguishable,
so batching is a scaling decision, not a micro-optimization.

# Phase 1 — engine core

## CPU subsystem baselines

`cargo run --release --bin cpu_bench`. Median of 9 samples, 100,000 iterations per sample.
Same machine as the Phase 0 tables; the numbers are meaningless across machines.

### Allocators

| Operation | Cost |
| --- | --- |
| `Arena::alloc<u64>` | 1.49 ns |
| `Arena::alloc_slice(256 B)` | 5.82 ns |
| Frame allocator: allocate + reset | 0.42 ns |
| `Box::new(u64)`, for scale | 34.97 ns |

Bump allocation is ~23x cheaper than the system allocator for a scalar, and the per-frame
reset is effectively free — it is one store, and the measurement is dominated by the
allocation next to it. The 256-byte slice figure is `memcpy` cost, not allocator cost.

### Input

| Operation | Cost |
| --- | --- |
| `begin_frame` + 4 events | 1.95 ns |
| Two state queries | 0.69 ns |

A whole frame of input handling is under 2 ns, so the transition bookkeeping that lets
gameplay ask "pressed this frame?" costs nothing worth reclaiming.

### Profiler

| Build | Cost per scope |
| --- | --- |
| Release (instrumentation compiled out) | 2.57 ns |
| Release + `engine-core/profiling` | 29.16 ns |
| Recording forced off | 2.58 ns |

The release figure is the measurement harness, not the profiler: an inert scope and a
compiled-out scope cost the same. Recording costs ~29 ns per scope, which is why
instrumentation is opt-in for a release build rather than always on — 100 scopes a frame
would be 2.9 us, about 0.7% of a 4.17 ms frame at 240 FPS.

### Job system

1000 jobs of a synthetic integer workload. Full table and analysis in `docs/threading.md`.

| Work per job | Sequential | 1 worker | 11 workers | 23 workers |
| --- | --- | --- | --- | --- |
| ~0.1 us | 0.105 ms | 0.171 ms (0.61x) | 0.487 ms (0.22x) | 0.449 ms (0.23x) |
| ~12 us | 12.48 ms | 12.46 ms (1.00x) | 1.29 ms (9.64x) | 0.72 ms (17.25x) |

| Measurement | Cost |
| --- | --- |
| submit + wait, one empty job | ~3.3 us |
| submit + wait, 1000 empty jobs | ~112 ns per job |

**Small jobs are slower in parallel.** At ~0.1 us of work per job the pool is 4x slower
than a sequential loop, and adding workers makes it worse — the shared queue is contended
by threads that finish faster than they can be fed. Parallelize by chunking, and measure
before moving anything onto jobs.

## Engine loop overhead

The Phase 1 loop adds per frame, to what Phase 0 did: one `Time::advance`, one metrics
record, one input `begin_frame`, one frame-allocator reset, and four profiler scopes
(inert in release). Measured end-to-end at the same workload as the Phase 0 regression
runs, 60,000 frames at 1000 triangles:

| Build | CPU mean | Phase 0 band |
| --- | --- | --- |
| Phase 1 release | 0.170–0.175 ms | 0.166–0.184 ms |

Within the Phase 0 band, so the added per-frame work is below the run-to-run noise of the
measurement. This is checked after every milestone rather than at the end.

# Phase 2 — renderer foundation

## Milestone 2 — is the frame CPU-bound or GPU-bound?

Release build, 1920x1080, 20,000 frames, three frames in flight. "Stalled" counts frames
where the CPU's pacing wait actually had to block on the fence.

| Scene | CPU mean | GPU mean | Stalled frames | Max stall |
| --- | --- | --- | --- | --- |
| 1,000 triangles | 0.167 ms | 0.051 ms | 3 / 20,000 (0.02%) | 1.83 ms |
| 100,000 triangles | 0.129 ms | 0.069 ms | 3 / 20,000 (0.02%) | 2.06 ms |

**The renderer is CPU-bound.** The GPU finishes in roughly a third of the time the CPU
spends preparing and submitting, and the CPU almost never waits for it. The handful of
stalls are startup artefacts — the first frames of each slot, plus one long outlier that
matches a driver or scheduler hiccup rather than steady-state pacing.

Consequences for the rest of Phase 2:

* Work that reduces **CPU submission cost** — batching, instancing, fewer descriptor and
  pipeline changes — moves the frame time.
* Work that reduces **GPU cost** will not, until the GPU workload grows past the CPU's.
  Depth, lighting and thousands of objects will change that, and this measurement should be
  repeated when they land.
* The three frames in flight are currently doing nothing for throughput, because the CPU
  is never the one waiting. They cost nothing and become load-bearing the moment the GPU
  side grows.

Counter-intuitive result worth recording: a **single** frame in flight also shows zero
stalls when the GPU work is trivial. An empty command list completes before the CPU can get
back around the loop. What serializes CPU and GPU is one slot *plus* GPU work that outlasts
a CPU iteration — verified in `frame_sync.rs` by giving the GPU 64 MiB buffer copies, which
does produce a stall on every frame after the first.

## Methodology addition — comparing two builds under background load

Milestone 3 was first measured as a 47% regression (0.248 ms against a 0.168 ms baseline).
It was not one. The two numbers came from different sessions, and the machine had picked up
background load in between: an IDE holding 2.6 GB and the GPU vendor's service.

What the same change looked like under four methods:

| Method | Before | After | Verdict |
| --- | --- | --- | --- |
| Single run, mean, compared against an earlier session | 0.168 ms | 0.248 ms | "47% regression" |
| 3 interleaved runs, mean | 0.229-0.309 ms | 0.219-0.332 ms | indistinguishable |
| 5 interleaved runs, median of means | 0.223 ms | 0.243 ms | overlapping, unresolved |
| 21 reporting intervals, median p50 | 0.155 ms | 0.155 ms | no change |

Two rules follow, and they now apply to every comparison in this document:

1. **Interleave.** Run A and B alternately in one session. This was already the rule after
   phase 0 retracted two conclusions for the same reason.
2. **Report a spike-robust statistic.** Interleaving is not sufficient. The mean is the
   wrong estimator under interference: a handful of multi-millisecond scheduler spikes moves
   it substantially while leaving the frame's actual cost untouched. Use the median of the
   per-interval p50 values, which the running engine already reports.

A corollary worth stating: absolute numbers are only comparable within one session on one
machine state. The 0.155 ms p50 measured here and the 0.168 ms mean measured in milestone 2
are not in conflict - they are different statistics of different runs under different load.

## Milestone 4 - descriptor allocation

`cargo run --release --bin cpu_bench`, median of 9 samples.

| Operation | Cost |
| --- | --- |
| Per-frame table, 1 descriptor (bump) | 2.85 ns |
| Per-frame table, 4 descriptors (bump) | 2.86 ns |
| Frame region reset | 0.44 ns |
| Free-list allocate + free (persistent) | 3.54 ns |

Table size does not affect the cost: a bump is a bump whether it hands out one descriptor
or four. Reclaiming a whole frame's descriptors is one store regardless of how many were
allocated.

Against the frame budget, for the case that scales with scene size:

| Visible objects, one table each | Descriptor cost | Share of a 2.08 ms frame |
| --- | --- | --- |
| 1,000 | 2.9 us | 0.14% |
| 10,000 | 28.5 us | 1.4% |
| 100,000 | 285 us | 13.7% |

At 10,000 objects descriptor allocation is not the bottleneck. At 100,000 individually
bound objects it starts to matter - which is another argument for instancing, where one
table serves the whole batch, rather than for a faster descriptor allocator.

## Milestone 6 - upload system

Release build, 1000-triangle stress scene, 20,000 frames, three reporting intervals.

| Statistic | Value |
| --- | --- |
| CPU frame, median of per-interval p50 | 0.133 ms |
| GPU frame, median of per-interval p50 | 0.034 ms |

No regression against the 0.152 ms baseline from milestone 5. The upload ring is created
and reset once per frame but nothing writes to it yet, so the only frame-path change is one
store per slot; the difference above is machine state, not the change.

The batching claim is measured by test rather than by benchmark: `batched_uploads_cost_one_synchronization` asserts that eight buffer uploads produce `total_copies == 8` and
`total_flushes == 1`. The old per-mesh path would have produced eight of each, and each
flush is a full GPU round trip.

## Milestone 9 - transforms

`cargo run --release --bin cpu_bench`, median of 9 samples.

| Operation | Cost |
| --- | --- |
| `Transform::matrix` | 3.42 ns |
| `write_matrices`, per transform over 100,000 | 2.65 ns |
| `Transform::then` (compose two transforms) | 12.05 ns |

The batched form is 22% cheaper per transform than the individual call, measured over an
array far larger than L2: two contiguous streams, one pass, no allocation.

| Visible objects | Matrix generation | Share of a 2.08 ms frame |
| --- | --- | --- |
| 1,000 | 2.7 us | 0.13% |
| 10,000 | 26.5 us | 1.3% |
| 100,000 | 265 us | 12.7% |

`docs/threading.md` puts the job-system break-even at roughly 12 us of work per job, so a
culling or transform chunk has to be about 4,500 transforms before submitting it pays for
itself. At 10,000 objects the whole pass is 26 us - two chunks - which is not obviously
worth the submission overhead. Chunking stays a call-site change rather than a rewrite.

## Milestone 12 - the depth buffer's cost

Release build, 1000-triangle stress scene, 1920x1080, three reporting intervals, median of
per-interval p50.

| | Before (M11) | After (M12) | Change |
| --- | --- | --- | --- |
| CPU frame | 0.144 ms | 0.161 ms | +0.017 ms |
| GPU frame | 0.034 ms | 0.054 ms | +0.020 ms |

A cost, not a regression: a full 1920x1080 depth clear plus per-fragment depth testing is
work that did not exist before. 20 us of GPU time is 1% of the 2.08 ms stretch budget.

The CPU delta is one extra ClearDepthStencilView and one extra bind per frame. 17 us is more
than those two calls should cost, so some of it is machine state; it is recorded as measured
rather than explained away.

## Milestone 15 - frustum culling

`cargo run --release --bin cpu_bench`, median of 9 samples. Roughly half the scene is outside
the view, so both the accept and the reject path are exercised.

| Scene | Per object | Whole pass | Share of a 2.08 ms frame |
| --- | --- | --- | --- |
| 1,000 | 3.59 ns | 3.6 us | 0.17% |
| 10,000 | 3.69 ns | 36.9 us | 1.8% |
| 100,000 | 3.69 ns | 369 us | 17.7% |

Flat per object, which is what a linear pass over two contiguous arrays should be.

Against the 12 us per-job break-even from docs/threading.md, a 10,000-object cull is three
chunks - three submissions to save at most 25 us, which is not worth it. At 100,000 objects
it becomes worth doing. The loop reads slices, so chunking is a call-site change.

## Milestone 17 — windowed and fullscreen are two different measurements

Every measurement in this document before this section was taken in a **window**. That was
never stated, because until it was measured nobody suspected it mattered. It matters a lot.

A windowed swap chain is composited by the Desktop Window Manager. `DXGI_PRESENT_ALLOW_TEARING`
is ignored under composition, DWM has to blit a 1920×1080 client area onto whatever the
desktop actually is, and its work contends for the same GPU. A borderless fullscreen swap
chain can take the independent-flip path and skip all of that.

Same scene, same build, same machine, 10,000 objects and 100,000 objects:

| Scene | Mode | CPU | GPU | Present | FPS |
| --- | --- | --- | --- | --- | --- |
| 10,000 | windowed | 0.846 ms | 0.400 ms | 0.577 ms | 1182 |
| 10,000 | fullscreen | 0.515 ms | 0.224 ms | 0.132 ms | 1942 |
| 100,000 | windowed | 3.877 ms | 2.308 ms | 1.271 ms | 258 |
| 100,000 | fullscreen | 3.101 ms | 0.686 ms | 0.246 ms | 322 |

**Our own GPU timestamps read 3.4× high at 100,000 objects when windowed.** The timestamps
are correct — they measure how long our command list took to execute — but a contended GPU
executes it slower, so the number describes the machine's total load rather than the
renderer's cost.

### The rule from here

**Report both.** Neither column is the truth on its own:

* **Fullscreen** is the renderer's cost. It is what an optimization has to move, and it is
  what the frame budgets in this document should be compared against.
* **Windowed** is what a windowed game actually delivers on a desktop with other things on
  it. It is the number a player would see, and it is the development default.

A single-column result is ambiguous and should not be recorded. Where a table below this
section has one column, it is fullscreen unless it says otherwise.

Earlier sections keep their windowed numbers and are **not** re-measured. They are a log of
what each milestone found, their relative conclusions all still hold — a comparison between
two builds measured the same way is unaffected by a constant factor — and re-running fifteen
milestones' tables would produce different numbers with the same meanings.

### What Present actually is

Present had never been attributed. It is not the renderer:

* It does **not** scale with draw calls. 100,001 draws present *faster* (0.752 ms) than
  17,299 (1.233 ms), because the slower CPU frame gives the GPU time to drain.
* It does **not** scale with resolution. 1920×1080 to 480×270 changes it by nothing, and
  changes GPU time by nothing either — this renderer is geometry and submission bound, not
  pixel bound.
* It does **not** move with frames in flight. 1, 2 and 3 all present in 0.56–0.61 ms, while
  the *fence* stall behaves exactly as designed: 0% at 2 and 3 frames, 15% at 10,000 objects
  and 81% at 100,000 with 1.

It is where the CPU waits for the display pipeline, and windowed that pipeline includes DWM.
The frame ring's stall counter cannot see it — that measures the fence at the top of the
*next* frame, a different queue — which is why the Milestone 15 audit concluded the engine
was CPU-bound with 0.1% stalls. That conclusion was drawn from the wrong queue.

There is also a **~2000 FPS ceiling** in fullscreen and ~1200 windowed, enforced by Present
regardless of workload: at 10 objects and at 10,000 the frame lands at almost exactly the
same 0.50 ms. Any result near those numbers is measuring the swap chain, not the engine.
Well clear of the 480 FPS stretch target, but it invalidates comparisons taken up there.

## Milestone 17 — render submission profile

Counters for every driver call the recording loop makes, at 100,000 objects / 17,298 visible:

| Counter | Value |
| --- | --- |
| Binding calls | 86,495 |
| PSO changes | 2 (+0 redundant) |
| Material changes | 16,273 |
| Mesh changes | 12,606 |
| Root CBV binds | 34,597 (+1,025 redundant) |
| Vertex buffer binds | 17,299 (+4,693 redundant) |
| Index buffer binds | 17,298 (+4,693 redundant) |
| Redundant overall | 15.0% |

**PSO changes are already 2, not 17,299.** There is one mesh pipeline and one overlay
pipeline, and the command context has eliminated redundant pipeline binds since Milestone 3.
Sorting by pipeline can win nothing at the current material model — which is worth knowing
before building a sort key with twelve bits reserved for it.

Material and mesh changes are near the draw count against **9 materials and 4 meshes**, so
that is where the headroom is. What a material change costs today is one
`SetGraphicsRootConstantBufferView` — roughly 18 ns — so removing 16,264 of them is worth
about 0.3 ms of the 1.7 ms recording time. Real, and much larger later: when a material
gains a texture, a material change becomes a descriptor table bind.

## Milestone 17 — parallel culling

`Culler::cull_parallel` chunks the world across the job system, one `RenderList` per chunk,
merged in chunk order. The merged list is identical to the serial one — asserted at 1, 2, 4,
8 and 16 workers.

Chunks are at least 4,096 slots. `docs/threading.md` puts the job break-even near 12 µs and
a cull-and-gather pass measures ~6 ns per object, so a few thousand objects is the smallest
chunk worth handing to a worker; below that the pass stays serial.

Alternating runs, 1920×1080:

| Scene | Mode | Cull | CPU frame | FPS |
| --- | --- | --- | --- | --- |
| 100,000, windowed | serial | 1.198 ms | 3.877 ms | 258 |
| 100,000, windowed | parallel | 0.218 ms | 2.619 ms | 382 |
| 100,000, fullscreen | serial | 1.203 ms | 3.101 ms | 322 |
| 100,000, fullscreen | parallel | 0.209 ms | 2.108 ms | 474 |
| 10,000, windowed | serial | 0.082 ms | 0.846 ms | 1182 |
| 10,000, windowed | parallel | 0.055 ms | 0.821 ms | 1218 |
| 10,000, fullscreen | serial | 0.082 ms | 0.515 ms | 1942 |
| 10,000, fullscreen | parallel | 0.064 ms | 0.511 ms | 1957 |

**5.5× on the cull pass and 1.47× on the whole frame at 100,000 objects**, which is 322 to
474 FPS fullscreen — across the 480 FPS stretch target's threshold within measurement error.

At 10,000 objects it is within noise, exactly as the break-even predicts: 1,772 visible is a
0.08 ms pass and there is nothing there to win. The threshold keeps it serial rather than
paying submission overhead for no gain, which is the whole reason the threshold exists.

The per-object cull cost also rose from 3.7 ns (Milestone 15) to 6.0 ns, measured by
`cpu_bench`. That is not a regression: since Milestone 16 the pass also gathers each
survivor into the render list, which is work the recording loop used to do.

## Phase 2 final baseline

Defaults as shipped: instancing on, parallel culling on, vsync off, release, 1920×1080.
Both columns, per the rule above.

| Objects | Visible | Draws | Windowed CPU | Windowed FPS | Fullscreen CPU | Fullscreen FPS |
| --- | --- | --- | --- | --- | --- | --- |
| 10 | 10 | 11 | 0.163 ms | 6135 | 0.514 ms | 1946 |
| 1,000 | 252 | 26 | 0.196 ms | 5102 | 0.502 ms | 1992 |
| 10,000 | 1,772 | 26 | 0.401 ms | 2494 | 0.516 ms | 1938 |
| 100,000 | 17,298 | 26 | 0.888 ms | 1126 | 0.692 ms | 1445 |

Read these against the ~2000 FPS fullscreen and ~1200 windowed swap-chain ceilings, not as
engine measurements: everything except the 100,000-object row is sitting on one. The
windowed column beating the fullscreen one at low object counts is the same artefact from
the other side — a tiny frame spends proportionally more of itself waiting in `Present`, and
the two modes wait differently.

**The only row that measures the renderer is the last one**, and it is 1445–1460 FPS against
a 480 FPS stretch target.

Against the Phase 2 frame budgets:

| Goal | Budget | 100,000 objects, fullscreen | Headroom |
| --- | --- | --- | --- |
| Minimum, 240 FPS | 4.17 ms | 0.69 ms | 6.0× |
| Target, 360 FPS | 2.78 ms | 0.69 ms | 4.0× |
| Stretch, 480 FPS | 2.08 ms | 0.69 ms | 3.0× |

And against the architectural target of "CPU renderer overhead < 1 ms, GPU < 1 ms for a
simple scene": 0.69 ms CPU and 0.49 ms GPU, for a scene that is not simple.

---

# Phase 3 — gameplay and combat

## Methodology

Two different measurements, kept apart on purpose:

* **`cargo run --release --bin game_bench`** measures the gameplay pipeline with no device:
  the simulation tick, the combat share of it, render extraction and frustum culling. Per
  *tick*, not per frame — a 60 Hz tick happens roughly once every five frames at 300 FPS.
* **`mygame --dummies <n> --frames 3000 --report 0`** measures the whole frame, including
  presentation. FPS and GPU time are properties of a frame, and mixing the two would measure
  the swap chain.

Both play a fixed rotation, so two runs are the same workload rather than two different
ones. Reference machine, release, vsync off, 1920×1080 windowed.

## Milestone 24 — the gameplay pipeline, per tick

3,000 measured ticks per scenario after 600 warm-up ticks.

| Dummies | Entities | Sim mean | of which combat | Extraction | Culling | Sim p99 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 3 | 0.40 µs | 0.10 µs | 0.12 µs | 0.07 µs | 0.5 µs |
| 10 | 12 | 0.55 µs | 0.13 µs | 0.29 µs | 0.14 µs | 0.8 µs |
| 100 | 102 | 2.24 µs | 0.51 µs | 2.16 µs | 0.84 µs | 5.0 µs |
| 500 | 502 | 9.71 µs | 2.18 µs | 10.35 µs | 3.43 µs | 24.0 µs |
| 1,000 | 1,002 | 19.22 µs | 4.30 µs | 20.80 µs | 5.99 µs | 49.0 µs |

Linear in entity count, which is what a pass over parallel arrays should be.

## Milestone 24 — the whole frame

3,000 frames per scenario.

| Dummies | CPU frame mean | CPU p99 | GPU frame mean | FPS |
| --- | --- | --- | --- | --- |
| 1 | 0.208 ms | 0.728 ms | 0.076 ms | 4753 |
| 10 | 0.203 ms | 0.645 ms | 0.074 ms | 4427 |
| 100 | 0.214 ms | 0.666 ms | 0.079 ms | 4654 |
| 500 | 0.264 ms | 0.805 ms | 0.116 ms | 3769 |
| 1,000 | 0.340 ms | 1.156 ms | 0.150 ms | 2941 |

At 1,000 entities the simulation costs about 4 µs of an average 300 FPS frame: **0.06% of
the 480 FPS stretch budget**. Gameplay is not the bottleneck and is not close to being one.

## What the measurement found

The first targeting implementation ranked every hostile by distance with an insertion sort,
and `nearest_hostile` — which runs every tick to keep a valid target selected — read one
element off the front of that sort.

| 1,000 dummies | Sim mean | Sim p99 |
| --- | --- | --- |
| Insertion sort, ranked every tick | 39.06 µs | 398.5 µs |
| Single scan for nearest, `sort_unstable_by` for the ranking | 19.22 µs | 49.0 µs |

**2× on the mean and 8× on the p99.** A 398 µs spike every fifteen ticks would have been
invisible in an average and clearly visible on screen. This is the entire reason the
benchmark exists, and it is the only optimization Phase 3 made.

## Milestone 25 — allocation

`game_bench` counts every process allocation with a counting global allocator, over the
measured ticks only.

| Dummies | Allocations over 3,000 ticks | Per tick |
| --- | --- | --- |
| 1 | 0 | 0.0000 |
| 10 | 2 | 0.0007 |
| 100 | 9 | 0.0030 |
| 500 | 8 | 0.0027 |
| 1,000 | 8 | 0.0027 |

The residue is amortized growth rather than per-frame work: the damage meter appends a row
the first time each distinct target is hit, and the target cycle keeps reaching new dummies
for tens of seconds in the larger scenarios. The scenario where every target has been seen
during warm-up allocates **exactly zero** over 3,000 ticks.

Known per-frame allocations added by Phase 3: none. The event stream, command buffer,
ability hit list, status outcome list, target scratch, meter rows, extractor slot map and
overlay strings are all cleared rather than freed, and each has a test asserting its
capacity does not move in a steady state.

No custom allocator and no object pool: 0.003 allocations per tick justifies neither.

## Phase 2 regression check

| Measurement | Phase 2 baseline | Phase 3 |
| --- | --- | --- |
| `cull_bench`, 100,000 objects, 50% visible | ~6.0 ns/object | 5.54 ns/object, 0 allocations |
| Showcase, 10,000 objects, windowed | 0.401–0.514 ms CPU | 0.469 ms CPU, 2132 FPS |

No rendering regression. The only change to shared code is the additive
`engine_core::time::FixedTimestep`.

## Addendum — enemy nameplates

Text is one quad per lit font cell, so a nameplate costs roughly 330 quads and the total is
linear in the number of plates. Release, 1920×1080 windowed, 6,000 frames, mean CPU frame:

| Dummies | No nameplates | Uncapped | Capped at 20 nearest |
| --- | --- | --- | --- |
| 1 | 0.208 ms | 0.213 ms | 0.228 ms |
| 10 | 0.203 ms | 0.284 ms | 0.291 ms |
| 100 | 0.214 ms | 0.650 ms | 0.411 ms |
| 500 | 0.264 ms | — | 0.495 ms |

Uncapped, a hundred enemies cost 0.44 ms a frame: 1515 FPS against 4654 for the same scene
without plates. Capping at the twenty nearest stops the cost growing with the pull — 500
enemies cost what 100 do — which is why the cap exists rather than being a preference.

The remaining ~0.2 ms for twenty plates is the approach: 20 plates × ~22 characters × ~15 lit
cells is about 1.1 MB of vertices uploaded per frame. A font atlas with instanced glyph quads
would remove it, and that is a renderer change rather than a nameplate one.

The plates are a **second overlay layer**, rebuilt every frame and uploaded unconditionally,
while the developer HUD keeps its per-slot cache. Merging them would re-upload the HUD's tens
of thousands of quads every frame to move a plate by a pixel.


## Phase 2 (environment) — the combat arena

Release, 1920x1080 windowed, no vsync, Ryzen 5800X / RX 6700 XT. Figures from the game's own
periodic report over 12,000-40,000 frame runs.

| Scenario | CPU mean | CPU p99 | GPU mean | FPS | Draws | Triangles | Visible |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Arena, no dummies | 0.172 ms | 0.31 ms | 0.076 ms | 5,808 | 31 | 2,596 | 85 |
| + 10 dummies | 0.203 ms | 0.42 ms | 0.092 ms | 4,936 | 33 | 3,316 | 105 |
| + 50 dummies | 0.299 ms | 0.46 ms | 0.149 ms | 3,341 | 33 | 5,548 | 185 |
| + 100 dummies | 0.318 ms | 0.71 ms | 0.156 ms | 3,147 | 33 | 7,948 | 272 |
| Stress x4 (571 objects) | 0.385 ms | 1.24 ms | 0.187 ms | 2,599 | 34 | 10,084 | 400 |
| Stress x8 (1,071 objects) | 0.416 ms | 1.22 ms | 0.203 ms | 2,402 | 34 | 13,240 | 582 |

The 240 FPS floor is 4.167 ms and the 480 FPS aspiration is 2.083 ms. The worst shipping case
is 0.318 ms of CPU at a 0.71 ms p99, which meets both with over six times the headroom.

**Draw calls do not scale with object count.** 5.5x the objects costs one extra draw, because
every placement of an asset shares the mesh and material handles of a single upload and the
render list's counting sort collapses them into one instanced draw.

### Per-pass cost, headless (`env_bench`, 3,000 frames)

| Scenario | LOD us | Extract us | Cull us | Group us | Total us | Allocs/frame |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Empty arena | 2.39 | 0.03 | 1.17 | 1.56 | 5.15 | 0.0003 |
| + 100 dummies | 2.43 | 7.09 | 3.19 | 2.07 | 14.77 | 0.229 |
| Worst view | 2.29 | 5.68 | 3.58 | 2.12 | 13.67 | 0 |
| Stress x4 | 7.41 | 0.67 | 5.23 | 2.51 | 15.82 | 0 |

The environment's own per-frame cost is **3.6 us** — 2.4 us of level selection plus 1.2 us of
culling — which is 0.09% of a 240 FPS frame. The residual allocations in the dummy-heavy rows
are the Phase 3 damage-meter residue documented above, not environment work.

### Memory

| | |
| --- | --- |
| Environment textures | 932 KiB (18 uploads of 9 distinct 128^2 and 64^2 images) |
| Environment geometry | 922 triangles at LOD0 across 16 models |
| Render entities added | 240 for the combat arena, 1,353 at stress x8 |

### One allocation removed

`RenderList::group_by_material_and_mesh` copied its bucket cursors with `to_vec()` every
frame. With nine materials and four meshes nobody noticed; with the environment's forty of
each, `env_bench` measured **1.18 allocations per frame**. The cursors now live in a reused
field, and the empty-arena scenario allocates 0.0003 per frame.


# Phase 3 (VFX) — combat feedback

`cargo run --release --bin vfx_bench` measures the CPU side with no device; the shipped
binary with `--vfx-stress <rate>` measures the whole frame including the GPU. The stress
driver plays catalogue effects around the player **without touching gameplay**, so the
difference between two runs is the cost of drawing effects rather than the throughput of the
ability system.

## What effects cost the CPU

Microseconds per frame, mean over 600 measured frames after 300 of warm-up.

| Fight | without effects | with effects | delta |
| --- | ---: | ---: | ---: |
| 1 target | 0.8 | 2.2 | **+1.4** |
| 50 targets, area rotation | 5.6 | 7.3 | **+1.7** |
| 100 targets | — | 11.9 | — |
| Ultimate on 50 (2,100 particles) | — | 19.6 | — |

Event observation does not appear: it is 0.0 us to one decimal in every scenario, being a
pass over a slice of a few dozen events. The VFX update itself — instances, emitters,
particle integration, mesh-draw rebuild, render-entity sync — is 1.3-1.6 us in ordinary
combat and 13.6 us while the ultimate's 2,100 particles are alive.

## The whole frame, on the GPU

1920x1080, vsync off, 6,000 frames per configuration.

| Scenario | CPU mean | GPU mean | FPS | Draw calls | Particles |
| --- | ---: | ---: | ---: | ---: | ---: |
| Combat, 100 dummies, no effects | 0.350 | 0.173 | 2,861 | 33 | 0 |
| Combat, 100 dummies, 30 effects/s | 0.230 | 0.155 | 4,349 | 41 | 1,412 |
| Combat, 100 dummies, 120 effects/s | 0.755 | 0.413 | 1,325 | 41 | 12,047 |
| VFX scene, 240 effects/s | 0.830 | 0.498 | 1,205 | 19 | 10,756 |

Against a 240 FPS budget of 4.17 ms, the worst case measured uses 18 % of it on the CPU and
10 % on the GPU. At combat-realistic effect rates the cost is inside the run-to-run noise of
a sub-millisecond frame; only the deliberate stress moves it measurably.

## Why the particle pass costs so little

The whole pass is **two draw calls**, whatever the particle count: one per blend mode. A
particle is 36 bytes of per-instance data and the quad's four corners come from
`SV_VertexID`, so 12,000 particles is 432 KiB of upload and two `DrawInstanced` calls rather
than 72,000 vertices and a draw per effect.

## Allocation

`vfx_bench` counts every allocation the process makes during the measured frames: **6 to 12
per 600 frames** in every scenario, effects on or off. The VFX system allocates nothing per
frame — instances use a free list, particles a fixed-capacity structure-of-arrays pool,
render entities are pooled per shape and hidden rather than despawned, and every per-frame
buffer is cleared rather than freed.

## The one bound that is reached

Under artificial stress (120+ effects a second) the system sits at its 128-instance ceiling
and refuses effects, while the particle pool never drops one (peak 12,112 of 16,384). Real
combat peaks at 24 instances. The caps are left as they are: raising one to make a synthetic
scenario look better is how a budget stops describing the game.

# Phase 4 — audio

Ryzen 5800X, 1920x1080, vsync off. Audio measurements come from `audio_bench` (1,200 measured
frames per scenario, 300 warm-up, 48 kHz, no device) and from the game's own report line.

## What audio costs the game thread

The game thread's share is observing the frame's events, resolving anchors and updating the
listener. The mix is **not** on it: it happens on the device's callback thread.

| Scenario | sim us | observe us | director us | game thread total | p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| idle, no audio | 0.8 | — | — | 0.8 | 1.2 |
| idle | 0.7 | 0.0 | 0.1 | 0.8 | 1.2 |
| 1 target, no audio | 0.5 | — | — | 0.6 | 0.8 |
| 1 target | 0.6 | 0.0 | 0.3 | 0.9 | 2.3 |
| 10 targets | 0.7 | 0.0 | 0.3 | 1.1 | 1.9 |
| 50 targets | 1.5 | 0.0 | 0.3 | 1.8 | 3.0 |
| 100 targets | 2.5 | 0.0 | 0.3 | 2.8 | 4.5 |
| aoe-heavy, 50, no audio | 1.4 | — | — | 1.5 | 2.0 |
| aoe-heavy, 50 | 1.5 | 0.0 | 0.3 | 1.9 | 3.3 |
| worst case, 100 | 2.5 | 0.0 | 0.3 | 2.8 | 4.7 |

**Audio costs the game thread 0.3-0.4 us a frame**, measured as the difference between paired
runs of the same fight in the same binary. Against a 240 FPS budget of 4,170 us that is under
0.01 %. Event observation does not appear at all above the timer's resolution: it is one pass
over a slice of a few dozen events.

## What the mix costs the audio thread

One callback is 512 frames at 48 kHz — 10.7 ms of sound, asked for roughly every 2.5 game
frames at 240 FPS.

| Scenario | voices (mean) | voices (peak) | mix per callback | share of the callback's own budget |
| --- | ---: | ---: | ---: | ---: |
| idle (wind only) | 1.0 | 1 | 2.7 us | 0.03 % |
| 1-100 targets, single-target rotation | 6.4-6.7 | 12 | 14.6-16.1 us | 0.15 % |
| aoe-heavy, 50 | 6.6 | 13 | 15.2 us | 0.14 % |
| worst case, 100 | 6.6 | 13 | 15.3 us | 0.14 % |

The mix is flat in the number of *targets* and linear in the number of *voices*, which is the
whole point of the limiter: a hundred targets and one target produce the same handful of
voices. The pool is 64 and combat peaks at 13.

## In the game

`mygame --scene combat --dummies 50 --frames 20000 --report 1`, with and without `--no-audio`:

| Configuration | CPU mean | FPS | GPU mean | audio stage |
| --- | ---: | ---: | ---: | ---: |
| Audio on | 0.432 ms | 2,315 | 0.070 ms | 0.000 ms |
| `--no-audio` | 0.426 ms | 2,347 | 0.070 ms | 0.000 ms |

The difference is inside the run-to-run noise of a sub-millisecond frame; the reported audio
stage is below the 1 us the line prints to.

## Memory and allocation

* **Bank**: 8.5 MiB, 39 sounds and 64 variations, synthesised at load in ~100 ms at 48 kHz.
  Mono `f32` PCM, shared between voices behind an `Arc` — a voice holds a cursor, not a copy.
* **Voices**: 64, allocated once at construction and never again.
* **Allocation**: `audio_bench` counts **zero** allocations across the measured frames in
  every scenario, audio on or off. The device callback allocates nothing either: its scratch
  buffer is grown once to the largest block the device asks for.

## The bounds that are reached, and by design

The area rotation on 50-100 targets refuses 79-115 cues over 1,200 frames — all of them to
the per-sound minimum interval and the per-sound cap, none of them to the voice pool
(`dropped` is 0 in every scenario). That is the limiter working: fifty simultaneous hits are
one hit sound, and fifty burning targets are four burning beds.

# Phase 6 — enemy AI

Ryzen 5800X, 1920x1080, vsync off. Figures from `ai_bench` (1,200 measured frames per
scenario, 300 warm-up, no device) and from the game's own report line.

## What a fight costs

The player is teleported into the middle of the pack and kept alive by the harness, so every
scenario measures a fight *in progress* rather than the moment before everything is dead.

| Scenario | AI us | sim us | extract us | cull us | total us | p99 | fighting | allocs/1200 frames |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 enemy | 0.1 | 0.7 | 0.2 | 0.1 | 1.2 | 1.9 | 1 | 7 |
| 10 enemies | 0.9 | 1.9 | 0.9 | 0.4 | 4.1 | 6.8 | 4 | 43 |
| 50 enemies | 8.4 | 8.2 | 4.2 | 1.5 | 22.4 | 37.2 | 33 | 161 |
| 100 enemies | 25.1 | 15.9 | 7.7 | 2.6 | 51.5 | 77.9 | 46 | 236 |
| 10, full frame | 0.9 | 1.9 | 0.9 | 0.5 | 11.5 | 29.2 | 4 | 53 |
| 50, full frame | 8.6 | 8.4 | 4.2 | 2.0 | 53.0 | 103.9 | 33 | 163 |
| 100, full frame | 25.4 | 16.2 | 7.6 | 3.3 | 94.1 | 167.4 | 46 | 235 |

"Full frame" adds the VFX director, the audio director and a mix on the device's own
schedule — the whole presentation stack observing the same fight.

**Against the 240 FPS budget of 4,170 us**, the worst case measured — a hundred enemies,
forty-six of them actively fighting, with effects and audio — uses **2.3 % of the frame**.
A ten-enemy fight, which is what the game will actually contain, uses 0.3 %.

## Where the time goes, and how it scales

* **AI is 0.09 us per enemy at 10, 0.17 at 50, 0.25 at 100.** It is superlinear because
  perception asks the targeting query for the nearest hostile, which is a scan: at 100
  enemies each of them walks a 101-entity world. That is a known cost with a known fix
  (a spatial index, or a shared per-tick nearest-hostile pass), deliberately not taken yet —
  25 us at a hundred enemies is not the frame's problem, and optimising before measuring is
  how a budget stops describing the game.
* **Presentation adds 7.4 / 30.6 / 42.6 us** at 10 / 50 / 100 enemies, almost all of it the
  VFX director: telegraphs are effects, and a hundred enemies produce a lot of them.
* **Allocation is ~0.2 per frame at 100 enemies** — threat rows appearing for a new attacker
  and the world's own restore path. Everything per-tick reuses its buffers.

## In the game

`mygame --scene encounter --frames 8000 --report 2`, two enemies, idle:

| Configuration | CPU mean | FPS | GPU mean | sim |
| --- | ---: | ---: | ---: | ---: |
| Encounter scene, 2 enemies | 0.353 ms | 2,835 | 0.045 ms | 0.003 ms |
| Combat scene, 4 dummies (control) | 0.410 ms | 2,436 | 0.064 ms | 0.004 ms |
| Character scene (control) | 0.458 ms | 2,182 | 0.056 ms | 0.005 ms |

No regression: the encounter scene is *faster* than the scenes it sits beside, because it
draws fewer objects. Nothing in this phase changed the cost of a frame that has no enemies in
it — the AI stage costs nothing when `enemies` is empty, which is the case in every existing
scene.


# Phase 7 — shadows

## What shadows cost

`mygame --scene shadows --frames 900 --report 0 --shadows <level>`, release, 152 objects and
230–390 casters, mean over the run:

| Preset | Frame GPU | Shadow stage CPU | Shadow stage GPU | Shadow draws | Casters |
| --- | --- | --- | --- | --- | --- |
| off | 0.125 ms | – | – | – | – |
| low | 0.177 ms | 0.014 ms | 0.016 ms | 31 | 49 |
| medium | 0.213 ms | 0.030 ms | 0.032 ms | 82 | 215 |
| high | 0.342 ms | 0.037 ms | 0.065 ms | 82 | 215 |
| ultra | 0.422 ms | 0.045 ms | 0.071 ms | 117 | 371 |

The dungeon at high, the heaviest shipping scene: 0.062 ms CPU, 0.079 ms GPU, 252 draws,
151 026 triangles, 393 casters, 6 local shadow views granted and 19 refused.

The draw and caster counts are about 12% lower than the first measurement of the same scene
because floors, ridges and the sky dome stopped being drawn into cascades: a flat plane's
shadow is the plane, and the ground plain is one 640 m quad that was being drawn into every
cascade to change no pixel.

Two readings. The **shadow stage itself is under a tenth of a millisecond** at every preset —
it is a depth-only pass with a null pixel shader over a culled, LOD-reduced caster list.
Most of the difference between off and high is elsewhere: the depth prepass contact shadows
need, and the extra sampling the mesh pass does per pixel. That is why High enables contact
shadows and Medium does not, rather than the other way round.

The stage's CPU cost scales with casters and not with preset, which is what the caster
culling and the four-level shadow LOD are for.

## Memory

The atlas is the whole footprint: 8 MB at low, 16 MB at medium, 64 MB at high, 80 MB at
ultra. Local shadow views cost nothing extra at high — eight 512² tiles fit inside the
quarter of a 4096² atlas that three 2048² cascades leave empty.

## Allocation

The shadow stage allocates nothing per frame. Caster lists, batch scratch and instance
scratch are owned buffers that grow to the scene once and are cleared rather than freed;
constants go through the frame's existing upload ring. Changing the quality **does** allocate
and stall: the atlas is recreated, behind a `wait_for_idle()`, which is why it belongs on a
settings screen rather than on a slider.

# Phase 7b — the shadow optimisation

Eight milestones against the combat arena, each measured with the same script against a
reference binary run in the same session. `docs/performance/shadows/` carries one document per
milestone with the numbers, the quality measurements and the regressions; `00-baseline.md`
states the methodology and `06-final-comparison.md` is the summary.

GPU p50, `--scene combat`, RX 6700 XT:

| Resolution | Before, Ultra | After, Ultra | Before, High | After, High |
| --- | ---: | ---: | ---: | ---: |
| 1280×720 | 0.307 | 0.216 | 0.231 | 0.190 |
| 1920×1080 | 0.606 | 0.396 | 0.454 | 0.358 |
| 2560×1440 | 1.008 | 0.633 | 0.758 | 0.583 |
| 3840×2160 | 2.754 | **1.327** | 1.598 | **1.201** |

The two changes that did most of it: contact shadows became a half-resolution pass with a
depth-weighted reconstruction (1.31 ms to 0.42 at 4K), and the 5×5 PCF grid became a six-tap
rotated disc at the same penumbra width (0.22 ms at 4K). Clustered light culling took the
per-light cost from 0.070 ms to 0.008 and lifted `MAX_LOCAL_LIGHTS` from 8 to 32, which the
dungeon needed — it has twenty local lights and twelve of them were being dropped every frame.

Three measurement controls were added to the game binary and are worth knowing about:
`--warmup <n>`, `--shadow-tweak <k=v,…>` and `--lights <n> --shadow-lights <n>`. The driver is
`tools/bench/shadow_matrix.ps1`, which interleaves its repetitions because the machine drifts
enough over a few minutes to move a 4K number by 20 %.

# Phase 8 — where the frame actually goes, and the passes that were parallelized

## The question that started it

GPU utilization sat at 67 % with the CPU at 4 % of 24 logical processors — one core saturated
and the rest idle, which is what a single-threaded frame loop looks like from Task Manager.

## Milestone 25 — attribution

Two thirds of the frame had never been attributed to anything. `mygame` now reports the stage
split every report interval, and the shadow stage reports its preparation separately from its
recording (`ShadowStats::prepare_ms`). Recording is derived as
`render − cull − group − shadow prep`; wrapping it in a second `Instant` pair would time the
same span twice.

`--scene combat`, 1920×1080 windowed, release, vsync off, 4000 frames after 500 warm-up:

| Stage | Mean |
| --- | ---: |
| events | 0.001 ms |
| update | 0.017 ms |
| render | 0.117 ms |
| — of which cull | 0.002 ms |
| — of which group | 0.004 ms |
| — of which shadow prep | 0.009 ms |
| — of which recording | 0.103 ms |
| present | 0.163 ms |
| **CPU frame** | **0.403 ms** |
| GPU frame | 0.317 ms |

**`Present` is the largest single item in the frame and recording is the second.** Everything
the gameplay pipeline does — simulation, extraction, VFX, audio, culling, UI — is 0.037 ms
together, or 9 % of the frame. That is the number every parallelization decision below is
measured against.

It also settles what GPU utilization can be: it is `gpu_ms / frame_ms`, and the GPU cannot
saturate until GPU time is the longest stage. With `Present` alone imposing 0.5 ms fullscreen
(Milestone 17), no amount of CPU threading saturates a 0.27 ms GPU frame at this workload.
Saturating it needs more GPU work, not less CPU work.

## Milestone 25 — the job system reaches the game

`engine-app` had no `JobSystem` at all: `Culler::cull_parallel` and `Renderer::render_world_parallel`
had been built and tested in Milestone 17 and only the showcase ever called them. The game now
owns a pool of `recommended_worker_count() - 1` workers and takes the parallel path, with
`--serial-jobs` forcing the serial one for A/B and determinism.

Interleaved, 3 runs each, `--hidden`, 3000 frames after 500 warm-up:

| Path | CPU p50 |
| --- | ---: |
| parallel | 0.304, 0.305, 0.312 ms |
| `--serial-jobs` | 0.305, 0.303, 0.304 ms |

**No change, which is the pass criterion.** At ~1000 objects `cull_parallel` falls through to
serial at its own chunk threshold, exactly as designed. This is infrastructure for a scene ten
times larger, not an optimization of this one.

## Milestone 25 — passes made parallel, and the thresholds that keep them serial

`docs/threading.md` measures the job break-even between 0.1 µs and 12 µs of work per job, so a
fan-out needs ~12 µs *per chunk* — roughly 50 µs of total pass before a 4-way split wins. Each
pass below therefore carries a size threshold and stays on the calling thread beneath it.

| Pass | Measured today | Threshold | Runs parallel today? |
| --- | ---: | --- | --- |
| UI batching (`UiBatcher::build_with`) | 13–42 µs | 2048 work units (~95 µs) | no |
| Particle integration (`ParticlePool::update_with`) | part of 17 µs | 8192 particles | no |
| Particle write (`ParticlePool::write_with`) | part of 17 µs | 8192 particles | no |
| Frustum culling (Milestone 17) | 2–4 µs | 4096 slots per chunk | no |

Every one is written so the serial and parallel paths share the same body — `build_range`,
`integrate_chunk`, `write_range`, `cull_range` — and each has a determinism test asserting
byte-identical output at 0, 1, 2, 4, 8 and 16 workers
(`crates/engine-renderer/tests/ui_batch_parallel.rs`,
`crates/engine-vfx/tests/particle_parallel.rs`).

Two things the split cost and had to be paid back:

* The UI merge copied the whole vertex buffer even for one chunk, which put ~4 µs on the serial
  path. A single chunk now swaps its buffer out instead of copying it: 13.9 / 24.0 / 26.4 /
  34.7 µs against the 13.7 / 23.4 / 26.7 / 35.4 µs baseline, 3 runs.
* Splitting particle integration from compaction means two passes over the arrays where there
  was one. Measured neutral at combat sizes, 3 interleaved runs.

**Not built**: parallel extraction and parallel AI perception. Both are real splits and both
are provably zero at any scene this game can currently produce — extraction is 8 µs in the game
and 57 µs at 1,000 dummies, AI perception is sub-microsecond inside a 7 µs tick — and both
would restructure code whose correctness rests on ordering (`RenderWorld`'s free list, the AI
think/execute sequence). The triggers to revisit: 5,000 extracted entities, and 500 enemies —
where a spatial grid is the better answer anyway, because it changes the complexity class.

## Milestone 25 — a 4× VFX regression, found by measuring

`cd3fe69` laid every ground footprint on the terrain as a band of quad pieces instead of one
flat plate. It cost far more than intended, and nothing had measured it.

`vfx_bench`, interleaved against a build of `cd3fe69~1`, 3 runs each, median:

| Scenario | vfx, before | vfx, after the commit | cull | total frame |
| --- | ---: | ---: | ---: | ---: |
| ultimate, 50 | 16.2 µs | 78.7 µs | 1.5 → 10.1 µs | 22.8 → 96 µs |
| worst case, 100 | 17.2 µs | 79.0 µs | 2.7 → 11.1 µs | 29.3 → 100 µs |

Instrumenting `sync_entities` found 52 µs of the 77: **`build_band` 41.5 µs, placement 9.9 µs,
for 5.8 footprints cut into 565 pieces every frame** — about 73 ns of `atan2`, `tan` and
quaternion construction per piece. Every static burning zone was recomputing an identical band
each frame.

**The fix is a cache, not a thread.** A band is keyed on `(shape, anchor, yaw, band)` and
rebuilt only when one of them changes; the ground is the one input the key cannot see, so the
level says when it has moved (`VfxDirector::invalidate_ground`, called from `rebuild_terrain`).

| Scenario | vfx before fix | vfx after fix | total frame |
| --- | ---: | ---: | ---: |
| ultimate, 50 | 78.7 µs | **46.1 µs** | 96 → 61.8 µs |
| worst case, 100 | 79.0 µs | **46.9 µs** | 100 → 67.6 µs |

Median of 3 interleaved runs. The residual against the parent build is inherent to the design
rather than to the cache: ~565 render entities per frame instead of one plate, which is 10 µs
of placement and 8 µs of extra culling, plus the rebuilds of footprints that genuinely move.
The next lever, if it is ever wanted, is `place`'s two `atan2`s and `Quat::from_yaw_pitch_roll`
per piece — worth a few microseconds and not worth the risk of getting a rotation subtly wrong
without a test that would catch it.

## Milestone 26 — presenting on another thread, and why it was reverted

`Present` is the largest single item in the frame (Milestone 25), and it is a wait rather
than work, so the obvious move is to take it off the thread that records frames: a worker
calls `Present` and signals the frame's fence, and the game thread carries on. The renderer
was made `Send` for it and the worker was built. **It does not work, and the reason is worth
keeping.**

### What happened

The device was removed at **exactly the third frame**, every run, windowed and fullscreen:

```
IDXGISwapChain3::Present failed: 0x887a0005 (DXGI_ERROR_DEVICE_REMOVED)
```

Three is the back-buffer count. Frame 3 is the first one that comes back around to buffer
zero, and it was recording into a buffer DXGI had not released.

### Why the fence is not enough

The frame ring's fence proves **GPU execution order**: slot `i` is not reused until the work
recorded from it has completed. It says nothing about **DXGI buffer ownership**. What
enforced that was `Present` itself — a synchronous `Present` blocks once every back buffer is
spoken for, and *that block is the back-pressure*. Moving `Present` to another thread removes
it, and nothing in the fence takes its place.

This is worth stating plainly because the design looked sound: the ordering argument (present
then signal, in queue order, waited on before the slot is reused) is correct as far as it
goes, and it is not far enough.

### The supported replacement, and why it did not hold either

DXGI's answer is a frame-latency waitable object:
`DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT`, `SetMaximumFrameLatency(2)`, and a wait
at the top of each frame. Implemented, the game **hung** instead — the waitable object stopped
being signalled after the first two frames and never recovered. Waiting with a 200 ms timeout
instead of `INFINITE` produced `WAIT_TIMEOUT` and then, on the very next present, the same
`DEVICE_REMOVED`.

So the waitable object was not delivering the back-pressure in this configuration — windowed,
composited, `Present(0, ALLOW_TEARING)`, and a window that is not the foreground one — and
proceeding without it corrupts the chain immediately. Whether that is the tearing flag, DWM
composition, or occlusion was not chased further, because of what the payoff turned out to be.

### The payoff, weighed

The most a present thread can buy is overlapping the frame's CPU work with the display's
wait: `max(0.14 ms of update and recording, 0.163 ms of present)` instead of their sum. That
is a real change — 0.40 ms to roughly 0.20 ms — but the frame is already at 2,500–3,000 FPS
against a 480 FPS stretch target, and the cost of buying it is a swap-chain flag that can hang
the game when its window is covered. That is not a trade worth making for headroom nothing is
currently asking for.

**Reverted**, except for the part that stands on its own: the renderer is `Send`
(Milestone 25's `refactor(renderer)`), asserted by a `const` block, so whoever picks this up
does not have to do that half again.

### What a future attempt has to answer first

1. Why the waitable object stops signalling. Reproduce it in the showcase with tearing off,
   with the window focused, and fullscreen, and find which of the three it is.
2. What happens when the window is occluded. An engine that stops producing frames because
   its window is covered is worse than one that presents on the frame's own thread.
3. Whether the same overlap is reachable without touching presentation at all — the render
   thread of the original Phase 5 plan moves recording off the game thread instead, and its
   back-pressure is the fence it already has.

# Performance audit (2026-10)

The whole audit — baseline, profiles, findings, every change with its interleaved before/after,
what is left — is `docs/PERFORMANCE_AUDIT.md`, with its data in `docs/performance/audit/`.
The numbers below are the frame-time summary, 1920x1080 `--hidden`, Ryzen 9 5900X +
RX 6700 XT, the unoptimized and the final build interleaved in one session, 3 runs each:

| Scenario | CPU p50 before | after | avg FPS before | after | 1% low before | after |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Monster playground fight (21 monsters, autocast) | 0.672 ms | 0.505 ms | 1483 | 2013 | 1087 | 1199 |
| Dungeon (24 animated monsters) | 0.385 ms | 0.381 ms | 2550 | 2577 | 1633 | 1794 |
| Graveyard | 0.525 ms | 0.509 ms | 1852 | 1881 | 1215 | 1238 |
| VFX stress (100 dummies, 120 effects/s) | 1.134 ms | 1.059 ms | 717 | 719 | 316 | 332 |
| Arena, empty scene, encounter, environment stress | unchanged | | | | | |

Two methodology additions this file's rules should carry forward:

* **A parallel path is measured in the game, against `--serial-jobs`.** The UI and particle
  thresholds were set from `cpu_bench`'s break-even, where the workers are awake; in the game
  they are asleep and the split lost by 2x. See `docs/threading.md`.
* **The periodic report's stage split is one frame.** Read the run-wide means the game now
  prints at exit (`run stages`, `run render`) instead; `tools/bench/frame_matrix.ps1` does.
