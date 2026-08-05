# Testing

Testing is a first-class requirement. Tests must protect existing behaviour from
regressions, and no test code may end up in the shipped game executable.

## Categories

| # | Category | Location | Command |
| --- | --- | --- | --- |
| 1 | Unit tests | `#[cfg(test)]` modules inside crate sources | `cargo test --workspace --lib` |
| 2 | Integration tests | `crates/<crate>/tests/*.rs` | `cargo test --workspace --test '*'` |
| 3 | GPU/rendering integration tests | `crates/engine-renderer/tests/` (M9) | `cargo test -p engine-renderer` |
| 4 | Performance benchmarks | `crates/*/benches/`, `engine-bench` binary (M6+) | `cargo bench`, `cargo run --release --bin performance_test` |
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

GPU integration tests (Milestone 9) require a D3D12 capable adapter on the machine
running them. On a machine without one — a CI container, for example — they must skip
with a clear message rather than fail. They render offscreen and never open a window.

## Manual verification

Some behaviour cannot be asserted automatically: that a window actually appears, that
it resizes without artefacts, that a triangle is visible, that the debug overlay is
legible. Each milestone in `docs/phase0.md` lists its manual verification steps.
