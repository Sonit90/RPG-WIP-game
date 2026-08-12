# Architecture

## Layering

```
Game            (executable, wiring only)
  |
Input -> Commands          (engine-game::input_map, engine-game::command)
  |
Gameplay / Simulation      (engine-game, fixed 60 Hz, no D3D12)
  |
Render extraction          (engine-presentation)
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
| `engine-render-data` | What the game hands the renderer. Pure data, no `windows`. | M15 |
| `engine-showcase` | Development-only visual test bed. Nothing depends on it. | M15 |
| `engine-collision` | Collision shapes, the static collision world and the character controller. `engine-core` only. | Phase 5 |
| `engine-game` | The gameplay simulation. `engine-core` and `engine-collision` only — no renderer, no `windows`, not even render data. | Phase 3 |
| `engine-environment` | The level as data: placements, collision primitives, lighting, spawn points. `engine-core` only. | Phase 2 (environment) |
| `engine-presentation` | Gameplay to rendering: extraction, the third-person camera, HUD text, the VFX director. | Phase 3 |
| `engine-vfx` | Effects as data and the runtime that plays them: catalogue, instances, particles. `engine-core` and `engine-render-data` only — no gameplay, no device. | Phase 3 (VFX) |

`engine-platform` hands the renderer an opaque window handle and a client size.
It does not link D3D12; the renderer does not create windows.

### Dependency graph

```text
                        engine-core
                       /     |      \        \
                      /      |       \        \
        engine-platform      |   engine-render-data
                      \      |      /  ^        ^
                       \     |     /   |        |
                     engine-renderer   |    engine-game
                             ^         |        ^
                             |         |        |
                             |    engine-presentation
                             |              ^
                             \             /
                              engine-app
```

`engine-render-data` depends on **`engine-core` and nothing else**. That absence is the
point of the crate and it is checkable:

```
cargo tree -p engine-render-data
engine-render-data
└── engine-core
    └── log
```

Until the Milestone 15 checkpoint, `RenderWorld`, the handle types, `Camera` and `Lighting`
lived in `engine-renderer` beside the D3D12 pipeline objects. Anything that wanted to
*build* a world therefore had to link D3D12 — including the gameplay simulation of Phase 3,
which must not. Splitting the data out is what makes this pipeline real:

```text
Input -> Commands -> Simulation -> RenderWorld -> Renderer -> GPU
                                   ^^^^^^^^^^^
                                   engine-render-data
```

**`engine-render-data` must never gain a `windows` dependency.** If it does, the seam is
gone and gameplay is coupled to D3D12 again.

### The frame pipeline (Phase 2, M16)

```text
RenderWorld          engine-render-data, outlives the frame
    |
    | Culler::cull   one pass over bounds; survivors gathered as they are found
    v
RenderList           engine-renderer, rebuilt every frame, allocation reused
    |
    | draw_render_list
    v
D3D12 command list -> GPU
```

`Renderer::render_world` is `build_render_list` followed by `draw_render_list`, and the two
halves are separable on purpose: the first is pure CPU and touches no D3D12 object, the
second reads only the list and the GPU resources it names. Recording never looks at the
`RenderWorld`.

`RenderList` is four parallel arrays — entity, transform, mesh handle, material handle — 64
bytes an entry. It carries the `Transform` rather than the composed `Mat4`; storing the
matrix was built first and measured **14% slower** at 10,000 objects, because composing it
in the recording loop hides under the per-draw driver calls and composing it in a tight
gather loop does not. See the module docs for the measurement.

What stayed in `engine-renderer` and why:

| Type | Why it did not move |
| --- | --- |
| `Culler` | Pure CPU, but visibility is the renderer's policy, not data the game authors. |
| `FrameConstants`, `ObjectConstants` | Plain data, but they are the CPU mirror of an HLSL `cbuffer`. Shader ABI belongs beside the shader. |
| `MeshData`, `MaterialDesc` | Asset-authoring data, not scene data. Candidates for a future asset crate. |

## The environment (Phase 2, environment)

```text
                    EnvironmentScene            engine-environment
                           |                     (engine-core only)
                           v
                  Environment Objects
                           |
             +-------------+-------------+
             v             v             v
           Mesh         Material      Collision
      (engine-assets, procedural)   (engine-game ->
             |             |         engine-collision)
             +------+------+
                    v
               RenderWorld slots        engine-presentation
                    |                   EnvironmentPlacer
             +------+------+
             v             v
          Culling          LOD
             |             |
             +------+------+
                    v
                 Renderer -> GPU
