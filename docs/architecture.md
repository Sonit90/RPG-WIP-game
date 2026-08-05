# Architecture

## Layering

```
Game            (executable, wiring only)
  |
Gameplay / Simulation      (not in Phase 0)
  |
RenderWorld                (data handoff: transforms, meshes, camera, instances)
  |
Renderer                   (D3D12 resources, pipelines, command recording)
  |
D3D12 / DXGI               (windows-rs bindings)
```

Dependencies point downward only. The renderer never learns gameplay concepts —
no `Player`, `Ability`, `Damage`, `Buff`, `Debuff`, `DPS` or `Enemy` type may appear
in `engine-renderer`. The simulation publishes a `RenderWorld` snapshot; the renderer
consumes it. That boundary is what later allows the simulation to run on its own
schedule and, eventually, to be driven by a network authority.

## Crates

| Crate | Purpose | Introduced |
| --- | --- | --- |
| `engine-core` | Logging, timing, shared error types. No OS, no GPU. | M0 |
| `engine-app` | The `mygame` executable. Wires platform + renderer. | M0 |
| `engine-platform` | Win32 window, message pump, input events. No GPU types. | M1 |
| `engine-renderer` | DXGI/D3D12 device, swap chain, pipelines, GPU timing. | M2 |

`engine-platform` hands the renderer an opaque window handle and a client size.
It does not link D3D12; the renderer does not create windows.

## Deviations from the originally proposed repository layout

1. **The workspace lives at the repository root** rather than in a nested
   engine directory. The repository already exists for this project,
   so a nested directory would add a level with no content of its own.
2. **No top-level `tests/` and `benches/` directories.** In a virtual Cargo workspace
   these are inert: Cargo only discovers `tests/` and `benches/` inside a package.
   Integration tests therefore live in `crates/<crate>/tests/` and benchmarks in
   `crates/<crate>/benches/`. This also enforces the "no test code in the shipped
   executable" rule for free, since neither target type is built by
   `cargo build --release --bin mygame`.
3. **Placeholder crates are created when they get their first real code**, not up front.
   Empty crates cannot be built, tested or linted in a meaningful way, and they make
   `cargo clippy --workspace` output misleading.
4. `tools/` and `assets/` are created when the first tool or asset exists.

## Technology decisions

### Why Rust

* Predictable, no-GC performance with a memory-safety model that survives a large
  codebase — the failure mode of a hand-written C++ engine at this scale is lifetime
  bugs in resource management, exactly what the borrow checker covers.
* `unsafe` is explicit and greppable, so the risky surface (the D3D12 FFI boundary)
  stays auditable instead of being spread through the whole codebase.
* First-class tooling out of the box: `cargo test`, `cargo bench`, `clippy`, `rustfmt`.
  Testing is a first-class requirement for this project, and Rust does not require a
  build-system project to get it.
* Data-oriented layouts (`Vec<T>` of plain structs, slices, explicit indices) are
  natural, which matters for the eventual instanced/batched renderer.

### Why Direct3D 12

* Windows is the only launch target, so a cross-platform abstraction would be paid for
  and not used in Phase 0.
* Explicit command lists, descriptor heaps and fences are what make a 240–480 FPS budget
  measurable: the CPU cost of a frame is visible instead of hidden in a driver runtime.
* GPU timestamp queries give real GPU frame time, which D3D11 exposes far less directly.
* Multi-threaded command recording is available when the draw-call count grows.
* `windows-rs` provides maintained, generated bindings, so no hand-written FFI is needed.

### Why not an existing engine

Unreal, Unity, Godot and Bevy all impose a frame architecture and an update model. The
performance targets here (a stable 240 FPS floor, 360 FPS target) are a property of the
frame architecture, so owning it is the point of the project.

## Error handling policy

* Engine crates propagate `Result`. `unwrap()`/`expect()` are denied by a workspace lint.
* Unrecoverable initialization failures produce a message naming the failing D3D12/DXGI
  call and its `HRESULT`, then exit with a failure code.
* Every `unsafe` block carries a `// SAFETY:` comment stating why `unsafe` is required
  and which invariant makes it sound.

## Allocation policy

No custom allocator in Phase 0. The rules that keep a future frame allocator possible:

* No heap allocation in the per-frame render path where practical.
* Per-frame buffers are allocated once at init and reused.
* Every known per-frame allocation is documented in `docs/performance.md`.
* Logging allocates (message formatting), so it is banned from the hot path; frame
  metrics are accumulated into plain counters and logged on an interval.
