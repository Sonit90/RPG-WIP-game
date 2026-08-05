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

**This is a synchronization bottleneck, and it means the windowed loop cannot measure the
engine's ceiling.** The benchmark executable must therefore either render offscreen or
present outside composition. No mitigation has been applied — measure first, and GPU
timestamps (M5) come before any attempt to fix this.
