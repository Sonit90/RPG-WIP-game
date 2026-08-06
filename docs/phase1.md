# Phase 1 — Engine core

Phase 1 builds the engine core the future renderer and simulation depend on. No gameplay,
no ECS, no physics, no animation, no networking, no assets, no advanced rendering.

The goal was a small, testable core — not a framework.

Milestone status:

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Core application lifecycle | done |
| 2 | Time system | done |
| 3 | Frame timing | done |
| 4 | Input abstraction | done |
| 5 | Logging | done |
| 6 | Diagnostics and assertions | done |
| 7 | Error handling | done |
| 8 | Memory foundations | done |
| 9 | Threading foundation | done |
| 10 | Basic job system | done |
| 11 | Profiling infrastructure | done |
| 12 | Configuration | done |
| 13 | Debug / release separation | done |
| 14 | Testing architecture | done |

## Definition of Done

| Requirement | Status |
| --- | --- |
| Engine lifecycle exists | pass — `engine-core::application`, core owns sequencing |
| Time system exists | pass — real and simulation timelines over one monotonic clock |
| CPU/GPU frame timing works | pass — `engine-core::metrics`, percentiles on an interval |
| Input abstraction exists | pass — `engine-core::input`, no Win32 types above the platform layer |
| Keyboard input works | pass — letters, digits, F1–F12, modifiers, arrows, transitions |
| Mouse input works | pass — movement, five buttons, wheel |
| Logging exists | pass — level, thread, subsystem; verbose levels compiled out of release |
| Diagnostics/assertions exist | pass — `engine_assert!`, `engine_assert_eq!`, `engine_verify!` |
| Error model exists | pass — one `EngineError` with eight categories and a source chain |
| Frame allocator exists | pass — reset every frame by the engine loop |
| Arena allocator exists if justified | pass — the frame allocator is built on it |
| Memory statistics exist | pass — capacity, used, peak, allocations, failures |
| Thread abstraction exists | pass — named workers, cooperative shutdown, joined on drop |
| Basic job system exists | pass — submit, batch, wait, clean shutdown |
| CPU profiler exists | pass — scoped, nested, compiled out of release |
| GPU profiling from Phase 0 remains functional | pass — GPU frame times still reported every run |
| Basic configuration exists | pass — `EngineSettings`, validated at startup |
| Debug/release separation exists | pass — matrix in `docs/testing.md` |
| Automated tests exist | pass — 302 tests across 17 targets |
| Regression tests exist | pass — table in `docs/testing.md` |
| Manual test checklist exists | pass — `docs/manual-tests.md`, MT-001 to MT-014 |
| Performance benchmarks exist | pass — `cpu_bench` plus the Phase 0 `performance_test` |
| Phase 0 renderer still works | pass — verified after every milestone |
| Phase 0 tests still pass | pass |
| Release build does not contain test infrastructure | pass — verified by `tools\package.ps1` |
| Documentation is updated | pass |
| No unexplained unsafe blocks remain | pass — all `unsafe` is in `memory.rs` (core) and `window.rs` (platform), each with a safety comment |
| No undocumented per-frame allocations in the core hot path | pass — see `docs/memory.md` |

### What Phase 1 established, in numbers

| Question | Answer |
| --- | --- |
| Cost of a bump allocation | 1.49 ns (`Box::new` is 34.97 ns) |
| Cost of resetting the frame allocator | 0.42 ns |
| Cost of a frame of input handling | 1.95 ns |
| Cost of a profiler scope while recording | 29.16 ns |
| Cost of a profiler scope compiled out | 0 (identical to an inert scope) |
| Cost of submitting and waiting on one job | ~3.3 µs |
| Cost per job in a 1000-job batch | ~112 ns |
| Where the job system pays off | above ~12 µs of work per job; at ~0.1 µs it is 4× *slower* than sequential |
| Cost of the whole Phase 1 loop addition | below measurement noise — 0.170–0.175 ms vs the Phase 0 band of 0.166–0.184 ms |

