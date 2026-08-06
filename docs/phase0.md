# Phase 0 — Rust / D3D12 technical spike

Phase 0 validates the technical foundation only. No gameplay, no ECS, no assets,
no lighting, no UI framework.

Milestone status:

| # | Milestone | Status |
| --- | --- | --- |
| 0 | Project bootstrap | done |
| 1 | Windows platform layer | done |
| 2 | D3D12 initialization | done |
| 3 | Render loop | done |
| 4 | Triangle renderer | done |
| 5 | GPU timing | done |
| 6 | Debug performance overlay | done |
| 7 | Rendering stress tests | done |
| 8 | Instancing test | done |
| 9 | Automated GPU integration test | not started |

---

## Milestone 0 — Project bootstrap

### What was built

* Cargo workspace at the repository root (`resolver = "3"`, edition 2024).
* `engine-core`: a stderr logging backend for the `log` facade, with level parsing and
  an `MYGAME_LOG` environment override.
* `engine-app`: the `mygame` binary. Installs the logger and exits. No graphics.
* `rustfmt.toml`, `rust-toolchain.toml` (pins stable + `rustfmt` + `clippy` +
  the `x86_64-pc-windows-msvc` target).
* Workspace lints: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::todo`,
  `clippy::undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn` are denied;
  `missing_docs` warns.
* Release profile: `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `debug = 1`
  (line tables retained so release profiling is possible).
* Docs: `README.md`, `architecture.md`, `phase0.md`, `testing.md`, `performance.md`.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| `cargo build` succeeds | pass |
| `cargo test` succeeds | pass |
| `cargo fmt --check` succeeds | pass |
| `cargo clippy` succeeds | pass |

### Manual verification

```powershell
cargo run --bin mygame
```

Expect three log lines on stderr (`mygame 0.1.0`, a milestone line, a debug line)
and exit code 0. `$env:MYGAME_LOG = "warn"; cargo run --bin mygame` should
print nothing.

### Known limitations

* No window, no device, no rendering — by design for this milestone.
* The logger writes synchronously to stderr under a lock, and `log!` macros allocate
  while formatting. Acceptable for startup and interval reporting; banned from the
  per-frame path.
* Timestamps are process-relative seconds, not wall clock.

---

## Milestone 1 — Windows platform layer

### What was built

`engine-platform`, depending on `windows` 0.62 (no hand-written FFI):

* `config.rs` — `WindowConfig` (title, client size, resizable, visible) and validation
  against `MIN_DIMENSION..=MAX_DIMENSION`. Pure data, no Win32.
* `event.rs` — `WindowEvent` (`Resized`, `Minimized`, `Restored`, `CloseRequested`,
  `FocusChanged`) and `EventQueue`. Consecutive resizes coalesce, because dragging a
  border emits hundreds of `WM_SIZE` messages and only the final size matters to a
  renderer that recreates swap-chain buffers. Pure logic, no Win32.
* `error.rs` — `PlatformError` with `InvalidConfig` and `Win32 { call, source }`; the
  message names the failing API and its `HRESULT`.
* `window.rs` — the only module with `unsafe`. Class registration (once per process),
  `CreateWindowExW`, the window procedure, `PeekMessageW` pump, `DestroyWindow` on drop.
* `dpi.rs` — per-monitor DPI awareness V2, opted into before any window exists.

Design points:

* The requested size is the **client** size. `AdjustWindowRectEx` grows the outer window
  so 1920x1080 means 1080 rows of pixels to render, not 1080 minus the title bar.
* Window state lives in a `Box<WindowState>` whose pointer is stashed in `GWLP_USERDATA`
  during `WM_NCCREATE` and reclaimed after `DestroyWindow` returns.
* `WM_CLOSE` does not destroy the window; it enqueues `CloseRequested` and lets the
  application decide. This is what will later allow a "really quit?" prompt or a save.
* `Window::raw_handle() -> isize` is the entire renderer-facing surface. `engine-renderer`
  will not depend on `engine-platform` types.
* The message pump is non-blocking (`PeekMessageW` + `PM_REMOVE`), so Milestone 3 can own
  the frame pace with no artificial cap.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Application launches | pass |
| Window appears | pass (smoke run: `MainWindowTitle = "myGame — phase 0"`, responding) |
| Window can be resized | pass automatically for the message path; visual check is manual |
| Window closes cleanly | pass (`CloseMainWindow` -> `close requested` -> `clean shutdown`) |
| No crash on shutdown | pass (exit code 0) |

### Manual verification

Run `cargo run --bin mygame` and check:

1. A 1920x1080 window titled `myGame — phase 0` appears.
2. Dragging a border logs `resized to WxH` with the final size, not a flood of lines.
3. Minimizing logs `minimized`, restoring logs `restored`.
4. Alt-tabbing away and back logs `focus: false` / `focus: true` at debug level.
5. Clicking the close button logs `close requested` then `clean shutdown`, and the
   process exits with code 0 leaving no window behind.

### Known limitations

* No keyboard or mouse input yet — `WM_KEYDOWN`, raw input and mouse capture arrive with
  the gameplay layer, not in Phase 0.