```

`engine-environment` depends on **`engine-core` and nothing else**, for the same reason
`engine-render-data` does and it is checkable the same way. Three crates that are forbidden
from knowing about each other have to read the same placement list: `engine-game` may not
name a mesh, `engine-assets` may not name a collision world, and `engine-presentation` must
not be where content lives. So the description sits below all three.

**It must never gain `windows`, `engine-renderer`, `engine-render-data`, `engine-assets` or
`engine-game`.** If it does, the level stops being data that three consumers read and becomes
a fourth thing coupled to all of them.

Four rules the environment follows, each a boundary rather than a convention:

* **Static objects are spawned once and never extracted.** `RenderExtractor::extract` walks
  gameplay slots and rebuilds them every frame, because a dummy moves and dies. A rock does
  neither, so it is spawned into the same `RenderWorld` at load and costs the per-frame path
  nothing but its share of a cull. The only thing that changes is its level of detail.
* **Instancing is sharing, not a system.** Each asset is uploaded once; every placement refers
  to the handles that upload returned; the render list's counting sort finds them contiguous;
  each `(mesh, material)` pair becomes one `DrawIndexedInstanced`. A thousand of the same rock
  costs a thousand transforms and no GPU resources. Measured: 5.5x the objects, one extra draw.
* **Collision is chosen, not derived.** An `EnvironmentCollision` is `None` or one box, in the
  model's local space, carried through the placement's transform. A pillar is 48 triangles and
  one box; a fern is 60 triangles and nothing. There is no path by which a visual mesh becomes
  a collision mesh.
* **Lighting is content.** `EnvironmentLighting` lives with the scene and is copied into
  `engine_render_data::Lighting` by `engine_presentation::scene_lighting`, which is the only
  place in the project that knows both types.

See `phase_environment.md` for the layouts, the measurements and the two scalability answers.

## Gameplay (Phase 3)

```text
InputState -> PlayerCommand -> Simulation -> CombatEvent -> CombatMeter
                                   |
                                   +-> GameWorld -> extraction -> RenderWorld
```

`engine-game` depends on **`engine-core` and nothing else** — deliberately not
`engine-render-data` either. It cannot name a `MeshHandle`, so it cannot start owning one,
and `crates/engine-game/tests/headless_simulation.rs` runs the whole pipeline with no
device. Checkable:

```
cargo tree -p engine-game
engine-game
├── engine-core
│   └── log
└── log
```

`engine-presentation` is the only crate that knows both sides. It decides that a dummy is a
scaled cube in a particular colour, which is a decision neither neighbour can make:
`engine-game` has no meshes and `engine-render-data` has no health.

### Fixed timestep

Gameplay advances in whole 60 Hz ticks from an accumulator
(`engine_core::time::FixedTimestep`); the renderer runs at whatever rate it reaches. Nothing
in gameplay reads the frame delta. A simulation tied to the frame rate gives a player at
1400 FPS a different jump arc than one at 240 FPS, makes cooldowns frame-rate dependent, and
makes a recording unreproducible. Catch-up is bounded at eight ticks per frame so a stall
cannot spiral.

The camera is the exception and is presentation: it is smoothed at the *render* rate, which
is what stops a 60 Hz simulation from looking like 60 FPS.

### Commands and events

Two seams that exist for what comes after Phase 3:

* **Commands** — `engine-game::input_map` is the only code in the project that knows which
  key does what. The simulation consumes `PlayerCommand`, so a replay, an AI or a network
  client drives it identically.
* **Events** — every combat state change is announced as a `CombatEvent`. The damage meter
  reads a slice of them; nothing in the ability system mentions telemetry. VFX, a combat log
  and replication are further readers of the same slice.

## Combat feedback (Phase 3, VFX)

```text
Simulation ─ CombatEvent ─▶ VfxDirector ─▶ VfxSystem ─┬─▶ ParticleDrawList ─▶ particle pass
                (observe)    (decides)                └─▶ MeshDraw ─▶ RenderWorld entities