### Carried into the next phase

* The job system is correct but naive — one mutex-guarded queue. The measurements that
  would justify work stealing are recorded in `docs/threading.md`; nothing should be moved
  onto jobs without checking the job's size against them.
* Simulation and rendering still run at one rate. `Time` separates the two timelines and
  `advance_by` exists, so a fixed timestep is a change inside the loop.
* Worker threads are not profiled. That needs a per-thread buffer and a merge step, which
  is not worth building before there is multi-threaded frame work to look at.
* Presentation still dominates the windowed frame, as it did in Phase 0. That is a
  renderer problem, untouched here.

---

## Milestone 1 — Core application lifecycle

### What was built

* `engine-core::application`: `Engine<A: Application>`, `EngineContext`, `EngineConfig`,
  `StageTimings`.
* Lifecycle `new() → initialize() → run() → shutdown()`, sequenced by the engine.
* Per-frame stages `process_events → update → render → present → end_frame`, each timed
  separately by the engine.
* `engine-app::App` reduced to an `Application` implementation over the window and the
  renderer.

### The decision that shaped everything after it

`engine-core` must not depend on `engine-platform` or `engine-renderer`, but the core is
required to own lifecycle sequencing. Three options were considered and the choice was
made explicitly:

| Option | Cost |
| --- | --- |
| **Core owns the loop, static dispatch** (chosen) | The concrete application type is baked into the binary |
| Core provides parts, app keeps the loop | Sequencing is enforced by convention, not by the engine |
| Core owns the loop, `Box<dyn Application>` | A virtual call per stage per frame, for a runtime swap with no use case |

The generic version monomorphizes: the six per-frame hooks are direct calls, with no
vtable, no boxed application and nothing allocated. Baking in the application type is
exactly what a single-game engine wants.

### Loop invariants

* Exit is requested through `EngineContext::request_exit`, never by returning from a
  stage. The current frame always completes, so the renderer never exits with a recorded
  but unpresented frame in flight.
* A stage returning `Err` stops the loop immediately; `end_frame` does not run for that
  frame, and `shutdown` always does.
* `is_render_ready() == false` skips `render` and `present`, and their stage timings read
  zero. A minimized window is the motivating case.
* The first frame's delta is zero. Simulation code must tolerate that rather than dividing
  by it.
* Window and device creation stay in `App::new`, not `initialize`: both are values the
  application cannot exist without, and holding them as `Option` to allow a two-phase init
  would add a check to every use for no gain.
* The clock is reset after `initialize` returns — device creation and shader compilation
  take hundreds of milliseconds and are not frame time.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Core sequences the lifecycle | pass |
| Stages run in the documented order | pass — asserted against a recording test application |
| Frame budget bounds a run | pass |
| An error in any stage still shuts down | pass — checked for all four stages |
| Lifecycle calls are idempotent | pass |
| No regression against Phase 0 | pass — 0.172 ms CPU mean vs the 0.166–0.184 ms band |

### Known limitations

* One rate for simulation and rendering.
* `EngineContext` is passed by `&mut` to every stage, so two stages cannot run
  concurrently. That is correct today and will need revisiting when frame work becomes
  parallel.

---

## Milestone 2 — Time system

### What was built

`engine-core::time::Time` — two timelines over one monotonic clock:

| | Source | Clamped | Scaled | Stops on pause | Used by |
| --- | --- | --- | --- | --- | --- |
| Real | `Instant` (QPC) | no | no | no | frame statistics, profiling, benchmarks |
| Simulation | real delta | yes | yes | yes | gameplay, animation, abilities |

Plus `frame_index`, `delta_seconds`, `set_scale`, `set_paused`, `set_max_delta`, `reset`.

### Why the clamp matters