* Only one window per process is supported (single registered class, single event queue).
* Resize is reported, not throttled: a renderer recreating swap-chain buffers must handle
  a burst of size changes itself (Milestone 2/3).
* The temporary 1 ms sleep in the app loop exists only because there is nothing to render
  yet. Milestone 3 removes it.
* Integration tests need an interactive desktop session; see `docs/testing.md`.

---

## Milestone 2 — D3D12 initialization

### What was built

`engine-renderer`, depending on `windows` 0.62 and `engine-core` only — never on
`engine-platform`. A window arrives as an opaque `isize` handle.

* `adapter.rs` — `AdapterInfo` plus `select_adapter`, the pure selection policy:
  hardware always beats software; among hardware, most dedicated video memory wins;
  ties keep enumeration order; software is used only if it is all that exists. No vendor
  or device id is ever consulted for behaviour, so no GPU model is hardcoded.
* `device.rs` — DXGI factory, adapter enumeration, `D3D12CreateDevice`, direct command
  queue, feature-level probe (12_2 down to 11_0), tearing-support query, and the
  validation layer (debug builds only, with break-on-error).
* `swapchain.rs` — flip-discard swap chain, 3 back buffers, RTV heap, `ResizeBuffers`,
  `Present`. Alt+Enter is detached from DXGI so it cannot resize the chain behind us.
* `sync.rs` — `GpuFence`: a fence plus an auto-reset event, separating per-frame pacing
  waits from full idle waits.
* `renderer.rs` — `Renderer`: per-slot command allocators, one command list, record →
  execute → present → signal, plus `resize` and `wait_for_idle`.
* `engine-app` now creates the renderer and presents cleared frames until close.

Synchronization model — the two waits are deliberately different:

* **Pacing**: before reusing slot `i`, wait for the fence value signalled the last time
  slot `i` was submitted. With three slots the CPU only blocks when it is a full lap
  ahead of the GPU. This is the only wait in the frame path.
* **Idle**: signal, then wait for that value. Fully serializes CPU and GPU. Used before
  `ResizeBuffers` (the GPU may still be reading the old back buffers) and in `Drop`
  (releasing resources the GPU still references is what produces device-removed reports
  and debug-layer live-object errors at shutdown). Never used per frame.

### Startup output (this machine)

```
GPU: AMD Radeon RX 6700 XT
dedicated GPU memory: 12242 MiB
feature level: 12_1
backend: Direct3D 12
resolution: 1920x1080
presentation: vsync=false, tearing=true
```

Adapter 1, `Microsoft Basic Render Driver` (software), was enumerated and rejected by the
selection policy.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| D3D12 device initializes | pass |
| Correct GPU selected | pass (discrete adapter chosen over the software one, by policy) |
| Swap chain initializes | pass |
| Application can present frames | pass (39,402 frames in a 3.6 s debug run) |
| Clean shutdown without GPU synchronization errors | pass (exit 0, no validation output) |

### Manual verification

`cargo run --bin mygame`, then check:

1. The startup block above appears with *your* GPU name and VRAM.
2. The window is filled with a dark blue-grey, not an unpainted white/black rectangle.
3. Resizing repaints at the new size with no stretching and no validation errors.
4. Minimizing and restoring does not crash (rendering is skipped while minimized).
5. Closing logs `presented N frames`, `renderer shut down after N frames`,
   `clean shutdown`, and exits with code 0.

### Performance

Not a benchmark — there is no timing infrastructure until Milestone 3. The only figure
available is a frame count over a wall-clock window: **39,402 frames in ~3.63 s
(~10,850 FPS)** clearing 1920x1080 in a **debug** build with the validation layer on.
It shows presentation is uncapped, nothing more. No CPU or GPU frame time has been
measured yet, and no claim about the 240/360/480 FPS targets is made.

### Known limitations

* Only clears the back buffer. No pipeline state, no geometry — Milestone 4.
* GPU frame time is not measured; Milestone 5 adds timestamp queries.
* No handling of `DXGI_ERROR_DEVICE_REMOVED`: a TDR or driver update currently surfaces
  as a generic `Present` failure. Recovery needs a device-recreation path.
* No fullscreen or display-mode handling; the swap chain is windowed only.
* Resize waits for full GPU idle. Correct, but it stutters during a drag; a future
  version can recycle buffers without a full stall.
* The renderer reads `GetCurrentBackBufferIndex` as its frame-slot index, which assumes
  the flip model's round-robin ordering. Valid for flip-discard, but it must be revisited
  if the swap effect ever changes.
* `engine-renderer` has `engine-platform` as a **dev**-dependency so GPU tests can create
  a real window. The library itself does not depend on it, so the layering holds.

---

## Milestone 3 — Render loop and CPU frame timing

### What was built

* `engine-core::time` — `FrameClock` (monotonic deltas, frame counter) and
  `FrameStatsWindow`: a fixed-capacity ring of frame times with mean/min/max and
  nearest-rank p50/p95/p99. Both the sample buffer and the sort scratch are allocated
  once, so recording a sample and computing statistics allocate nothing.
  Nearest rank rather than interpolation: a reported p99 should be a frame that actually
  happened.
