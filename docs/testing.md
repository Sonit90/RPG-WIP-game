# Testing

Testing is a first-class requirement. Tests must protect existing behaviour from
regressions, and no test code may end up in the shipped game executable.

## Categories

| # | Category | Location | Command |
| --- | --- | --- | --- |
| 1 | Unit tests | `#[cfg(test)]` modules inside crate sources | `cargo test --workspace --lib` |
| 2 | Integration tests | `crates/<crate>/tests/*.rs` | `cargo test --workspace --test '*'` |
| 3 | GPU/rendering integration tests | `crates/engine-renderer/tests/` | `cargo test -p engine-renderer` |
| 4 | GPU/renderer benchmarks | `crates/engine-bench` | `cargo run --release --bin performance_test` |
| 5 | CPU subsystem benchmarks | `crates/engine-bench` | `cargo run --release --bin cpu_bench` |
| 6 | Manual verification | `docs/manual-tests.md` | human |

Everything in categories 1–3 runs under `cargo test --workspace`.

## Build configurations that must be checked

Behaviour differs by build, so a green `cargo test` in one configuration is not a green
project. What each configuration changes:

| Configuration | `engine_assert!` | `profile_scope!` | `trace!` / `debug!` | Validation layer |
| --- | --- | --- | --- | --- |
| debug | on | recording | on | on |
| release | off | compiled out | compiled out | off |
| release + `engine-core/assertions` | on | compiled out | compiled out | off |
| release + `engine-core/profiling` | off | recording | compiled out | off |
| release + `--validation` | off | compiled out | compiled out | on |

The first two are the ones that matter and are checked every milestone. The others exist
so a specific question can be asked of the build that ships: keep invariant checks in a
tester's build, profile the optimized binary, or debug GPU misuse in release.

## Regression tests

Every bug that gets fixed gets a test, and the test stays after the bug is gone. The
accumulated ones so far, each named for the behaviour rather than the bug:

| Bug | Test |
| --- | --- |
| Triangle back-face culled, nothing on screen | `offscreen_triangle.rs` pixel assertions |
| `ResizeBuffers` called with the old back buffers still referenced | `overlay_survives_many_frames_and_a_resize` |
| Device released before the swap chain (field order) | `d3d12_init.rs` teardown tests |
| Resize storm flooding the event queue | `coalesces_consecutive_resizes` |
| Auto-repeat re-triggering a toggle | `auto_repeat_holds_the_key_without_repeating_the_press` |
| Keys stuck down after alt-tab | `losing_focus_releases_everything_without_firing_release_logic` |
| Negative cursor coordinates read as ~65000 | `unpacks_cursor_positions_including_negative_ones` |
| A frame's scratch memory accumulating across frames | `frame_memory_is_reclaimed_every_frame` |
| A panicking job killing its worker | `a_panicking_job_is_reported_and_the_pool_survives` |

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

`crates/engine-renderer/tests/gpu_integration.rs` is the full automated GPU integration
test built on that module: device, offscreen target, known triangle, readback, and a pixel
count checked against an expectation **derived from the triangle's vertices** rather than
hardcoded. It prints expected vs actual and PASS/FAIL.

### A skip is not a pass

Every GPU test returns green on a machine with no D3D12 adapter, after printing `SKIP:`.
That keeps the suite runnable on hardware the engine does not target, but it means a
"tests passed" result from an unknown machine does **not** prove the GPU path works. CI
that matters must run on a GPU runner and check that no `SKIP:` line appeared.

## Packaging check

`tools\package.ps1` builds the release binary, copies it to `dist\mygame\`, and then
*verifies* the result: exactly one executable, no file whose name resembles a test or
benchmark artefact, no test symbols in the binary, and a two-frame smoke run. Cargo already
makes this true by construction, but a future build script, feature or copy step could
break it silently, so it is checked rather than assumed.

## Manual verification

Some behaviour cannot be asserted automatically: that a window actually appears, that
it resizes without artefacts, that a triangle is visible, that the debug overlay is
legible, that input feels connected. `docs/manual-tests.md` is the checklist, with an ID,
purpose, steps and expected result per entry. Per-milestone Phase 0 steps remain in
`docs/phase0.md`.

A SKIPPED manual test is not a pass. Two entries need hardware not every machine has
(a second monitor, a machine without a D3D12 adapter), and they stay SKIPPED until someone
runs them on that hardware.
