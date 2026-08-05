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
```

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
  engine-renderer/    DXGI/D3D12 device, swap chain, command recording (no gameplay)
  engine-app/         the game executable (bin: mygame)
docs/                 architecture, phase 0 log, testing, performance
```

## Documentation

* [docs/architecture.md](docs/architecture.md) — layering and technology decisions
* [docs/phase0.md](docs/phase0.md) — milestone-by-milestone log
* [docs/testing.md](docs/testing.md) — test categories and how to run them
* [docs/performance.md](docs/performance.md) — measurement methodology and results
