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
