# Phase 0 — Rust / D3D12 technical spike

Phase 0 validates the technical foundation only. No gameplay, no ECS, no assets,
no lighting, no UI framework.

Milestone status:

| # | Milestone | Status |
| --- | --- | --- |
| 0 | Project bootstrap | done |
| 1 | Windows platform layer | done |
| 2 | D3D12 initialization | not started |
| 3 | Render loop | not started |
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
