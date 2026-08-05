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
| 4 | Triangle renderer | not started |
| 5 | GPU timing | not started |
| 6 | Debug performance overlay | not started |
| 7 | Rendering stress tests | not started |
| 8 | Instancing test | not started |
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
