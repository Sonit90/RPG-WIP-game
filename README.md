# myGame Engine

Custom Rust + Direct3D 12 engine for a future third-person PvE combat game.
Windows 10/11 only, MSVC toolchain, `windows-rs` bindings, no third-party game engine.

**Current state: Phase 0 (technical spike). No gameplay.**

## Requirements

* Windows 10 or 11
* Rust stable (MSVC host), installed via `rustup`
* Visual Studio Build Tools with the C++ workload (provides `link.exe`)
* A Direct3D 12 capable GPU

## Build and run

```powershell
cargo build                 # debug build of the whole workspace
cargo run --bin mygame      # run the game executable
cargo build --release       # release build (no test or bench code)
```

Options (`cargo run --bin mygame -- --help`):

```
--width <pixels>     client width (default 1920)
--height <pixels>    client height (default 1080)
--vsync              wait for vertical blank (off by default)
--frames <count>     exit after rendering this many frames
--report <seconds>   frame-time report interval (default 1.0, 0 disables)
--overlay            start with the debug overlay visible
--triangles <count>  stress-test triangle count (default 1)
```

Runtime keys:

| Key | Action |
| --- | --- |
| F1-F4 | stress level 1 / 100 / 1,000 / 10,000 triangles |
| F6 | stress level 100,000 triangles |
| F7 / F8 | step one stress level down / up |
| F5 | show or hide the debug overlay |

The debug overlay shows FPS, CPU and GPU frame time, resolution and adapter name. It ships
in release builds, starts hidden, and is toggled with **F5**. Its cost is below the noise
floor of measurement (see `docs/performance.md`).

It can still be removed from the binary entirely, which is what the `debug-overlay` Cargo
feature is for:

```powershell
cargo build --release --no-default-features
```

A reproducible timing run:

```powershell
cargo run --release --bin mygame -- --frames 60000 --report 1
```

## Benchmark

```powershell
cargo run --release --bin performance_test
cargo run --release --bin performance_test -- --instances 1000 --frames 500
```

Compares one draw call per object against a single instanced draw, rendering offscreen so
the numbers measure the renderer rather than the compositor. Lives in its own crate, so it
is never part of the game executable.

## Checks

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Layout

```
Cargo.toml            workspace root
crates/
  engine-core/        lifecycle, time, input, memory, jobs, profiling, logging, config
  engine-platform/    Win32 window, message pump, input translation (no GPU types)
  engine-renderer/    DXGI/D3D12 device, swap chain, command recording (no gameplay)
  engine-app/         the game executable (bin: mygame)
  engine-bench/       benchmarks (bins: performance_test, cpu_bench) — never shipped
docs/                 architecture, phase logs, testing, performance, memory, threading
tools/                package.ps1 — builds and verifies a clean release distribution
```

## Documentation

* [docs/architecture.md](docs/architecture.md) — layering and technology decisions
* [docs/phase0.md](docs/phase0.md) — Phase 0 log: Rust/D3D12 spike
* [docs/phase1.md](docs/phase1.md) — Phase 1 log: engine core
* [docs/testing.md](docs/testing.md) — test categories and how to run them
* [docs/manual-tests.md](docs/manual-tests.md) — manual verification checklist
* [docs/performance.md](docs/performance.md) — measurement methodology and results
* [docs/memory.md](docs/memory.md) — allocation policy and allocator design
* [docs/threading.md](docs/threading.md) — worker threads, job system, measured scaling
