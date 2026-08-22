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
| 6 | Gameplay benchmarks | `crates/engine-bench` | `cargo run --release --bin game_bench` |
| 7 | Manual verification | `docs/manual-tests.md` | human |

Everything in categories 1–3 runs under `cargo test --workspace`.

## Gameplay tests need no GPU, and that is enforced

`engine-game` depends on `engine-core` alone, so every gameplay, combat, ability, targeting
and telemetry test runs with no device, no window and no adapter — they cannot be skipped
the way a GPU test can, and they run in milliseconds.

`crates/engine-game/tests/headless_simulation.rs` exists to make that structural rather than
incidental: it is a separate integration target linking only `engine-game` and
`engine-core`, so a change that makes the simulation depend on the renderer stops it
compiling instead of quietly making the suite need a GPU.

`crates/engine-game/tests/determinism.rs` compares fixed-point snapshots of two runs of the
same command script, and the event log of both. Floating-point state compared loosely is
what makes a determinism test useless, so it is compared exactly.

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
| A cooldown that never expires because 1/60 s is inexact in `f32` | `a_cooldown_runs_down_at_the_simulation_rate_and_stops_at_zero` |
| A six-second burn ticking five times for the same reason | `a_status_ticks_once_a_second_for_its_whole_duration` |
| One jump press becoming eight jumps in a catch-up batch | `a_jump_command_lifts_the_character_once_per_press` |
| Damage applied to the world but missing from the meter | `the_full_rotation_runs_and_the_meter_agrees_with_the_health_lost` |
| A new entity inheriting a dead one's render entity or damage row | `a_reused_gameplay_slot_gets_its_own_render_entity`, `a_reused_slot_gets_its_own_row_rather_than_inheriting_one` |

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

## Visual verification that survives

Some visual facts *can* be asserted, and where they can, they are, rather than being looked
at once on one afternoon. `engine-renderer/tests/character_render.rs` and
`environment_render.rs` render offscreen through the same pipeline, root signature and depth
state the window uses, read the pixels back, and assert on them: that the model reaches the
screen, that posing moves it, that the arena has a sky above and ground below and they
differ, that the boundary changes the picture from the middle of the fight, that removing
the background removes depth, and that every asset in the environment library draws more
than a couple of hundred lit pixels. Each of them skips itself when no D3D12 adapter is
present.

These do not replace a person's judgement about whether something looks *good*. They replace
the part of a visual check that is a fact — is it there, is it the right size, is it lit,
is it inside out — and that is the part that regresses silently.

## VFX tests (Phase 3, VFX)

The VFX system is tested at four levels, and only one of them needs a GPU:

| Level | Where | Needs a device |
| --- | --- | --- |
| The event layer gameplay emits | `engine-game/tests/presentation_events.rs` | no |
| The catalogue, instance lifecycle and particle pool | `engine-vfx` unit tests | no |
| The draw list and the pipeline's state | `engine-render-data`, `engine-renderer` unit tests | no |
| The pass, in pixels | `engine-renderer/tests/particle_render.rs` | yes, skips without one |
| The director, and every ability | `engine-presentation` unit tests + `tests/ability_vfx.rs` | no |

The rule the rest of this document states — gameplay tests need no GPU — holds for effects
too, and for a stronger reason: `engine-vfx` cannot link a device at all, so "the particle
pool integrates correctly" is a question that is *answerable* without one by construction.
The seven pixel tests exist because the things they check (a premultiplied blend with the
wrong factors, a billboard basis taken from the wrong axis, a per-vertex input layout where
a per-instance one was meant) all produce a healthy CPU state and a black frame.

`engine-presentation/tests/ability_vfx.rs` casts every ability on the bar in a real fight and
asserts that each produces an effect, that an area is drawn at the radius it hits, that a
rapid rotation never exhausts the pools, and that effects survive their targets dying.

## Shadow tests (Phase 7, shadows)

Shadows are tested at four levels, and two of them need a GPU:

| Level | Where | Needs a device |
| --- | --- | --- |
| Cascades, atlas rectangles, LOD, light budget, presets | `engine-renderer/src/shadow/*` unit tests | no |
| Who casts and receives, per entity | `engine-render-data`, `engine-presentation` unit tests | no |
| The caster-bounds debug view | `engine-presentation/src/shadow_debug.rs` unit tests | no |
| The system, in pixels | `engine-renderer/tests/shadow_render.rs` | yes, skips without one |
| The shipping scene, in pixels | `engine-renderer/tests/shadow_course_render.rs` | yes, skips without one |