```

Three layers, and the split is the same one the rest of the engine uses:

* **`engine-game`** announces what happened — a bolt's line, an impact's radius and delay, an
  area's `ZoneShape` and duration, a hit's position and whether it crit. It knows nothing
  about effects and calls nothing.
* **`engine-vfx`** owns what an effect *is*: a static catalogue of thirteen effects, an
  instance lifecycle with generation-checked handles, and a fixed-capacity CPU particle pool.
  It depends on `engine-core` and `engine-render-data` and **must never gain `engine-game`,
  `engine-renderer`, `engine-assets` or `windows`** — the first absence is what stops an
  effect from reaching into the simulation, the rest are what keep the runtime testable with
  no device.
* **`engine-presentation`** is the only layer allowed to say "this ability looks like that",
  exactly as it is the only layer allowed to say "a dummy is a scaled cube in this colour".

Effects reach the screen two ways. Particles go through a dedicated blended pass — one
`DrawInstanced` per blend mode, a 36-byte record per particle, quads expanded in the vertex
shader from the camera basis, depth tested and never written. Geometry goes into the same
`RenderWorld` everything else uses, so it is culled, batched and instanced with the scene;
colour is per instance (`InstanceStyle`), so one material serves every effect.

**Gameplay is authoritative and the dependency graph enforces it.** Deleting `engine-vfx`
would leave a game that plays identically with nothing to look at. Every cap in the system —
128 instances, 16,384 particles — degrades by dropping visuals and counting the drop.

## Enemies and encounters (Phase 6, enemy AI)

```text
Encounter ──▶ Enemy (an entity with a brain)
                 │
             AI  │  perceive ─▶ think ─▶ AiDecision { intent, request }
                 ▼
            Simulation ── movement, the ability, the telegraph, the damage
                 │
            CombatEvent ──▶ VFX · audio · UI · the damage meter
```

The layering is the phase's whole point, and it is enforced by where the code lives:

* **`engine-game/src/enemy/brain.rs`** decides *what an enemy wants*. It is handed an
  `AiPercept` — its own position, whether it is alive, its target and the nearest hostile —
  and returns an `AiDecision`. It cannot see the world, move a body, deal damage or emit an
  event, and it has no randomness in it at all.
* **`engine-game/src/sim/enemy_exec.rs`** decides *what happens*. It perceives, calls the
  brain, parks the movement intent for the character controller, and validates and executes
  what was asked for — through the same `Simulation::deal_damage` every player ability goes
  through. **An enemy has no privileged path to anybody's health.**
* **`engine-presentation`** decides *how it looks and sounds*, by observing the same event
  stream everything else observes. No AI code names a particle, a voice or a widget.

Four things are data rather than code, which is what makes a new enemy or a new attack a
table row:

| Table | Where | What a new row buys |
| --- | --- | --- |
| `ARCHETYPES` | `enemy/mod.rs` | a new enemy type: health, size, speed, aggro, leash, preferred range, and its ability list |
| `ENEMY_ABILITIES` | `enemy/mod.rs` | a new attack: range, cast time, cooldown, damage, shape, aim, interruptibility, recovery |
| `TelegraphShape` | `enemy/mod.rs` | the *hit test*, which is also what the telegraph draws |
| `EnemySpawn` | `encounter.rs` | who is in a fight and where they stand |

**The telegraph and the damage are one value.** `TelegraphShape` is resolved once, when a
cast starts — including the cone's yaw — stored on the cast, carried by the
`TelegraphStarted` event, and used again by the hit test when the cast lands. There is no
second shape to drift, and `engine-game/tests/encounter.rs` asserts that every hit lands
inside the shape that was announced.

**Threat is a table, not a field.** Each enemy holds up to eight rows; damage adds threat in
the one place damage is applied; the target is the highest-threat row the *world* still
accepts as a victim. With one player it always names the player — and with a pet, a second
player or a taunt it will not have to be rewritten.

## Combat audio (Phase 4, audio)

```text
Simulation ─ CombatEvent ─▶ AudioDirector ─▶ AudioSystem ─▶ stereo frames ─▶ cpal ─▶ WASAPI
                (observe)     (decides)      (voices, mixer, listener)
