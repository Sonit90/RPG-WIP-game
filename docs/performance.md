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
| — | — | — |

## Measured results

None yet. Populated from Milestone 3 onward with hardware, build profile, resolution,
sample count and the exact command used.
