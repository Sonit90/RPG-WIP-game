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

## Milestone 6 — the debug overlay is not free

Three runs of 30,000 frames per configuration, release, 1920x1080:

| Configuration | FPS | CPU mean | GPU mean |
| --- | --- | --- | --- |
| Overlay on | 4593 / 4677 / 4667 | 0.210–0.212 ms | 0.058 ms |
| Overlay off | 5435 / 5680 / 5719 | 0.163–0.174 ms | 0.043–0.046 ms |

About 17% of frame rate: +0.045 ms CPU, +0.014 ms GPU. Four lines of text is ~1,200 quads
against a scene of one triangle, so the overlay is most of the geometry. **Benchmark runs
should use `--no-overlay`, and the shipping build removes it entirely.**

Measured and rejected: skipping the per-frame vertex upload when the text has not changed
made no measurable difference (4567 / 4719 / 4499 FPS). The change was kept as waste
avoidance, not as an optimization that delivered.