```

The same three layers as the effects, deliberately in the same shape:

* **`engine-game`** announces what happened and is unchanged by this phase. Not one gameplay
  file mentions a sound, and no event was added for audio: the stream that already carried a
  bolt's line, an impact's radius and delay, an area's shape and a hit's crit flag carries
  everything a sound needs, position included.
* **`engine-audio`** owns what a sound *is*: a static catalogue of 39 sounds, the bank of PCM
  behind them, a fixed pool of 64 voices with generation-checked handles, the listener model
  (inverse-distance attenuation, constant-power panning), the mixer's four volumes, the
  per-sound limits and the priority model. It depends on `engine-core` and **must never gain
  `engine-game`, `engine-renderer`, `cpal` or `windows`** — the first absence stops a sound
  reaching into the simulation, the rest keep the whole runtime, mix included, testable with
  no audio hardware.
* **`engine-presentation`** is the only layer allowed to say "Detonate sounds like an
  explosion", exactly as it is the only layer allowed to say what one looks like.

The device is `cpal` in `engine-app`, and it is a device wrapper rather than an engine: it
opens a stream and asks for blocks of samples, and the mix that fills them is ours. The
runtime lives behind one mutex — the game thread holds it for the frame's cues, the callback
for the mix.

**Sounds are authored in code**, like every other asset here. One synthesiser (filtered noise
+ a swept sine + sparse impulses, times an envelope) and a table of parameters produce the
whole bank at load: 8.5 MiB in ~100 ms at 48 kHz. A WAV dropped into `assets/audio/<folder>/`
replaces any sound without a code change.

**Gameplay is authoritative and the dependency graph enforces it.** Deleting `engine-audio`
would leave a game that plays identically in silence. A machine with no sound card, a
malformed WAV and an empty bank all resolve the same way: cues are counted, nothing is
played, and the fight is unaffected. Every cap — 64 voices, per-sound caps, minimum intervals
— degrades by dropping sound and counting the drop.

Three rules the audio layer keeps that are worth stating separately:

* **One listener, and it is the camera.** `AudioDirector::set_listener` copies the gameplay
  camera's position and basis every frame. There is no second camera to drift.
* **Timing follows the picture, not the other way round.** A bolt's impact plays when the
  *cosmetic* bolt lands, using the same three constants `VfxDirector` flies it with.
* **A burn is not fifty hits.** Damage credited to Ignite plays no impact at all, and the
  burning bed is capped at four voices however many targets are alight.

## The user interface (Phase 4)

```text
Simulation --> UiSnapshot --> GameUi --> UiDrawList --> UiBatcher --> one draw call
              (plain data)   (layout)   (commands)     (vertices)