* `engine-renderer` — `render_clear` no longer presents. `present` is a separate call
  that presents and then signals the slot fence, so the loop keeps its four-stage shape
  and each stage can be timed on its own. Presenting without a recorded frame, or twice,
  is an error; a resize drops a frame recorded against the destroyed buffers.
* `engine-app` — `app.rs` holds the loop, `args.rs` the command line
  (`--width`, `--height`, `--vsync`, `--frames`, `--report`, `--help`).

The loop:

```text
process_events() -> update() -> render() -> present()
```

No frame cap, no sleep, no vsync by default. `update` is deliberately an empty stage: it
exists so the shape and its timing are in place before there is anything to simulate.
Statistics are logged on an interval (default 1 s) because `log!` allocates while
formatting, which is banned from the frame path.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Loop follows process_events -> update -> render -> present | pass |
| No artificial frame cap, no fixed sleep | pass (verified: 60,000 frames in 65.2 s, uncapped) |
| CPU frame time measured | pass |
| FPS reported | pass |
| Frame-time percentiles | pass (p50/p95/p99, plus min/max, over a 1024-frame window) |

### Performance

Release build, 1920x1080, clear-only, Ryzen 7 5800X + RX 6700 XT, 60,000 frames:

```
ran 60000 frames in 65.154 s (921 FPS average over the whole run)
CPU frame: mean 1.20 ms (~830 FPS) | p50 0.076 | p95 ~8 | p99 ~18 | min 0.050 | max ~24
stages:    events 0.000 ms | update 0.000 ms | render ~1.15 ms | present ~0.053 ms
```

**The distribution is bimodal and that is the headline result.** Half of all frames cost
0.076 ms (~13,000 FPS), but roughly one frame in twenty costs 7–10 ms and one in a
hundred costs 16–24 ms. The stall is charged to the `render` stage because that is where
the per-slot fence wait lives.

The display refresh period on this machine is **4.166 ms (240 Hz)**, measured directly by
running with `--vsync`:

```
--vsync: mean 4.206 ms (238 FPS) | p50 4.166 | p95 4.202 | n=600
```

Every stall value is an integer multiple of that period: p95 ≈ 2 refreshes, p99 ≈ 4,
max ≈ 6. That quantization is the signature of a **synchronization/presentation**
bottleneck, not a CPU or GPU one. A windowed swap chain under DWM composition hands back
buffers on the compositor's schedule, so `ALLOW_TEARING` does not make windowed
presentation truly uncapped: the frame queue drains, the pacing fence blocks, and a burst
of cheap frames is followed by a refresh-quantized stall. The average (921 FPS) is real,
but neither it nor the p99 says anything yet about how fast the engine *can* run.

Worth noting for later: the 240 FPS minimum target is exactly this display's refresh
rate, so with vsync on the engine already meets it (238 FPS measured) — but that number
describes the monitor, which is precisely why the default is vsync off.

This is exactly what the measurement system was built to expose, and it is why the
benchmark executable (section 6 of the plan) must not measure through a composed window.
No optimization has been attempted — measure first.

> **Correction (Milestone 5).** The stall pattern above **did not reproduce**. Running the
> same command on the Milestone 5 build gives ~5800 FPS with p99 ≈ 0.65 ms and no
> refresh-quantized stalls at all, reproducibly across four runs, both with the window in
> the foreground and occluded. The conclusion that windowed DWM composition imposes a
> synchronization bottleneck was drawn from a single run and is **not supported**; the
> most likely explanation for the original numbers is contention from other work on the
> machine during that one measurement. The lesson is recorded rather than deleted: a
> single run is not a measurement, and an attribution built on one is a guess. Current
> figures are in `docs/performance.md`.

### Manual verification

```powershell
cargo run --release --bin mygame -- --frames 60000 --report 1
cargo run --release --bin mygame -- --frames 600 --report 2 --vsync
cargo run --bin mygame -- --help
```

Check that reports appear once per second, that `--vsync` pins the mean to your display's
refresh period (4.166 ms on the 240 Hz panel used here), and that closing the window still
logs a clean shutdown.

### Known limitations

* GPU frame time is still not measured. Every number above is CPU wall time; the GPU may
  be idle or saturated and these numbers cannot tell them apart. Milestone 5.
* The presentation stall described above is measured but not addressed. Options for later:
  a waitable swap-chain object, a lower maximum frame latency, or fullscreen/independent
  flip. All are optimizations and belong after the GPU timing exists.
* `update` does nothing, so its 0.000 ms says nothing about future simulation cost.
* Statistics cover a rolling 1024-frame window, so a report during a stall burst shows a
  worse p95 than a steady-state window would.
* `--frames` exits after a frame count, not a duration; a run's wall-clock length depends
  on how fast the frames are.
