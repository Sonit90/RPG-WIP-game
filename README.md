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
  engine-core/        logging, timing, shared utilities (no platform, no renderer)
  engine-platform/    Win32 window, message pump, window events (no GPU types)
  engine-app/         the game executable (bin: mygame)
docs/                 architecture, phase 0 log, testing, performance
```

`engine-renderer` (D3D12 device, swap chain, GPU timing) is added in Milestone 2.

## Documentation

* [docs/architecture.md](docs/architecture.md) — layering and technology decisions
* [docs/phase0.md](docs/phase0.md) — milestone-by-milestone log
* [docs/testing.md](docs/testing.md) — test categories and how to run them
* [docs/performance.md](docs/performance.md) — measurement methodology and results
