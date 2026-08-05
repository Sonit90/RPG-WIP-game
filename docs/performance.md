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

Not yet implemented. Planned:

* **CPU frame time** — `Instant` deltas around the full `process_events -> update ->
  render -> present` sequence (M3).
* **GPU frame time** — D3D12 timestamp queries resolved into a readback buffer and read
  one or more frames later, so the CPU never blocks on the GPU to obtain them (M5).
  GPU time is never derived from wall-clock timing.
* **Percentiles** — p50/p95/p99 over a fixed-capacity ring buffer of frame samples, so
  sampling itself does not allocate per frame (M3/M6).

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
| `Renderer::render_clear` | none | Command list, allocators and the `ExecuteCommandLists` argument are all built once in `Renderer::new`; barriers live on the stack |
| `Window::pump_events` | none in the common case | `EventQueue` pre-reserves 32 events and reuses the buffer; only a burst beyond that grows the `VecDeque` |

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

Hardware: Ryzen 7 5800X, Radeon RX 6700 XT (12242 MiB), feature level 12_1, tearing
supported. This is not a benchmark: it has no percentiles, no CPU/GPU split, and it was
taken in a debug build.