* **Dragging or resizing the window stops rendering, and one drag becomes one enormous
  frame sample.** Windows runs a modal message loop inside `DefWindowProcW` for the whole
  drag, so `pump_events` does not return and `render`/`present` are never reached. The
  window keeps showing its last presented frame (DWM composites it), so it looks frozen
  rather than black. The measurement consequence matters more: the entire drag lands in a
  single `FrameClock::tick`, and that one multi-second sample distorts `max`, `p99` and
  the mean until it rolls out of the 1024-frame window. Deliberately left as-is for now —
  fixing it means either emitting `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` events and dropping
  the spanning sample, or driving redraws from a `WM_TIMER` inside the modal loop, which
  would require the platform layer to call back into application code. **Treat frame-time
  statistics taken across a window drag as invalid.**
* **GPU integration tests must run serialized.** Presenting from several D3D12 devices
  concurrently in one process crashes inside the driver; the tests now take a
  process-wide mutex. Full diagnosis in `docs/testing.md`. The engine itself has one
  device on one thread, so it is unaffected — but a future multi-device tool would be.

### Bugs found and fixed during this milestone

Chasing an intermittent `STATUS_ACCESS_VIOLATION` in the test suite turned up two real
defects, both fixed:

1. `Renderer::resize` called `ResizeBuffers` while the command list still referenced the
   old back buffers. A recorded list holds references until it is reset, and
   `ResizeBuffers` requires that none survive. The list is now reset to an empty state
   after the idle wait.
2. `Renderer`'s field order released the device before the swap chain and command list.
   D3D12 objects must outlive nothing — the device must be released last. `context` is
   now the final field, with a comment saying the order is load-bearing.

Neither turned out to be the cause of that particular crash, which was reproduced with
correct API usage and a silent validation layer, but both were genuine.

---

## Milestone 4 — Triangle renderer

### What was built

* `shader.rs` — `compile_hlsl` via `D3DCompile` from `d3dcompiler_47.dll`. Errors carry
  the compiler's own diagnostics, not a bare `HRESULT`. Debug builds compile with
  `WARNINGS_ARE_ERRORS` and no optimization; release builds optimize. No cache, no
  reflection, no hot reload — this is not a shader system.
* `shaders/triangle.hlsl` — `VSMain`/`PSMain`, position + colour, embedded with
  `include_str!` so the binary needs no files beside it.
* `pipeline.rs` — `Vertex` (`repr(C)`, 28 bytes), `TRIANGLE_VERTICES`, an empty root
  signature (input-assembler layout only), the graphics PSO, and an upload-heap vertex
  buffer.
* `renderer.rs` — `render_clear` became `render`: it clears, sets viewport and scissor
  from the swap-chain size, binds the root signature, PSO, topology and vertex buffer,
  and issues one `DrawInstanced`. Adds `draw_calls_last_frame` / `triangles_last_frame`.
* `offscreen.rs` — renders to a texture, copies to a readback buffer and returns the
  pixels. No window required. Milestone 9 will build on this.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Vertex buffer, root signature, PSO, VS, PS, command recording, presentation | pass |
| A visible triangle is rendered | pass — verified by offscreen readback, see below |

### How "visible" was verified

A recorded draw call and a silent validation layer are **not** evidence that anything was
drawn. The first version of this milestone produced both while rendering an empty screen.

`tests/offscreen_triangle.rs` therefore renders offscreen and inspects the pixels:

* the triangle covers 16.5% ± tolerance of the image (0% if culled, 100% if the target is
  filled — both fail);
* samples near each vertex are red, green and blue respectively, and the centre is a lit
  blend, which proves the input layout, interpolation and vertex order;
* the four corners keep the clear colour;
* a distinctive clear colour arrives at the render target unchanged.

The test's power was checked by reintroducing the bug: with `FrontCounterClockwise` set
back to `false`, coverage drops to 0.0% and the test fails. It was then reverted.

### Bug found and fixed: the triangle was culled

Symptoms: a correctly recorded draw call, no validation-layer output, and an empty
screen showing only the clear colour.

Cause: winding is judged on the **render target**, where Y points down.
`TRIANGLE_VERTICES` is counter-clockwise in clip space (Y up) and stays counter-clockwise
after the flip, but the rasterizer was configured with `FrontCounterClockwise: false`,
declaring clockwise triangles front-facing. The triangle was therefore back-facing and
back-face culling discarded it — silently.

Fix: `FrontCounterClockwise: true`, with the reasoning written into `rasterizer_desc` and
a unit test asserting that the vertex winding and the rasterizer setting agree.

A second bug was fixed on the way: `ManuallyDrop::into_inner` on the PSO descriptor's
`pRootSignature` dropped the non-owning interface copy, releasing a reference the
descriptor never took. That underflowed the root signature's refcount and killed the test
process at exit code 2173. `ManuallyDrop` fields holding borrowed COM pointers must simply
be left alone.

### Performance

Not re-measured. The frame now contains one draw call of one triangle, which is not a
meaningful workload — Milestone 7's stress test is where geometry cost gets measured.

### Manual verification

```powershell
cargo run --release --bin mygame
```

A triangle with a red top vertex, green bottom-left and blue bottom-right, smoothly
interpolated, on the dark blue-grey background. It should stay centred and correctly
proportioned while the window is resized.