A real frame never takes 250 ms at these frame-rate targets, so a longer delta means the
process was stalled — a breakpoint, a window drag, a driver reset, a swapped-out page.
Passing that through makes bodies teleport through walls and ability timers fire in a
burst. Real time still records the true value, so the stall stays visible in the
statistics.

### Determinism

`advance_by` drives both timelines from a supplied delta rather than the clock. Tests use
it to exercise clamping and scaling without sleeping, and it is the door a recorded replay
— and later a network authority — needs. A fixed timestep is not implemented; the shape
supports it.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Monotonic | pass — `Instant`, never wall clock |
| Delta and elapsed correct on both timelines | pass |
| Frame counting correct | pass |
| Reset behaviour correct | pass — settings survive, measurements do not |
| Pause, scale and clamp never move time backwards | pass — property test over a mixed sequence |

### Known limitations

* No fixed timestep yet.
* `scale` is an `f64` multiply; a deterministic simulation will want integer nanoseconds.

---

## Milestone 3 — Frame timing

### What was built

`engine-core::metrics::FrameMetrics` — CPU and GPU frame-time history, per-stage averages,
and the `FrameSummary` both the log reports and the debug overlay read. The engine records
one sample per frame; nothing else has to.

### Record vs. summarize

| | Cost | When |
| --- | --- | --- |
| `record` | a few stores into pre-allocated buffers | every frame |
| `refresh` | sorts both windows, `O(n log n)` | every 250 ms |

At 5000 FPS a per-frame sort of a 1024-sample window would cost more than the frame it
measures. Readers call `summary()`, which returns a cached `Copy` snapshot and never sorts,
so adding a reader cannot make the frame slower.

### Consequences worth knowing

* Stage averages cover the frames since the last *refresh* (250 ms), not since the last
  report. A one-second report line shows the most recent quarter-second of stage times.
* CPU and GPU are separate windows. GPU timestamps lag by a few frames and are absent
  right after startup or a resize, so a missing sample is skipped rather than recorded as
  zero — averaging in zeros would understate GPU time.
* The final report calls `refresh()` explicitly, because the cached summary can be 250 ms
  old.
* `reset_clock` clears the history: frame times from the previous stress level describe a
  different workload.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| CPU frame time, update time, render time tracked | pass |
| GPU frame time still available from Phase 0 | pass |
| Frame number and FPS available | pass |
| Average, min, max, p50, p95, p99 | pass |
| Expensive statistics not recomputed every frame | pass — asserted in a test with a long interval |
| Metrics exposed to the debug system | pass — the overlay reads the same summary as the log |

---

## Milestone 4 — Input abstraction

### What was built

```
Windows messages -> engine-platform -> InputEvent -> InputState -> application
```

`engine-core::input`: `KeyCode` (63 named keys plus `Unknown`), `MouseButton`,
`InputEvent`, `MouseState`, `InputState`. Virtual-key codes, `WPARAM` bit layouts and
`WM_*` numbers stop inside `engine-platform`.

Nothing here is a gameplay action. There is no `Jump` and no `Fireball`, because bindings
belong to gameplay, which does not exist yet.

### State *and* transitions

`is_key_down` answers "held"; `was_key_pressed` / `was_key_released` answer "changed this
frame". Both are stored, not inferred: polling alone cannot distinguish them, and every
system that tried to infer transitions would keep its own copy of last frame's state and
get it subtly differently.

The engine calls `InputState::begin_frame()` **before** `process_events`, so a press is
visible for exactly the frame its message arrived in.

### Two behaviours that are easy to get wrong

* **Auto-repeat is marked, not dropped.** A repeating key is still held. `KeyDown` carries
  `repeat: bool`; consumers wanting physical presses match on `repeat: false`.
* **Focus loss releases everything without reporting releases.** Key-up messages go to
  whichever window has focus, so a key held while alt-tabbing away would stay down
  forever; but firing release logic for a dozen keys at once would trigger gameplay the
  player never asked for.