```

Four rules, each a boundary rather than a convention:

* **The UI reads a `UiSnapshot`, never `&Simulation`.** A snapshot is taken once per frame at
  a defined point; a borrow would let two parts of the layout read different answers, and it
  would disagree with interpolated geometry about where things are. A future network client
  has no `Simulation` to borrow — it has a stream of state, which is this shape.
* **The layout emits commands, not vertices.** `UiDrawList` lives in `engine-render-data`, so
  `engine-presentation` builds the entire HUD without linking D3D12. Expanding a string into
  glyph quads needs the UI atlas, which lives beside the texture it is uploaded into, so a
  command list is what keeps the shader ABI on the renderer's side of the seam. An icon is
  named (`UiIcon::Meteor`) rather than described, for the same reason.
* **Everything is one batch.** Text, icons and solid shapes sample the same UI atlas — glyphs
  their own cell, icons theirs, rectangles one opaque texel — so the whole interface is one
  vertex buffer through one pipeline state. Measured: 20,990 quads, one `DrawInstanced`.
  The atlas is RGBA8 in bands: the font on top, then one band per icon size. The pixel shader
  picks a point or a linear sampler by comparing `v` against the font/icon boundary, so the
  two kinds of art get the filtering each needs without a per-vertex flag or a second draw.
* **The interface is laid out in logical pixels.** Every layout constant is the size the UI
  was designed at; a `UiScale` from the player's settings converts it to physical pixels once
  per scale change, into a `HudMetrics`/`MenuMetrics`/`TalentMetrics`. Nothing multiplies at
  a use site, so a measurement cannot be scaled twice or forgotten, and the hit test reads
  the same metrics the draw did. Glyph scales snap to whole numbers, because the font is a
  bitmap that is only crisp at integer sizes.
* **Icons are rasterized at build time, at every size they can be drawn.** `build.rs` renders
  each SVG at 132 and 66 texels; the batcher picks the cell nearest the rectangle it is
  filling, so an icon is always minified and never magnified. That is what a mip chain would
  have done, except a mip chain of an atlas would blend glyphs into icons — so the selection
  happens on the CPU, where the draw size is already known, and the texture needs no mips.
* **Painter's order, not depth.** The UI pass neither tests nor writes depth. A depth test and
  alpha blending cannot coexist: a blended fragment that fails the test vanishes, and one that
  passes writes depth that occludes whatever should have blended over it.

The developer *statistics* overlay is not part of this and still uses the Phase 3 per-glyph
path. It is a debug readout behind the `debug-overlay` feature; the game UI ships either way.

### Interpolation

`GameWorld` records every body as it was at the start of the current tick. Presentation blends
the two by `FixedTimestep::alpha`, so a 60 Hz simulation draws smoothly at any frame rate.
Recording where something *was* is a simulation fact; blending is presentation's job, and the
simulation is never itself interpolated. The result lags the simulation by up to one tick,
which is deliberate — extrapolating would overshoot and snap back on every direction change.

### Per-instance visual state

A material describes a *surface* and is shared between entities; `InstanceStyle` — a colour
multiply and an emissive add — describes what is happening to **one** entity right now. It
rides in the per-instance vertex stream, so two entities with different tints still share a
batch, and changing one is a store rather than a render-entity respawn.

### What gameplay must never put in `RenderWorld`

Health, cooldowns, statuses, resources, factions, targets. `RenderWorld` carries transforms,
mesh and material handles, bounds and visibility, and there is nowhere in it for the rest to
go — which is enforced by the crate split rather than by review.

## Engine lifecycle (Phase 1, M1)

`engine-core::application` owns the lifecycle and the frame loop:

```
Engine::new() -> initialize() -> run() -> shutdown()
```

```
process_events() -> update() -> render() -> present() -> end_frame()
```

`engine-core` cannot depend on `engine-platform` or `engine-renderer` (see Layering), so
the seam is the `Application` trait: `engine-app` implements it over the window and the
renderer, and `Engine<A: Application>` drives it.

### Why a generic and not `Box<dyn Application>`

Monomorphization turns the six per-frame hooks into direct calls — no vtable, no boxed
application, nothing allocated per frame. A runtime-swappable application has no use case
in a single-game engine, so the flexibility of `dyn` would be paid for and not used.

### Why the loop moved out of `engine-app`

Simulation and rendering will eventually run at different rates. That is a property of the
loop, so it belongs where the engine can change it once rather than in each executable
(game, benchmark harness, future tools). `EngineContext` is the only channel: it carries
the frame delta, frame index, elapsed time and per-stage CPU timings, and it is where the
frame allocator, input state and profiler will live in later milestones.

### Loop invariants

* Exit is requested via `EngineContext::request_exit`, never by returning from a stage.
  The current frame always completes, so the renderer never exits with a recorded but
  unpresented frame in flight.
* A stage returning `Err` stops the loop immediately; `end_frame` does not run for that
  frame, and `shutdown` always does.
* `Application::is_render_ready() == false` skips `render` and `present`; their stage
  timings then read zero. A minimized window is the motivating case.
* The first frame's delta is zero: no frame has completed yet. Simulation code must
  tolerate that rather than dividing by it.
* Construction (window, D3D12 device) happens in `App::new`, not in `initialize`. Both are
  values the application cannot exist without, and holding them as `Option` to permit a
  two-phase init would add a check to every use for no gain. `initialize` exists for work
  that needs a running engine context.
* The clock is reset after `initialize` returns: device creation and shader compilation
  take hundreds of milliseconds and are not frame time.

## Time (Phase 1, M2)

`engine-core::time::Time` carries two timelines and one frame counter:

| | Source | Clamped | Scaled | Stops on pause | Used by |
| --- | --- | --- | --- | --- | --- |
| Real | `Instant` (QPC) | no | no | no | frame statistics, profiling, benchmarks |
| Simulation | real delta | yes | yes | yes | gameplay, animation, abilities |

* **Monotonic only.** `Instant` cannot move backwards; wall-clock time is never used for
  frame timing, so an NTP step or a user changing the system clock cannot produce a
  negative or enormous delta.
* **The simulation delta is clamped** to `DEFAULT_MAX_DELTA` (250 ms). A longer frame means
  the process was stalled — a breakpoint, a window drag, a driver reset. Passing that
  through makes bodies teleport through walls and ability timers fire in a burst. Real time
  still records the true value, so a stall stays visible in the statistics.
* **Performance numbers must read real time.** `App::end_frame` records
  `time().real_delta()`; using the simulation delta would make a paused game look infinitely
  fast.
* **`advance_by` exists for determinism, not only for tests.** Driving `Time` from supplied
  deltas rather than from this machine's clock is what a recorded replay — and later a
  network authority — needs. Tests use the same door to exercise clamping and scaling
  without sleeping.
* **Fixed timestep is not implemented.** The shape supports it: a fixed-step loop
  accumulates `delta()` and consumes it in fixed increments, and since the simulation only
  ever reads `Time`, nothing outside the loop changes.

## Frame metrics (Phase 1, M3)

`engine-core::metrics::FrameMetrics` owns CPU and GPU frame-time history, per-stage
averages, and the `FrameSummary` that both the log reports and the debug overlay read.
The engine records one sample per frame; nothing else has to.

The split that matters is **record vs. summarize**:

| | Cost | When |
| --- | --- | --- |
| `record` | a few stores into pre-allocated buffers | every frame |
| `refresh` | sorts both windows, `O(n log n)` | every 250 ms |

At 5000 FPS a per-frame sort of a 1024-sample window would cost more than the frame it
measures. Readers call `summary()`, which returns the cached snapshot and never sorts, so
adding a reader cannot make the frame slower. `FrameSummary` is `Copy`, so a reader gets a
consistent snapshot rather than a half-updated view.

Consequences worth knowing:

* Stage averages cover the frames since the last *refresh* (250 ms), not since the last
  report. A one-second report line therefore shows the most recent quarter-second of stage
  times, not the whole second.
* CPU and GPU are separate windows. GPU timestamps lag the CPU by a few frames and are
  absent right after startup or a resize, so a missing GPU sample is skipped rather than
  recorded as zero — averaging in zeros would understate GPU time.
* The final report calls `refresh()` explicitly: the cached summary can be up to 250 ms
  old, and the closing numbers should cover the whole window.
* `EngineContext::reset_clock` clears the history too. Frame times from the previous stress
  level describe a different workload.

## Input (Phase 1, M4)

```
Windows messages -> engine-platform -> InputEvent -> InputState -> application
```

`engine-core::input` defines `KeyCode`, `MouseButton`, `InputEvent`, `InputState`.
Virtual-key codes, `WPARAM` bit layouts and `WM_*` numbers stop inside `engine-platform`.
Nothing here is a gameplay action: there is no `Jump` and no `Fireball`, because bindings
belong to gameplay, which does not exist yet.

### State *and* transitions

`is_key_down` answers "held", `was_key_pressed` / `was_key_released` answer "changed this
frame". Both are stored, not inferred. Polling alone cannot distinguish them, so every
system that tried to infer transitions would keep its own copy of last frame's state and
get it subtly differently.

The engine calls `InputState::begin_frame()` **before** `process_events`, so a press is
visible for exactly the frame its message arrived in. Rolling the bits afterwards would
make every press look like a hold.

Known limit: a key pressed *and* released within one frame shows neither transition. That
is inherent to frame-granular state, it is tested explicitly, and it is why an event-driven
consumer must read the event stream rather than the state.

### Storage

`InputState` is a bitset (`[u64; 1]` today) plus scalars — no `Vec`, no allocation, and
`Copy`. That last property is deliberate: a deterministic replay records and replays a
frame's input by copying this struct.

### Auto-repeat and focus loss

* Auto-repeat is **marked, not dropped**: `KeyDown { repeat: true }` keeps the key held
  without re-firing a press. The window procedure passes the repeat bit through; consumers
  that want physical presses match on `repeat: false`.
* Losing focus releases every key and button, without reporting releases. Key-up messages
  go to whichever window has focus, so a key held while alt-tabbing away would stay down
  forever; but firing release logic for a dozen keys at once would trigger gameplay the
  player never asked for.
* Consecutive mouse moves coalesce in the queue like resizes do. A 1000 Hz mouse produces
  several `WM_MOUSEMOVE` per frame and only the latest position matters — the delta is
  preserved because `InputState` computes it from the previous position, not from the
  number of events.

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