### Known limitations

* The vertex buffer lives on an upload heap. Correct, and the simplest thing that works
  for three static vertices, but it means the GPU reads vertex data over PCIe every draw.
  Real geometry needs a default-heap buffer filled through a staging copy; that belongs
  with Milestone 7, where the difference is measurable.
* Vertices are already in clip space — there is no camera, no transform and no constant
  buffer. Milestone 7 needs at least a per-instance transform.
* Shader model 5_1 via `D3DCompile`, not shader model 6 via DXC: DXIL would require
  shipping `dxcompiler.dll`.
* Shaders compile at startup on every run (~milliseconds for two entry points), with no
  cache.
* `offscreen.rs` creates its own device per call, so it is a verification tool, not a
  rendering path.

---

## Milestone 5 — GPU timing

### What was built

* `gpu_timer.rs` — `GpuTimer`: a timestamp query heap with one query pair per frame slot,
  a readback buffer for resolved results, and the queue's timestamp frequency.
* `renderer.rs` — a timestamp is written at the start of each frame's command list and
  another after the final barrier, then resolved into the readback buffer. The result is
  read at the *next* use of that slot.
* `engine-app` — GPU frame times get their own statistics window and their own report
  line, so CPU and GPU numbers are never mistaken for the same sample.

### Why not wall-clock timing

CPU timing around `ExecuteCommandLists` measures submission — the call returns once the
driver has queued the work. Timing around a fence wait measures how long the CPU was
blocked, which mostly reflects how far ahead it was allowed to run. Neither is GPU time.
Only timestamps written by the GPU itself measure GPU execution.

### Synchronization implications

A timestamp pair lives on the GPU timeline and can only be read once the GPU has passed
it. Reading it in the same frame would mean a fence wait every frame, serializing CPU and
GPU — which would change the very thing being measured.

So the read is deferred: each slot's timestamps are read at the start of that slot's next
use, `FRAME_COUNT` (3) frames later, at a point where the renderer **already** waits on
that slot's fence for its own pacing. The extra synchronization cost is therefore zero,
and the price is that reported GPU time lags CPU time by up to three frames. In steady
state that is invisible; when frame cost changes abruptly the two lines disagree for a few
frames, and the overlay in Milestone 6 must not present them as simultaneous.

`last_gpu_frame_ms()` returns `None` until a slot has been reused, so the first frames
have CPU time but no GPU time. Timestamps that run backwards (a GPU reset between the
pair) are reported as `None` rather than as a negative or wrapped duration.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| GPU timestamp queries implemented | pass |
| Debug output distinguishes CPU and GPU frame time | pass, on separate lines |
| GPU time not derived from CPU wall clock | pass — `ID3D12GraphicsCommandList::EndQuery` + `ResolveQueryData`, converted with `GetTimestampFrequency` |
| Synchronization implications documented | pass — above, and in `gpu_timer.rs` |

### Performance

Release, 1920x1080, clear + one triangle, 60,000 frames, four runs (full table in
`docs/performance.md`):

```
CPU frame: mean 0.168 ms (5961 FPS) | p50 0.149 | p95 0.231 | p99 0.652 | max 0.919
GPU frame: mean 0.045 ms            | p50 0.044 | p95 0.045 | p99 0.046 | max 0.331
stages:    events 0.001 | update 0.000 | render 0.035 | present 0.153 ms
```

CPU time is ~3.7x GPU time, and 0.153 ms of the 0.166 ms CPU frame is the `Present` call
itself. At this scene size the engine is bound by presentation, not by rendering.

**This also retracted a Milestone 3 conclusion** — see the correction note in that
section. The stall pattern reported there did not reproduce in any of four runs.

### Manual verification

```powershell
cargo run --release --bin mygame -- --frames 60000 --report 5
```

Expect a `GPU frame:` line beside each `CPU frame:` line, with GPU mean well below CPU
mean, and a `GPU timestamp frequency` line at debug level during startup.

### Known limitations

* One timestamp pair per frame: the whole command list, with no per-pass breakdown. Finer
  scopes are only worth adding once there is more than one pass.
* GPU time is up to three frames stale, by design.
* Timestamps come from the direct queue only; a future copy or compute queue needs its own
  heap and its own frequency.
* `GetTimestampFrequency` is read once at startup. Modern hardware keeps a fixed timestamp
  clock, but a driver that changed it at runtime would silently skew the numbers.

---

## Milestone 6 — Debug performance overlay

### What was built

* `font.rs` — a 5x7 bitmap font as bit patterns: digits, uppercase letters and the
  punctuation the overlay uses. Unknown characters render as a hollow box so a missing
  glyph is visible rather than silently blank.
* `overlay.rs` — `OverlayGeometry` turns text into quads in clip space, one quad per lit
  font cell, and `OverlayBuffers` holds one persistently mapped upload buffer per frame
  slot.
* `renderer.rs` — draws the overlay last, through the **same** root signature, pipeline
  state and topology as the scene. Only the vertex buffer changes.