### Storage

A bitset (`[u64; 1]` today) plus scalars — no `Vec`, no allocation, and `Copy`. That last
property is deliberate: a deterministic replay records a frame's input by copying this
struct.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| WASD, Space, Escape, F1–F12, digits | pass |
| Mouse movement, buttons, wheel | pass |
| Current state queries | pass |
| Press and release transitions | pass |
| Event translation from Win32 | pass — tested by driving a real `HWND` with `SendMessageW` |
| Core does not depend on Windows messages | pass — `engine-core` has no `windows` dependency |

### Known limitations

* A key pressed *and* released within one frame shows neither transition. Inherent to
  frame-granular state, tested explicitly, and the reason an event-driven consumer must
  read the event stream.
* No raw input, so mouse deltas are affected by pointer acceleration and clamped at the
  screen edge. A first-person camera will need `WM_INPUT`.
* No gamepad.

---

## Milestone 5 — Logging

### What was built

```text
[    1.234] INFO  [main] engine_renderer::device: D3D12 device created
 ^ seconds  ^level ^thread ^subsystem              ^message
```

The subsystem is the `log` target, which defaults to the module path, so existing call
sites are attributed correctly without anyone passing a name. The thread field prefers the
thread's name over its numeric id — which is why workers are named on creation.

### Cost when disabled

1. **Compile time.** `engine-app` enables the `log` crate's `release_max_level_info`
   feature, so `trace!` and `debug!` compile to nothing in release: no branch, no argument
   evaluation.
2. **Run time.** Below that, each macro checks one relaxed atomic before evaluating its
   arguments.

What is not free is a record that passes both filters: formatting allocates. Logging is
therefore banned from the per-frame path; frame metrics accumulate in counters and are
logged on an interval.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| TRACE/DEBUG/INFO/WARN/ERROR | pass |
| Timestamp, thread, level, subsystem | pass |
| Level filtering | pass — checked at Debug, Warn and Off |
| No per-frame logging by default | pass |
| Cheap when disabled | pass — compiled out in release |

---

## Milestone 6 — Diagnostics and assertions

### What was built

| Situation | Tool |
| --- | --- |
| A programmer broke an invariant | `engine_assert!`, `engine_assert_eq!` |
| A check worth keeping in release, cheaply | `engine_verify!` |
| The user, the OS, the GPU or a file failed | `Result<T, EngineError>` |

`engine_assert!` compiles to nothing outside debug builds — condition included, so an
expensive invariant check costs a shipping build nothing. `engine_verify!` always evaluates
and yields its condition: it panics where assertions are enabled and logs an error and
returns `false` where they are not, so a release build reports the problem and keeps
running.

Both failure paths are `#[cold]`, out of line, and log as well as panic — a log sink may be
the only thing captured on someone else's machine.

The `engine-core/assertions` feature keeps the checks in a release binary, for shipping to
a tester with invariants intact.

### Policy

Assertions are not error handling. A missing file, a lost device, an invalid configuration
and a failed allocation all happen to correct programs and return `Result`. An assertion
says "this cannot happen unless the engine itself is wrong", and its failure is a bug
report.

---

## Milestone 7 — Error handling

### What was built

```text
EngineError { category, context, source }
```

Eight categories: `Platform`, `Graphics`, `File`, `Configuration`, `Initialization`,
`Resource`, `InvalidState`, `UnsupportedFeature`. One struct, not a hierarchy: another
wrapper enum per layer would force every caller to write another `match` arm, while the
thing a caller acts on is which subsystem failed and what it was doing.

### The conversions are not mechanical

`From<PlatformError>` and `From<RendererError>` live in those crates — `engine-core` must
not know their types, and the orphan rule forbids putting them in the binary. They map to
the category a caller can *act on*:

