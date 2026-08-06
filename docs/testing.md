# Testing

Testing is a first-class requirement. Tests must protect existing behaviour from
regressions, and no test code may end up in the shipped game executable.

## Categories

| # | Category | Location | Command |
| --- | --- | --- | --- |
| 1 | Unit tests | `#[cfg(test)]` modules inside crate sources | `cargo test --workspace --lib` |
| 2 | Integration tests | `crates/<crate>/tests/*.rs` | `cargo test --workspace --test '*'` |
| 3 | GPU/rendering integration tests | `crates/engine-renderer/tests/` (M9) | `cargo test -p engine-renderer` |
| 4 | Performance benchmarks | `crates/engine-bench` | `cargo run --release --bin performance_test` |
| 5 | Manual verification | `docs/phase0.md`, per milestone | human |

Everything in categories 1–3 runs under `cargo test --workspace`.

## Why test code cannot reach the release build

Cargo compiles `#[cfg(test)]` modules only under the `test` profile, and it builds
`tests/` and `benches/` targets only for `cargo test` / `cargo bench`. Therefore

```powershell
cargo build --release --bin mygame
```

links none of it. This is the reason integration tests live in `tests/` directories
rather than behind a runtime flag, and the reason test-only branches inside production
code are avoided: such a branch *would* ship.

## Running the checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Both feature configurations must build and lint cleanly, because the shipping build is
the one without the overlay:

```powershell
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo build --release --no-default-features
```

`clippy` runs with `--all-targets` so that test and bench code is linted too.
Test files may relax `clippy::unwrap_used` / `expect_used` with a crate-level
`#![allow(...)]`; engine code may not.

## Window test caveats

`crates/engine-platform/tests/window_lifecycle.rs` creates real `HWND`s. They are created
with `visible = false` so the test run does not steal focus or flash windows, but they
still need an interactive desktop session — a headless service account or a container
without a window station cannot run them, and `CreateWindowExW` will fail there.

The tests drive the window procedure with `SendMessageW`, which dispatches synchronously
on the calling thread. That is deliberate: it exercises the real `WM_CLOSE` and `WM_SIZE`
handling without depending on user input or timing. `cargo test` runs test functions on
separate threads, and each test owns the window it creates, so the message pump never
sees another test's window.

What is *not* covered automatically: that the window is actually painted, that dragging a
border feels correct, and that the title bar renders the expected text. Those are in the
manual checklist for Milestone 1 in `docs/phase0.md`.

## GPU test caveats

GPU integration tests require a D3D12 capable adapter on the machine running them. On a
machine without one — a CI container, for example — they skip with a `SKIP:` message
rather than fail; `crates/engine-renderer/tests/d3d12_init.rs` shows the pattern
(`GpuContext::new` returning `NoSuitableAdapter` means skip, any other error is a real
failure).

Those tests create a swap chain, which needs an `HWND`, so `engine-renderer` takes
`engine-platform` as a **dev**-dependency. The library never depends on it — it accepts
an opaque `isize` handle — so the layering rule still holds for anything that ships.
The windows are hidden.

### GPU tests are serialized on purpose

`cargo test` runs test functions on separate threads. Presenting concurrently from
several D3D12 devices inside one process crashes with `STATUS_ACCESS_VIOLATION` inside
the driver on the reference machine (RX 6700 XT). What was established while diagnosing
it, on 2026-08-06:

* Every test passes when run alone; the crash needs at least two threads presenting.
* Creating and destroying devices, windows and swap chains concurrently is fine.
* Recording and submitting command lists concurrently is fine. Adding `Present` is what
  triggers it.
* It is unaffected by window visibility, message pumping, `MakeWindowAssociation`,
  `CS_OWNDC`, and object teardown order, and it still happens when both objects are
  leaked rather than dropped.
* The D3D12 validation layer reports nothing before the crash.

The engine has exactly one device on one thread, so this configuration never occurs
outside the test harness. Every GPU test therefore takes a process-wide mutex
(`gpu_lock()` in `d3d12_init.rs`) before touching the GPU. If a future test creates a
renderer, it must take that lock too.

Two real bugs were found while chasing this and fixed regardless:

1. `ResizeBuffers` was called while the command list still held references to the old
   back buffers. The list is now reset to an empty state first.
2. `Renderer`'s fields were declared so the device was released before the swap chain and
   command list. `context` is now declared last; field order in that struct is
   load-bearing.

Run them with output to see which adapter was used:

```powershell
cargo test -p engine-renderer --test d3d12_init -- --nocapture --test-threads=1
```

### Verifying that something was actually drawn

`crates/engine-renderer/tests/offscreen_triangle.rs` renders through
`engine_renderer::offscreen`, which draws into a texture, copies it into a readback buffer
and returns the pixels. Assertions are made on those pixels: coverage fraction, colours
near each vertex, corners left at the clear colour.

This exists because weaker signals lie. During Milestone 4 the renderer recorded a valid
draw call, the validation layer said nothing, and the screen stayed empty — the triangle
was being back-face culled. Only a pixel-level check catches that.

**Do not verify rendering by taking screenshots of the desktop.** A screen capture picks
up whatever window happens to be in front, which makes it both unreliable as a test and a
way to capture unrelated on-screen content. Offscreen readback needs no window, no
desktop session and no compositor, and it is deterministic.

Milestone 9 extends this same module into the full automated GPU integration test.

## Manual verification

Some behaviour cannot be asserted automatically: that a window actually appears, that
it resizes without artefacts, that a triangle is visible, that the debug overlay is
legible. Each milestone in `docs/phase0.md` lists its manual verification steps.