* `engine-app` — builds the text four times a second (formatting allocates, so it stays
  off the per-frame path) and adds `--no-overlay`.

The overlay shows FPS (mean and p99), CPU frame time, GPU frame time, resolution and
adapter name.

### Why no texture, and no UI framework

Drawing one quad per lit cell reuses the existing pipeline exactly as it is: no font
atlas texture, no sampler, no shader-visible descriptor heap, no second shader, no
upload/transition of texture data. That is the smallest thing that can display text here,
and it keeps the overlay to a vertex generator plus an upload buffer. The trade is
measured below and is not free.

### Shipping in the release build (decision changed)

The Phase 0 brief asked for the overlay to be *disabled or compiled out* of the final
release build. **The project owner has since decided the final build needs the overlay**,
so the shipped configuration now includes it, hidden by default and toggled with **F5**.

The mechanics that support both:

| Control | Effect |
| --- | --- |
| `debug-overlay` Cargo feature (default **on**) | Whether the overlay exists in the binary. `--no-default-features` removes it entirely, verified below. |
| `--overlay` | Start with the overlay visible. |
| **F5** | Toggle at any time, including in a release build. |

Default state is hidden: an overlay nobody asked for should not draw.

The removal path still works and is verified rather than assumed — the overlay's format
string `MS  P99` is present in the default release binary and **absent** from the
`--no-default-features` one, which is also 8 KiB smaller (262,656 vs 254,464 bytes).

Supporting F5 required keyboard input, which the platform layer did not have:
`WM_KEYDOWN`/`WM_SYSKEYDOWN` now produce `WindowEvent::KeyPressed(Key)`. Auto-repeat is
filtered using bit 30 of `lparam`, so holding a key toggles once rather than thousands of
times. Unmapped keys keep their virtual-key code in `Key::Other` instead of being dropped.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Displays FPS, CPU frame time, GPU frame time, resolution, adapter name | pass |
| No heavyweight UI framework | pass — 5x7 bitmap font, existing pipeline, no texture |
| Controlled by a clean build configuration | pass — `debug-overlay` feature removes it from the binary; the shipped build keeps it, hidden, per the owner's decision |

### Performance

Release, 1920x1080, four **interleaved** pairs of 20,000-frame runs — hidden and visible
alternating, so machine drift affects both equally:

| Configuration | FPS (4 runs) | mean FPS | CPU mean | GPU mean |
| --- | --- | --- | --- | --- |
| Overlay hidden | 5240 / 5363 / 5433 / 5515 | 5388 | 0.193 ms | 0.049 ms |
| Overlay visible | 5438 / 5307 / 5425 / 5545 | 5429 | 0.185 ms | 0.0495 ms |

**The overlay's cost is below this setup's noise floor** (roughly ±5%). Visible even
measured nominally faster, which is simply run-to-run variance. About 2,400 extra tiny
triangles are nothing for this GPU. The draw is confirmed to happen: reports show
`draws 2 | tris 2381` when it is visible.

> **Retraction.** An earlier version of this section claimed the overlay cost ~17% of
> frame rate (+0.045 ms CPU, +0.014 ms GPU), from three runs with it on and three with it
> off. Those runs were **grouped by configuration**, not interleaved, so a slow patch on
> the machine landed entirely in the "on" group. This is the same error as the Milestone 3
> retraction, one milestone later. **A/B comparisons are interleaved from now on**, and
> that is why the table above alternates.
>
> A second claim goes with it: the per-frame vertex upload (~200 KiB of `memcpy`) was
> blamed for the CPU cost, and change-detection was added to skip it. Since the cost it
> was meant to explain does not exist, the caching is credited with nothing. It is kept
> only because copying unchanged data every frame is waste the allocation policy in
> `docs/performance.md` calls out.

### Manual verification

```powershell
cargo run --release --bin mygame                    # no overlay until F5
cargo run --release --bin mygame -- --overlay       # overlay from the first frame
cargo run --release --bin mygame --no-default-features   # F5 does nothing, no overlay
```

Press **F5** in the first: four lines of text appear in the top-left corner over the
triangle, updating about four times a second. Press it again and they disappear. Holding
F5 down must toggle once, not flicker.

### Known limitations

* One quad per lit cell is simple but inefficient in principle — a font atlas would need
  one quad per glyph, roughly a 30x reduction in primitives. Measurement says the current
  approach costs nothing detectable, so this stays unchanged until a measurement says
  otherwise.
* Only key *presses* are reported; there is no `KeyReleased`, no modifier state and no
  text input. Enough for a toggle, not enough for gameplay.

---

## Milestone 7 — Rendering stress tests

### What was built

* `stress.rs` — deterministic generation of N triangles in a grid that fills the clip
  volume. Triangle *size* shrinks as the count grows, so every level covers roughly the
  same screen area and the comparison measures geometry throughput rather than fill rate.
* `mesh.rs` — `GpuMesh`: a **default-heap** vertex buffer filled through an upload-heap
  staging copy. This replaces Milestone 4's upload-heap buffer, which would have had the
  GPU reading 8.4 MiB of vertex data across PCIe every frame at the largest level — the
  benchmark would have measured the bus.