| Source | Category | Why |
| --- | --- | --- |
| No D3D12 adapter | `UnsupportedFeature` | No retry or different call will help |
| Invalid swap-chain size | `Configuration` | The value is wrong, not the hardware |
| Failed D3D12 call | `Graphics` | |
| Invalid `WindowConfig` | `Configuration` | Nothing failed in the OS; the values were wrong before any call |

The original error is preserved as the source in every case, and `main` prints
`error.chain()`, so a failure names the call and the `HRESULT` rather than just the
outermost layer.

---

## Milestone 8 — Memory foundations

### What was built

* `Arena` — a linear bump allocator over one fixed block. Allocation is a compare and an
  add; reclamation is one store.
* `FrameAllocator` — an arena reset by the engine at the top of every frame.
* `MemoryStats` — capacity, used, peak, allocations, failures.

A pool allocator was **not** built: it needs a concrete use case (a fixed-size object
recycled at high rate), and Phase 1 has none. Building it now would be a guess at the block
size, the recycling policy and the ownership model.

### Two invariants enforced by the type system

1. **Destructors never run in an arena**, so the typed entry points require `T: Copy`,
   which excludes every type with a `Drop` impl.
2. **Nothing crosses a frame boundary.** `reset` takes `&mut self` while allocations borrow
   `&self`, so a reference that outlives its frame does not compile.

Neither is a rule anyone has to remember.

### Why `&self` allocation

Returning `&mut T` from `&self` is what lets several allocations be alive at once. It needs
`unsafe`, and every block documents why it is required, which invariant makes it sound and
who owns the memory. `Arena` is `Send` but deliberately not `Sync`: the offset is a `Cell`,
so one arena belongs to one thread.

### Instrumentation

`peak` survives `reset`, because a peak that resets every frame is not a peak — it is the
number that sizes the arena. `used` includes alignment padding, since that is what
consumes the block. Refused allocations are counted, because returning `None` leaves no
other trace. The frame allocator warns once when a frame crosses 75% of capacity, and again
only after usage drops back below it.

### Testing

| Case | Covered |
| --- | --- |
| Allocation and round-trip | yes |
| Alignment | `u8`–`u128`, `f32`, `f64`, `repr(align(32))`, `repr(align(64))`, after a deliberate one-byte misalignment |
| Capacity | exact-capacity allocation succeeds, the next byte fails |
| Reset | memory is reused, peak survives |
| Overflow | refused, counted, arena still usable |
| Multiple allocations | non-overlapping, independently writable |
| Zero-sized | empty slices and zero-sized types |
| Edge cases | zero-capacity arena, cross-thread move |
| Engine integration | 100 frames do not accumulate |

Alignment is tested rather than assumed: a bump allocator that returns a misaligned pointer
produces a fault or a silent slowdown far from the allocation site.

---

## Milestone 9 — Threading foundation

### What was built

`Worker` — one named OS thread, stopped through a `ShutdownSignal` the body polls, and
joined on drop.

* **Named**, so a debugger, a crash dump and the log agree on which thread is which.
* **Cooperative shutdown**, because there is no safe way to kill a thread.
* **Joined on drop.** A detached engine thread outliving the data it borrows is a
  use-after-free waiting for a slow machine; forgetting to join is not possible.
* **A panicked worker is reported, not resumed** — `join` returns an `InvalidState` error
  and the caller decides what a dead worker means. `Drop` cannot return, so it logs.

`recommended_worker_count()` leaves one core for the main thread and never returns zero.

---

## Milestone 10 — Basic job system

### What was built

One `Mutex<VecDeque>`, one `Condvar`, one completion counter per submission. `submit`,
`submit_batch`, `JobHandle::wait`, `wait_all`, `shutdown`.

Work stealing, lock-free queues, fibers and dependency graphs are absent on purpose. Each
costs real complexity, and the benchmark exists so the case for adding one can be made from
numbers.

### Measured, and the result is not flattering

1000 jobs, median of 9 samples, 24 logical processors:

