# myGame Engine

Custom Rust + Direct3D 12 engine for a future third-person PvE combat game.
Windows 10/11 only, MSVC toolchain, `windows-rs` bindings, no third-party game engine.

**Current state: Phase 4 complete — a playable prototype with a real combat interface.** A
character in a training arena that moves, jumps, targets, casts ten abilities with cooldowns,
costs and cast times, and damages training dummies with single-target, damage-over-time and
area synergy mechanics — all of it visible through a combat HUD: player health and resource
bars, a target frame, world-space enemy health bars, an ability bar with cooldown wipes, a
cast bar and floating damage numbers. The whole interface is **one draw call** and allocates
nothing per frame. No networking, no character models.

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
| 1 – 0 | the current hero's ability bar |
| F / G | the two ability buttons beside the bar, for a hero with more than ten |
| R | the Ultimate: Last Stand, Tranquility — the FireMage's is switched off |
| TAB | cycle target |
| BACKSPACE | reset the dummies, the fight and the combat meter |
| LEFT CLICK / ESC | capture / release the mouse for camera control |
| MOUSE WHEEL | camera distance |
| F5 | show or hide the developer overlay |
| F9 | wireframe on/off |

The combat HUD is always on: player health and resource bars, a target frame, an ability bar
with cooldown wipes, a cast bar, world-space health bars and nameplates above nearby enemies,
and floating damage numbers. It is drawn through a batched UI pass — one vertex buffer, one
texture, **one draw call** — and costs about 12 microseconds of CPU and zero allocations per
frame.

The *developer statistics* overlay is separate and hidden by default: FPS, CPU and GPU frame
time, simulation and extraction timing, and the damage meter broken down by ability and by
target. Toggle it with **F5**.

Only the statistics overlay can be removed from the binary, which is what the `debug-overlay`
Cargo feature is for. The game UI ships either way:

```powershell
cargo build --release --no-default-features
```

A reproducible timing run:

```powershell
cargo run --release --bin mygame -- --frames 60000 --report 1
```

## Level editor

The in-game level editor is behind the `editor` Cargo feature, so a normal build does not
contain it. Build with the feature, then press **F6** in game to enter and leave it:

```powershell
cargo run --bin mygame --features editor
```

Open a level directly in the editor. A `--scene-file` path that does not exist yet starts a
new, empty level that **CTRL+S** saves to that path:

```powershell
cargo run --bin mygame --features editor -- --editor --scene-file assets/scenes/showcase.json
cargo run --bin mygame --features editor -- --editor --scene-file assets/scenes/my_level.json
```

Without `--editor`, `--scene-file` plays the level instead of editing it. Every toolbar
button has a tooltip naming its key, and **F1** opens the full list of shortcuts. See [`docs/level-editing.md`](docs/level-editing.md) for
the full controls and the level format.

## Benchmark

```powershell
cargo run --release --bin performance_test
cargo run --release --bin performance_test -- --instances 1000 --frames 500
cargo run --release --bin game_bench
cargo run --release --bin ui_bench
```

`performance_test` compares one draw call per object against a single instanced draw,
rendering offscreen so the numbers measure the renderer rather than the compositor.
`game_bench` measures the gameplay pipeline — simulation, combat, extraction, culling — with
no device at all, and counts every allocation while it does. `ui_bench` measures the
interface: a CPU phase with no device, a GPU phase against a hidden window, and the real
combat HUD over an arena that has been fought in. All three live in their own crate, so none
is ever part of the game executable.

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
  engine-collision/   collision shapes, the static collision world, the broad and narrow
                      phases and the character controller. engine-core only
  engine-game/        the gameplay simulation: entities, movement, combat, abilities,
                      statuses, events, telemetry, the collision demo scene. engine-core
                      and engine-collision only — no D3D12, no windows
  engine-presentation/ gameplay to rendering: extraction, third-person camera, HUD text
  engine-renderer/    DXGI/D3D12 device, swap chain, command recording (no gameplay)
  engine-app/         the game executable (bin: mygame)
  engine-bench/       benchmarks (bins: performance_test, cpu_bench, cull_bench,
                      game_bench, ui_bench, collision_bench) — never shipped
  engine-showcase/    visual test bed for the renderer (bin: showcase) — never shipped
docs/                 architecture, phase logs, testing, performance, memory, threading
collision.md          the collision phase: design, decisions, benchmarks, limitations
nameplates.md         nameplate settings and the minimal-displacement stacking layout
tuning.md             where every default lives, and how to change one
tools/                package.ps1 — builds and verifies a clean release distribution
```

## Documentation

* [docs/architecture.md](docs/architecture.md) — layering and technology decisions
* [docs/phase0.md](docs/phase0.md) — Phase 0 log: Rust/D3D12 spike
* [docs/phase1.md](docs/phase1.md) — Phase 1 log: engine core
* [docs/phase2.md](docs/phase2.md) — Phase 2 log: the D3D12 renderer
* [docs/phase3.md](docs/phase3.md) — Phase 3 log: gameplay, combat and the prototype
* [docs/phase4.md](docs/phase4.md) — Phase 4 log: batched UI, the combat HUD and feedback
* [docs/testing.md](docs/testing.md) — test categories and how to run them
* [docs/manual-tests.md](docs/manual-tests.md) — manual verification checklist
* [docs/performance.md](docs/performance.md) — measurement methodology and results
* [docs/memory.md](docs/memory.md) — allocation policy and allocator design
* [docs/threading.md](docs/threading.md) — worker threads, job system, measured scaling