* `pipeline.rs` — `TrianglePipeline` became `ScenePipeline`: root signature and pipeline
  state only, no geometry, so the scene can be swapped without recompiling shaders.
* `renderer.rs` — `set_scene_triangles`, which waits for idle, resets the command list and
  uploads the new mesh.
* `engine-app` — `--triangles <n>` and runtime keys: **F1-F4** select 1 / 100 / 1,000 /
  10,000, **F6** selects 100,000, **F7/F8** step down and up. F5 stays the overlay toggle.
  Switching levels clears the statistics windows, because frame times from the old
  workload say nothing about the new one.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Supports 1 / 100 / 1,000 / 10,000 / 100,000 triangles | pass |
| Measures CPU frame time, GPU frame time, FPS, draw calls | pass — reported per interval and in the overlay |
| Simple runtime toggle | pass — F1-F4, F6, F7/F8, plus `--triangles` at startup |

### Performance

Release, 1920x1080, overlay hidden, 15,000 frames per run. **Three interleaved rounds**:
each round runs every level once, so machine drift spreads across all levels rather than
landing on one.

| Triangles | FPS (3 rounds) | mean FPS | CPU mean | GPU mean | Draw calls |
| --- | --- | --- | --- | --- | --- |
| 1 | 5487 / 5579 / 5605 | 5557 | 0.180 ms | 0.052 ms | 1 |
| 100 | 5595 / 5631 / 5743 | 5656 | 0.173 ms | 0.050 ms | 1 |
| 1,000 | 5543 / 5597 / 5618 | 5586 | 0.177 ms | 0.052 ms | 1 |
| 10,000 | 5531 / 5593 / 5577 | 5567 | 0.177 ms | 0.055 ms | 1 |
| 100,000 | 6838 / 6829 / 6826 | 6831 | 0.143 ms | 0.077 ms | 1 |

Stage breakdown, same runs:

| Triangles | render (record + submit) | present |
| --- | --- | --- |
| 1 | 0.035 ms | 0.147 ms |
| 10,000 | 0.034 ms | 0.142 ms |
| 100,000 | 0.032 ms | 0.107 ms |

### What this says

**The engine is presentation-bound at every level, and geometry is nearly free.**

* GPU time rises from 0.052 ms to 0.077 ms across a 100,000x increase in triangles —
  100,000 triangles in 0.077 ms is about 1.3 billion triangles per second, which is a
  believable primitive rate for this GPU and confirms the geometry itself is not a limit
  at this scale.
* CPU recording time is **flat** at ~0.034 ms regardless of triangle count, which is
  expected: one draw call submits the whole scene, so the CPU does the same work every
  time. This is the number Milestone 8 will move by splitting the scene into many draws.
* `present` dominates the CPU frame at 0.107-0.147 ms — three to four times the recording
  cost.

**An unexplained result, stated as unexplained:** at 100,000 triangles `present` gets
*cheaper* (0.107 ms vs 0.147 ms), making the heaviest scene the fastest overall. It
reproduced in all three interleaved rounds, so it is real, not drift. Plausible causes
include DXGI present-queue pacing interacting with the longer GPU frame, but nothing here
tests that, so no mechanism is claimed. It is a flag for the benchmark executable: as long
as `present` behaves like this, frame rate through a composed window is not a clean
measure of renderer cost, and GPU time plus recording time are the trustworthy numbers.

### Manual verification

```powershell
cargo run --release --bin mygame -- --overlay
```

Press F1-F4, then F6, and watch the overlay's triangle count and the log line
`stress level N: M triangles`. F7 and F8 step through the levels. The screen should stay
similarly covered at every level, with the triangles getting smaller — that constant
coverage is what makes the levels comparable, and it is asserted by a test.

### Known limitations

* One draw call per frame at every level. The draw-call axis is Milestone 8's job; this
  milestone deliberately varies only triangle count.
* Every triangle is identical in size within a level and the whole scene is static — no
  transforms, no camera, no per-object data. A real scene would also cost per-object CPU
  work, which this does not model.
* Switching levels stalls the GPU (`wait_for_idle`) and re-uploads the mesh; the frames
  around a switch are not representative, which is why the statistics windows are cleared.
* The largest level allocates an 8.4 MiB vertex buffer at load time. Nothing frees it
  until the next switch.
* Frame rate is limited by presentation, so these numbers say little about the 240/360/480
  FPS targets for a real workload.

---

## Milestone 8 — Instancing benchmark

### What was built

* `instancing.rs` — `InstanceData` (offset, scale, colour), a `UNIT_TRIANGLE` mesh, a
  deterministic instance grid, and `InstancedPipeline`: a two-slot input layout where slot
  1 is `PER_INSTANCE_DATA` with a step rate of 1.
* `shaders/instanced.hlsl` — places and tints the unit triangle from the per-instance
  stream.
* `bench.rs` — `BenchHarness`: an **offscreen** device, render target, timestamp queries
  and both draw modes, plus `capture` for pixel readback.