| Work per job | Sequential | 1 worker | 11 workers | 23 workers |
| --- | --- | --- | --- | --- |
| ~0.1 µs | 0.105 ms | 0.171 ms (0.61×) | 0.487 ms (0.22×) | 0.449 ms (0.23×) |
| ~12 µs | 12.48 ms | 12.46 ms (1.00×) | 1.29 ms (9.64×) | 0.72 ms (17.25×) |

**Small jobs are slower in parallel, and more workers make it worse.** At ~0.1 µs of work,
submission and queue contention cost several times the job itself. At ~12 µs the same pool
scales 17×. The break-even is between those two points on this machine.

The rule that follows: parallelize by **chunking**, not per item. A job per particle is on
the wrong side of the line; a job per 1000 particles is on the right side.

### Rules

* Jobs must not wait on other jobs — if every worker blocks waiting, nothing is left to run
  the work. This is the limitation that eventually motivates a fiber scheduler.
* A panicking job does not kill its worker; the panic is recorded and surfaced by `wait`.
  Losing a worker silently would appear only as unexplained slowness later.
* Zero workers is legal: jobs then run on the calling thread during `submit`, so a
  single-threaded or determinism-testing configuration uses the same API.

---

## Milestone 11 — Profiling infrastructure

### What was built

`profile_scope!(profiler, "Name")` and a `Profiler` that records name, depth, start offset
and duration into a buffer allocated once. The tree is reconstructed by the reader from the
depths, so the recording path does no tree building, no hashing and no allocation. The
engine scopes `Events`, `Update`, `Render` and `Present`.

Buffer overflow is counted and reported rather than growing the buffer mid-frame — that is
exactly the problem the profiler exists to find, not to cause.

### Two modes

| Build | Cost per scope |
| --- | --- |
| Release (compiled out) | 2.57 ns — identical to an inert scope, i.e. the harness, not the profiler |
| Release + `engine-core/profiling` | 29.16 ns |

The feature exists so the build that ships can be profiled. Profiling a debug build tells
you about a debug build.

### Why the profiler is an `Rc` handle

Borrowing it out of `EngineContext` would make every profiled stage a borrow conflict with
the `&mut EngineContext` the stage needs. `Rc` and not `Arc` because it records one thread;
worker threads will need their own profilers and a merge step.

---

## Milestone 12 — Configuration

### What was built

`EngineSettings`: width, height, display mode, vsync, debug overlay, renderer validation,
frame budget, report interval, title — validated once at startup.

```text
EngineSettings
  ├── width / height / fullscreen  -> engine-platform
  ├── vsync / renderer_validation  -> engine-renderer
  ├── debug_overlay                -> engine-renderer (feature-gated)
  └── frame_budget / report        -> engine-core
```

`engine-core` cannot depend on the platform or renderer crates, so the settings hold the
values and the executable distributes them. Nothing parses configuration per frame.

Command-line options are *overrides* on top of the defaults, not a parallel source of
truth: a default that moves moves for every run that did not name that option.

A bad value is a message before anything is created:

```
error: configuration: width 10 is outside 64..=16384
```

### Borderless fullscreen

Implemented as a `WS_POPUP` window at the primary monitor's size. Borderless rather than
exclusive: alt-tab is instant, the compositor keeps working, and there is no mode switch to
fail. Verified live at 3840×2160 and covered by a test comparing the client size against
`GetSystemMetrics`.

Dimensions are validated even in fullscreen, where they are unused — a later switch to
windowed should not be the moment the configuration turns out to be wrong.

### Known limitations

* Fullscreen always uses the **primary** monitor, not the one the window is on.
* No configuration file. Defaults plus command-line overrides only; a file is trivial to
  add on top of the typed struct when something needs to persist.

---

## Milestone 13 — Debug / release separation

### What differs by build