The split matters here more than anywhere else in the renderer, because most shadow bugs are
arithmetic: a cascade fitted to the wrong sphere, a texel snap performed in a space that is
itself moving, a light granted a budget it cannot spend. All of those are decided on the CPU
in code that needs no device, and all of them are asserted directly rather than through a
picture. `tests/shadow_render.rs` covers what only a GPU can answer — that a caster darkens
the floor, that `receives = false` is not darkened, that no preset produces acne, that an
alpha-tested caster's shadow has holes in it, that there is no seam where cascades meet.

`tests/shadow_course_render.rs` is the pair of that: it renders the scene the game actually
loads (`--scene shadows`), not a synthetic one, because a shadow system that works on two
objects and fails on a real scene fails for reasons that live in the content. It also asserts
the atlas debug view draws the atlas, on the principle that a debugging tool is at its most
dangerous when it is broken, because that is the one moment nobody is checking it.

`docs/shadows.md` is the design document; MT-026 and MT-027 in `docs/manual-tests.md` are the
judgements a person still has to make.

## Audio tests (Phase 4, audio)

Audio is tested at four levels and **none of them needs an audio device**, which is the point
of the split described in `docs/architecture.md`: the mix is a function from voices to
samples, and samples are something a test can assert on.

| Level | Where | Needs hardware |
| --- | --- | --- |
| The synthesiser, the WAV reader, the catalogue and the bank | `engine-audio` unit tests | no |
| Voices, limits, priority, panning, attenuation and the mix | `engine-audio` unit tests | no |
| Volumes, their bounds and their file | `engine-settings` unit tests | no |
| The device wrapper's channel mapping | `engine-app` unit tests | no |
| The director, and every ability | `engine-presentation/tests/ability_audio.rs` | no |

`engine-presentation/tests/ability_audio.rs` casts every ability on the bar in a real fight
and asserts that each one produces a sound; that Firebolt's chain is cast, flight and impact
in that order and leaves nothing playing; that fifty burning targets are at most four burning
beds; that a rapid rotation never exhausts the voice pool; that the same fight is measurably
quieter from 45 m away than from nothing; and that an **empty bank** — the state a failed
load leaves — costs the fight nothing at all.

The last of those is the phase's stated failure mode, tested rather than asserted in prose: a
game whose audio did not load keeps running, keeps dealing damage, counts every missed cue,
and says so once per sound in the log.

## Enemy AI tests (Phase 6, enemy AI)

Four levels, none of which needs a device:

| Level | Where | Needs a GPU |
| --- | --- | --- |
| The state machine, in isolation | `engine-game/src/enemy/brain.rs` unit tests | no |
| Threat rules | `engine-game/src/enemy/threat.rs` unit tests | no |
| Shapes: what a circle and a cone catch | `engine-game/src/enemy/mod.rs` unit tests | no |
| The encounter lifecycle | `engine-game/src/encounter.rs` unit tests | no |
| A real fight, end to end | `engine-game/tests/encounter.rs` | no |

The brain is testable *alone* because it consumes an `AiPercept` and returns an `AiDecision`:
a test supplies a percept with a target 10 m away and asserts the enemy chases, with no
world, no arena and no collision. That is the same trick that makes the mixer testable
without speakers, applied to AI.

`engine-game/tests/encounter.rs` runs the real simulation and asserts the *rules*: an enemy
notices, chases, stops in range and lands damage; a caster telegraphs before it hits and
everything it damages was inside the shape it drew; every telegraph is matched by exactly one
ending; an interrupt costs the cast and is announced; damage generates threat and threat
picks the target; a leashed enemy goes home, forgets, and can be pulled again; a dead enemy
stops fighting and completes the encounter; a dead player cannot cast and a reset restores
everybody; a pack of five all fight and all die; the damage meter counts what the player
dealt and *not* what it took; and two runs of the same fight produce identical events.

That last one is Milestone 22's determinism claim at the scale that matters. The AI uses no
randomness at all, so the comparison is exact rather than statistical.

## Manual verification

Some behaviour cannot be asserted automatically: that a window actually appears, that
it resizes without artefacts, that a triangle is visible, that the debug overlay is
legible, that input feels connected. `docs/manual-tests.md` is the checklist, with an ID,
purpose, steps and expected result per entry. Per-milestone Phase 0 steps remain in
`docs/phase0.md`.

A SKIPPED manual test is not a pass. Two entries need hardware not every machine has
(a second monitor, a machine without a D3D12 adapter), and they stay SKIPPED until someone
runs them on that hardware.
