# Architecture

## Layering

```
Game            (executable, wiring only)
  |
Gameplay / Simulation      (not in Phase 0)
  |
RenderWorld                (data handoff: transforms, meshes, camera, instances)
  |
Renderer                   (D3D12 resources, pipelines, command recording)
  |
D3D12 / DXGI               (windows-rs bindings)
```

Dependencies point downward only. The renderer never learns gameplay concepts —
no `Player`, `Ability`, `Damage`, `Buff`, `Debuff`, `DPS` or `Enemy` type may appear
in `engine-renderer`. The simulation publishes a `RenderWorld` snapshot; the renderer
consumes it. That boundary is what later allows the simulation to run on its own
schedule and, eventually, to be driven by a network authority.

## Crates

| Crate | Purpose | Introduced |
| --- | --- | --- |
| `engine-core` | Logging, timing, shared error types. No OS, no GPU. | M0 |
| `engine-app` | The `mygame` executable. Wires platform + renderer. | M0 |
| `engine-platform` | Win32 window, message pump, input events. No GPU types. | M1 |
| `engine-renderer` | DXGI/D3D12 device, swap chain, pipelines, GPU timing. | M2 |

`engine-platform` hands the renderer an opaque window handle and a client size.
It does not link D3D12; the renderer does not create windows.

## Engine lifecycle (Phase 1, M1)

`engine-core::application` owns the lifecycle and the frame loop:

```
Engine::new() -> initialize() -> run() -> shutdown()
```

```
process_events() -> update() -> render() -> present() -> end_frame()
```

`engine-core` cannot depend on `engine-platform` or `engine-renderer` (see Layering), so
the seam is the `Application` trait: `engine-app` implements it over the window and the
renderer, and `Engine<A: Application>` drives it.

### Why a generic and not `Box<dyn Application>`

Monomorphization turns the six per-frame hooks into direct calls — no vtable, no boxed
application, nothing allocated per frame. A runtime-swappable application has no use case
in a single-game engine, so the flexibility of `dyn` would be paid for and not used.

### Why the loop moved out of `engine-app`

Simulation and rendering will eventually run at different rates. That is a property of the
loop, so it belongs where the engine can change it once rather than in each executable
(game, benchmark harness, future tools). `EngineContext` is the only channel: it carries
the frame delta, frame index, elapsed time and per-stage CPU timings, and it is where the
frame allocator, input state and profiler will live in later milestones.

### Loop invariants

* Exit is requested via `EngineContext::request_exit`, never by returning from a stage.
  The current frame always completes, so the renderer never exits with a recorded but
  unpresented frame in flight.
* A stage returning `Err` stops the loop immediately; `end_frame` does not run for that
  frame, and `shutdown` always does.
* `Application::is_render_ready() == false` skips `render` and `present`; their stage
  timings then read zero. A minimized window is the motivating case.
* The first frame's delta is zero: no frame has completed yet. Simulation code must
  tolerate that rather than dividing by it.
* Construction (window, D3D12 device) happens in `App::new`, not in `initialize`. Both are
  values the application cannot exist without, and holding them as `Option` to permit a
  two-phase init would add a check to every use for no gain. `initialize` exists for work
  that needs a running engine context.
* The clock is reset after `initialize` returns: device creation and shader compilation
  take hundreds of milliseconds and are not frame time.

## Time (Phase 1, M2)

`engine-core::time::Time` carries two timelines and one frame counter:

| | Source | Clamped | Scaled | Stops on pause | Used by |
| --- | --- | --- | --- | --- | --- |
| Real | `Instant` (QPC) | no | no | no | frame statistics, profiling, benchmarks |
| Simulation | real delta | yes | yes | yes | gameplay, animation, abilities |

* **Monotonic only.** `Instant` cannot move backwards; wall-clock time is never used for
  frame timing, so an NTP step or a user changing the system clock cannot produce a
  negative or enormous delta.
* **The simulation delta is clamped** to `DEFAULT_MAX_DELTA` (250 ms). A longer frame means
  the process was stalled — a breakpoint, a window drag, a driver reset. Passing that
  through makes bodies teleport through walls and ability timers fire in a burst. Real time
  still records the true value, so a stall stays visible in the statistics.
* **Performance numbers must read real time.** `App::end_frame` records
  `time().real_delta()`; using the simulation delta would make a paused game look infinitely
  fast.
* **`advance_by` exists for determinism, not only for tests.** Driving `Time` from supplied
  deltas rather than from this machine's clock is what a recorded replay — and later a
  network authority — needs. Tests use the same door to exercise clamping and scaling
  without sleeping.