| Configuration | `engine_assert!` | `profile_scope!` | `trace!`/`debug!` | Validation layer |
| --- | --- | --- | --- | --- |
| debug | on | recording | on | on |
| release | off | compiled out | compiled out | off |
| release + `assertions` | on | compiled out | compiled out | off |
| release + `profiling` | off | recording | compiled out | off |
| release + `--validation` | off | compiled out | compiled out | on |

The debug overlay is the deliberate exception: it ships in release, because players and
testers need frame-time numbers without a command line. `--no-default-features` removes it.

### Packaging

`tools\package.ps1` builds `--release --bin mygame`, copies it to `dist\mygame\`, and then
verifies the result: exactly one executable, no file whose name resembles a test or
benchmark artefact, no test symbols in the binary, and a two-frame smoke run.

Cargo already makes this true by construction — `#[cfg(test)]` modules compile only under
the test profile, and `tests/` and `benches/` targets are built only by `cargo test` and
`cargo bench`. The script exists because a future build script, feature or copy step could
break it silently.

```
PASS: package contains no test or benchmark infrastructure
Contents (0.28 MB):
  mygame.exe
  README.md
```

---

## Milestone 14 — Testing architecture

### What exists

| # | Category | Location | Command |
| --- | --- | --- | --- |
| 1 | Unit tests | `#[cfg(test)]` in crate sources | `cargo test --workspace --lib` |
| 2 | Integration tests | `crates/<crate>/tests/` | `cargo test --workspace --test '*'` |
| 3 | GPU integration tests | `crates/engine-renderer/tests/` | `cargo test -p engine-renderer` |
| 4 | GPU/renderer benchmarks | `crates/engine-bench` | `cargo run --release --bin performance_test` |
| 5 | CPU subsystem benchmarks | `crates/engine-bench` | `cargo run --release --bin cpu_bench` |
| 6 | Manual verification | `docs/manual-tests.md` | human |

302 tests across 17 targets.

Coverage by subsystem: time (monotonicity, delta, frame counting, reset, pause, scale,
clamp), input (press, release, held, mouse, event translation, focus loss, auto-repeat),
memory (allocation, alignment, capacity, reset, overflow, statistics, edge cases),
threading (startup, execution, completion, shutdown, panics), logging (level filtering,
layout), configuration (defaults, overrides, validation).

### Regression tests

Every bug that gets fixed gets a test, and the test stays after the bug is gone. The
accumulated list is in `docs/testing.md`.

### Manual checklist

`docs/manual-tests.md`, MT-001 to MT-014: startup, resize, close, minimize, keyboard,
mouse, focus loss, overlay, stress levels, fullscreen, multi-monitor, GPU initialization on
other hardware, validation layers, release package, log levels, long-run stability.

A SKIPPED manual test is not a pass. Two entries need hardware not every machine has and
stay SKIPPED until someone runs them there.

---

## Static checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace
cargo build --workspace
cargo build --workspace --release
```

All pass. Clippy runs with `-D warnings` in both feature configurations; the workspace
denies `unwrap_used`, `expect_used`, `todo` and `undocumented_unsafe_blocks` in engine
code.

## Unsafe inventory

| Location | Why | Invariant |
| --- | --- | --- |
| `engine-core/src/memory.rs` | Bump allocation must return `&mut T` from `&self` | Regions never overlap; `reset` takes `&mut self`, so no reference can be alive when memory is reused |
| `engine-platform/src/window.rs` | Win32 FFI and the `GWLP_USERDATA` state pointer | The pointer is created before the window and reclaimed after `DestroyWindow`; the procedure runs only inside `pump_events`, which needs `&mut Window` |
| `engine-renderer/**` | D3D12/DXGI FFI (Phase 0) | Unchanged this phase |
| test modules | Setting `MYGAME_LOG`, reading an initialized `MaybeUninit` slice | Documented at each site |

No other `unsafe` exists in `engine-core`.