* `crates/engine-bench` — the `performance_test` binary (section 6 of the plan). A
  separate crate, so `cargo build --release --bin mygame` never compiles it.

### What makes the comparison fair

The two modes share the device, pipeline state, root signature, mesh, instance buffer and
render target. The **only** difference is submission:

| Mode | Call | Draw calls |
| --- | --- | --- |
| A | `DrawInstanced(3, 1, 0, i)` per instance — `StartInstanceLocation` picks the instance | N |
| B | `DrawInstanced(3, N, 0, 0)` once | 1 |

Because both read the same instance buffer, they must produce the same image — and that
is asserted rather than assumed: `tests/instancing_equivalence.rs` renders both offscreen,
reads the pixels back and requires **zero** differing pixels at 1, 7 and 100 instances.

### Why the benchmark renders offscreen

Milestone 7 measured `present` costing three to four times the CPU of recording an entire
frame, and behaving in ways that do not track renderer cost. A benchmark that presented
would mostly measure the compositor. `performance_test` therefore has no window, no swap
chain and no `Present`, and reports:

* **CPU record + submit** — the number draw-call count moves;
* **GPU time** — from timestamp queries;
* **serialized frame time** — record, submit *and wait*, since there is no pipelining
  here. Any frame rate derived from it is labelled a lower bound, never plain "FPS".

### Results

1920x1080, release, 300 measured frames plus 30 warm-up per configuration. A and B are
interleaved per instance count. **Three full runs of the benchmark:**

| Instances | Draw calls A / B | A CPU | B CPU | A GPU | B GPU | CPU ratio |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 1 / 1 | 0.016–0.018 ms | 0.016–0.018 ms | 0.034 ms | 0.034–0.035 ms | ~1.0x |
| 100 | 100 / 1 | 0.017–0.021 ms | 0.015–0.018 ms | 0.040 ms | 0.040 ms | 1.0–1.2x |
| 1,000 | 1,000 / 1 | 0.040–0.043 ms | 0.015–0.020 ms | 0.075 ms | 0.054–0.058 ms | 2.1–2.6x |
| 10,000 | 10,000 / 1 | 0.252–0.264 ms | 0.018–0.023 ms | 0.415–0.421 ms | 0.080–0.085 ms | 11.5–15.1x |
| 100,000 | 100,000 / 1 | 2.654–2.702 ms | 0.014–0.019 ms | 3.98–4.11 ms | 0.076–0.141 ms | 141–188x |

### What this establishes

* **Draw calls are not free, and they cost on both sides.** At 100,000 instances mode A
  spends 2.65 ms of CPU and 4.11 ms of GPU; mode B spends 0.014 ms and 0.076 ms for the
  identical image. That is roughly **27 ns of CPU and 41 ns of GPU per draw call**.
* **Mode B's cost is flat.** From 1 to 100,000 instances, B's CPU time stays at
  0.014–0.023 ms: the CPU issues one call regardless, and only the GPU notices the extra
  geometry.
* **The crossover is around 1,000 objects.** Below a few hundred instances the two modes
  are indistinguishable — instancing is not worth complexity at that scale. By 10,000 it
  is a 12–15x CPU difference, and at 100,000 it decides whether the frame fits in budget
  at all.
* **Budget context:** at 240 FPS the whole frame is 4.17 ms. Mode A at 100,000 objects
  spends 2.65 ms of that on the CPU alone before any gameplay runs; mode B spends 0.3% of
  the budget. For the target of many enemies and effects on screen, per-object draw calls
  are not viable and the renderer must batch.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| Compares many draw calls against one instanced draw | pass |
| Reports instance count, draw calls, CPU time, GPU time, frame rate | pass |
| Establishes a baseline for data-oriented rendering | pass — per-draw-call cost quantified above |
| Benchmark is not part of the game executable | pass — separate crate and binary |

### Manual verification

```powershell
cargo run --release --bin performance_test
cargo run --release --bin performance_test -- --instances 1000 --frames 500
```

### Known limitations

* Every instance uses the same mesh and pipeline state. Real scenes change state between
  draws, which is usually more expensive than the draw call itself, so mode A here is a
  *best case* for per-object drawing and the real penalty would be larger.
* Instance data is uploaded once and never changes. A real renderer rewrites transforms
  every frame, which adds upload cost that this benchmark does not measure.
* No depth buffer, no overdraw control, no textures, no culling.
* The serialized frame time is a lower bound: a pipelined renderer overlaps CPU and GPU,
  so its frame rate would be higher than the figure printed here.
* GPU times below ~0.04 ms are close to the timestamp resolution and to fixed per-frame
  overhead, which is why the smallest scenes show no difference between modes.
* Uppercase only; text is upper-cased before lookup.
* The overlay is drawn with the scene's pipeline, so it has no blending and no background
  panel. Over a bright scene it may be hard to read.
* Cell size is fixed at 2 px, so the overlay shrinks in relative terms at higher
  resolutions.
* Overlay text is built by the application, not the renderer. That keeps the renderer free
  of formatting, but it means the overlay cannot show anything the application does not
  pass it.
