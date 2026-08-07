# myGame Engine

Custom Rust + Direct3D 12 engine for a future third-person PvE combat game.
Windows 10/11 only, MSVC toolchain, `windows-rs` bindings, no third-party game engine.

**Current state: Phase 3 complete — a playable local prototype.** A character in a training
arena that moves, jumps, targets, casts ten abilities with cooldowns and costs, damages
training dummies with single-target, damage-over-time and area synergy mechanics, and
reports it in a developer combat meter. No networking.

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
--dummies <count>    training dummies in the group (default 4)
```

Controls:

| Key | Action |
| --- | --- |
| W A S D | move, relative to the camera |
| SPACE | jump |
| 1 / 2 / 3 | Fireball / Ignite / Detonate |
| 4 – 0 | placeholder abilities |
| TAB | cycle target |
| R | reset the dummies and the combat meter |
| LEFT CLICK / ESC | capture / release the mouse for camera control |
| MOUSE WHEEL | camera distance |
| F5 | show or hide the developer overlay |
| F9 | wireframe on/off |

The developer overlay shows FPS, CPU and GPU frame time, simulation and extraction timing,
the damage meter (total, DPS, by ability, by target) and the ability bar with cooldowns.

Enemies within 40 metres carry a nameplate — name, health percentage and active debuffs —
with a white outline on the current target. Nameplates are game UI and stay visible when the
statistics overlay is hidden with F5. It ships
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
cargo run --release --bin game_bench
```

`performance_test` compares one draw call per object against a single instanced draw,
rendering offscreen so the numbers measure the renderer rather than the compositor.
`game_bench` measures the gameplay pipeline — simulation, combat, extraction, culling — with
no device at all, and counts every allocation while it does. Both live in their own crate,
so neither is ever part of the game executable.

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
  engine-render-data/ what the game hands the renderer: RenderWorld, camera, lighting,
                      handles. Pure data — no windows, no D3D12
  engine-game/        the gameplay simulation: entities, movement, combat, abilities,
                      statuses, events, telemetry. engine-core only — no D3D12, no windows
  engine-presentation/ gameplay to rendering: extraction, third-person camera, HUD text
  engine-renderer/    DXGI/D3D12 device, swap chain, command recording (no gameplay)
  engine-app/         the game executable (bin: mygame)
  engine-bench/       benchmarks (bins: performance_test, cpu_bench, cull_bench,
                      game_bench) — never shipped
  engine-showcase/    visual test bed for the renderer (bin: showcase) — never shipped
docs/                 architecture, phase logs, testing, performance, memory, threading
tools/                package.ps1 — builds and verifies a clean release distribution
```

## Documentation

* [docs/architecture.md](docs/architecture.md) — layering and technology decisions
* [docs/phase0.md](docs/phase0.md) — Phase 0 log: Rust/D3D12 spike
* [docs/phase1.md](docs/phase1.md) — Phase 1 log: engine core
* [docs/phase2.md](docs/phase2.md) — Phase 2 log: the D3D12 renderer
* [docs/phase3.md](docs/phase3.md) — Phase 3 log: gameplay, combat and the prototype
* [docs/testing.md](docs/testing.md) — test categories and how to run them
* [docs/manual-tests.md](docs/manual-tests.md) — manual verification checklist
* [docs/performance.md](docs/performance.md) — measurement methodology and results
* [docs/memory.md](docs/memory.md) — allocation policy and allocator design
* [docs/threading.md](docs/threading.md) — worker threads, job system, measured scaling