* **Fixed timestep is not implemented.** The shape supports it: a fixed-step loop
  accumulates `delta()` and consumes it in fixed increments, and since the simulation only
  ever reads `Time`, nothing outside the loop changes.

## Frame metrics (Phase 1, M3)

`engine-core::metrics::FrameMetrics` owns CPU and GPU frame-time history, per-stage
averages, and the `FrameSummary` that both the log reports and the debug overlay read.
The engine records one sample per frame; nothing else has to.

The split that matters is **record vs. summarize**:

| | Cost | When |
| --- | --- | --- |
| `record` | a few stores into pre-allocated buffers | every frame |
| `refresh` | sorts both windows, `O(n log n)` | every 250 ms |

At 5000 FPS a per-frame sort of a 1024-sample window would cost more than the frame it
measures. Readers call `summary()`, which returns the cached snapshot and never sorts, so
adding a reader cannot make the frame slower. `FrameSummary` is `Copy`, so a reader gets a
consistent snapshot rather than a half-updated view.

Consequences worth knowing:

* Stage averages cover the frames since the last *refresh* (250 ms), not since the last
  report. A one-second report line therefore shows the most recent quarter-second of stage
  times, not the whole second.
* CPU and GPU are separate windows. GPU timestamps lag the CPU by a few frames and are
  absent right after startup or a resize, so a missing GPU sample is skipped rather than
  recorded as zero — averaging in zeros would understate GPU time.
* The final report calls `refresh()` explicitly: the cached summary can be up to 250 ms
  old, and the closing numbers should cover the whole window.
* `EngineContext::reset_clock` clears the history too. Frame times from the previous stress
  level describe a different workload.

## Deviations from the originally proposed repository layout

1. **The workspace lives at the repository root** rather than in a nested
   engine directory. The repository already exists for this project,
   so a nested directory would add a level with no content of its own.
2. **No top-level `tests/` and `benches/` directories.** In a virtual Cargo workspace
   these are inert: Cargo only discovers `tests/` and `benches/` inside a package.
   Integration tests therefore live in `crates/<crate>/tests/` and benchmarks in
   `crates/<crate>/benches/`. This also enforces the "no test code in the shipped
   executable" rule for free, since neither target type is built by
   `cargo build --release --bin mygame`.
3. **Placeholder crates are created when they get their first real code**, not up front.
   Empty crates cannot be built, tested or linted in a meaningful way, and they make
   `cargo clippy --workspace` output misleading.
4. `tools/` and `assets/` are created when the first tool or asset exists.

## Technology decisions

### Why Rust

* Predictable, no-GC performance with a memory-safety model that survives a large
  codebase — the failure mode of a hand-written C++ engine at this scale is lifetime
  bugs in resource management, exactly what the borrow checker covers.
* `unsafe` is explicit and greppable, so the risky surface (the D3D12 FFI boundary)
  stays auditable instead of being spread through the whole codebase.
* First-class tooling out of the box: `cargo test`, `cargo bench`, `clippy`, `rustfmt`.
  Testing is a first-class requirement for this project, and Rust does not require a
  build-system project to get it.
* Data-oriented layouts (`Vec<T>` of plain structs, slices, explicit indices) are
  natural, which matters for the eventual instanced/batched renderer.

### Why Direct3D 12

* Windows is the only launch target, so a cross-platform abstraction would be paid for
  and not used in Phase 0.
* Explicit command lists, descriptor heaps and fences are what make a 240–480 FPS budget
  measurable: the CPU cost of a frame is visible instead of hidden in a driver runtime.
* GPU timestamp queries give real GPU frame time, which D3D11 exposes far less directly.
* Multi-threaded command recording is available when the draw-call count grows.
* `windows-rs` provides maintained, generated bindings, so no hand-written FFI is needed.

### Why not an existing engine

Unreal, Unity, Godot and Bevy all impose a frame architecture and an update model. The
performance targets here (a stable 240 FPS floor, 360 FPS target) are a property of the
frame architecture, so owning it is the point of the project.

## Error handling policy

* Engine crates propagate `Result`. `unwrap()`/`expect()` are denied by a workspace lint.
* Unrecoverable initialization failures produce a message naming the failing D3D12/DXGI
  call and its `HRESULT`, then exit with a failure code.
* Every `unsafe` block carries a `// SAFETY:` comment stating why `unsafe` is required
  and which invariant makes it sound.

## Allocation policy

No custom allocator in Phase 0. The rules that keep a future frame allocator possible:

* No heap allocation in the per-frame render path where practical.
* Per-frame buffers are allocated once at init and reused.
* Every known per-frame allocation is documented in `docs/performance.md`.
* Logging allocates (message formatting), so it is banned from the hot path; frame
  metrics are accumulated into plain counters and logged on an interval.
