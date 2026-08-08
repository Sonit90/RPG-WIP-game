# Phase 4 — Combat feedback and the presentation layer that can afford it

**Status: COMPLETE**

## Mission

Phase 3 produced a playable prototype whose combat is legible only from a developer
readout. Phase 4 makes combat *visible* — health, casts, cooldowns, damage — and builds the
rendering primitives that make a real UI affordable at the project's frame budget.

The order is deliberate and is the whole point of the phase:

> Build the correct rendering primitives **first**, then build the UI on them.

The Phase 3 audit measured 20 nameplates at **0.43 ms of CPU per frame** — 2.7x the cost of
everything else in the frame, and roughly 10x the entire gameplay pipeline. Text is one quad
per lit font cell, so every UI element planned for this phase multiplies that number. Adding
the ability bar, HP bars, cast bar and damage numbers to that primitive would produce a
multi-millisecond UI against a 2.083 ms stretch budget.

Explicitly **not** in this phase: character models, skeletal animation, an asset pipeline,
environment art, networking, complex particles, post-processing. Those are Phase 5, and they
are a content problem. This is a rendering-architecture problem.

## Performance targets

```text
1920x1080, Radeon RX 6700 XT, Ryzen 7 5800X

240 FPS minimum      4.167 ms
480 FPS aspirational 2.083 ms
```

p99 is the figure that matters, not the mean. The Phase 3 audit found a mean of 0.797 ms
against a p99 of 3.87 ms in the 1,000-dummy scene — the mean was 19% of the 240 FPS budget
while the p99 was 93% of it. Every measurement in this document reports p50, p95 and p99
alongside the mean.

## Architecture

The seam Phase 3 established stays intact. Phase 4 adds a second one beside it, so the UI
reads a snapshot rather than the simulation:

```text
Input -> Commands -> Simulation (fixed 60 Hz)
                          |
                          +-- RenderExtractor --> RenderWorld ---> Culler -> RenderList
                          |        (interpolated by alpha)                       |
                          |                                                      v
                          +-- UiSnapshot ------> UiBuilder -> UiDrawList --> Renderer -> GPU
                                (plain data)      (layout)     (batched)
```

Rules this phase must not break:

* `engine-game` keeps its `engine-core`-only dependency. No UI type may appear in it.
* Gameplay never constructs a GPU resource. The UI describes itself as data; the renderer
  turns data into draws.
* The UI reads a `UiSnapshot`, never `&Simulation`.
* Interpolation happens in presentation. The simulation is never interpolated.

## Milestones

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Create `phase4.md` | COMPLETE |
| 2 | Baseline performance | COMPLETE |
| 3 | UI architecture redesign — the presentation snapshot | COMPLETE |
| 4 | Font atlas | COMPLETE |
| 5 | Batched UI rendering | COMPLETE |
| 6 | UI performance regression benchmark | COMPLETE |
| 7 | Per-instance material data | COMPLETE |
| 8 | Alpha blending | COMPLETE |
| 9 | Presentation interpolation | COMPLETE |
| 10 | Player and target UI snapshot | COMPLETE |
| 11 | Player HP/resource bars | COMPLETE |
| 12 | Target frame | COMPLETE |
| 13 | Target world-space HP bar | COMPLETE |
| 14 | Proper ability bar | COMPLETE |
| 15 | Cooldown visualization | COMPLETE |
| 16 | Cast bar | COMPLETE |
| 17 | Damage numbers | COMPLETE |
| 18 | Target feedback | COMPLETE |
| 19 | Hit feedback | COMPLETE |
| 20 | Combat HUD composition | COMPLETE |
| 21 | Combat feedback stress test | COMPLETE |
| 22 | Performance budget tests | COMPLETE |
| 23 | Full regression | COMPLETE |
| 24 | Final visual verification | COMPLETE |

## Test strategy

| Layer | How it is tested | Needs a device |
| --- | --- | --- |
| `UiSnapshot` | Built from a simulation, asserted field by field | no |
| UI layout / draw list | A built list is asserted for element count, order, geometry and allocation behaviour | no |
| Font atlas | Glyph lookup, missing glyphs, stability across rebuilds | no |
| Batched UI renderer | Draw-call counts, offscreen pixel readback | **yes** |
| Per-instance material | Extraction churn counters; instanced-vs-per-object pixel equivalence | partly |
| Alpha blending | Offscreen readback of a known blend result | **yes** |
| Interpolation | Deterministic: a known previous and current state at a known alpha | no |
| Damage numbers | Driven from a `CombatEvent` stream with no renderer present | no |
| Budgets | Asserted against measured baselines with machine-variance headroom | no |

Everything that can run without a device must run without one. That rule is what kept
Phase 3's gameplay tests honest and it applies to the whole UI stack.

## Milestone log

### Milestone 1 — Create `phase4.md`

Status: COMPLETE

**Implemented** — this document, written before any production code was touched.

---

### Milestone 2 — Baseline performance

Status: COMPLETE

**Method.** Release, 1920x1080 windowed, vsync off, 6,000 frames per run; figures are the
final CPU/GPU window (the last 1,024 frames), so start-up is excluded. Two builds were run
**interleaved** — default (nameplates drawn) and `--no-default-features` (no overlay, no
nameplates) — because a back-to-back A-then-B protocol on a machine that is also compiling
measures the machine, not the change. Three interleaved rounds for CPU, two for GPU; the
table reports the **median of the rounds**.

`report_final` was extended to print p50 and max alongside mean/p95/p99 — the phase states
its budgets against p99, and a report that omits the median cannot show the difference
between a slow frame and a slow run.

**CPU frame time, milliseconds**

| Scene | Plates | Build | mean | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 4 dummies | 5 | UI | 0.239 | 0.206 | 0.439 | 0.866 | 1.417 |
| 4 dummies | — | no UI | 0.194 | 0.174 | 0.258 | 0.711 | 1.157 |
| 20 dummies | 20 | UI | 0.348 | 0.295 | 0.760 | 1.113 | 1.691 |
| 20 dummies | — | no UI | 0.201 | 0.177 | 0.263 | 0.769 | 1.214 |
| 100 dummies | 20 | UI | 0.386 | 0.320 | 0.968 | 1.326 | 1.861 |
| 100 dummies | — | no UI | 0.203 | 0.183 | 0.265 | 0.726 | 1.187 |
| 500 dummies | 20 | UI | 0.466 | 0.394 | 1.085 | 1.271 | 2.105 |
| 500 dummies | — | no UI | 0.258 | 0.227 | 0.506 | 0.810 | 1.303 |
| 1000 dummies | 20 | UI | 0.540 | 0.454 | 1.218 | 1.734 | 2.366 |
| 1000 dummies | — | no UI | 0.311 | 0.272 | 0.667 | 0.940 | 1.474 |

**GPU frame time, milliseconds**

| Scene | Build | mean | p50 | p95 | p99 | max |
| --- | --- | --- | --- | --- | --- | --- |
| 4 dummies | UI | 0.086 | 0.081 | 0.085 | 0.380 | 0.690 |
| 4 dummies | no UI | 0.072 | 0.067 | 0.070 | 0.328 | 0.597 |
| 20 dummies | UI | 0.148 | 0.130 | 0.143 | 0.577 | 0.902 |
| 20 dummies | no UI | 0.070 | 0.066 | 0.070 | 0.311 | 0.337 |
| 100 dummies | UI | 0.150 | 0.132 | 0.140 | 0.518 | 0.718 |
| 100 dummies | no UI | 0.077 | 0.073 | 0.076 | 0.331 | 0.445 |
| 500 dummies | UI | 0.220 | 0.191 | 0.611 | 0.836 | 1.009 |
| 500 dummies | no UI | 0.106 | 0.099 | 0.101 | 0.374 | 0.587 |
| 1000 dummies | UI | 0.249 | 0.218 | 0.675 | 0.826 | 1.086 |
| 1000 dummies | no UI | 0.138 | 0.127 | 0.134 | 0.456 | 0.631 |

**The cost of 20 nameplates, isolated** (UI minus no-UI at the same scene, 20 plates drawn):

| Scene | CPU mean | CPU p50 | CPU p95 | GPU mean |
| --- | --- | --- | --- | --- |
| 20 dummies | **+0.147 ms** | +0.118 | **+0.497** | **+0.078 ms** |
| 100 dummies | **+0.183 ms** | +0.137 | **+0.703** | +0.073 ms |
| 1000 dummies | +0.229 ms | +0.182 | +0.551 | +0.111 ms |

**Correction to the Phase 3 audit.** That audit reported ~0.43 ms for 20 plates. Re-measured
here under an interleaved protocol on an otherwise idle machine, the figure is **0.15–0.23 ms
of CPU and ~0.08 ms of GPU**. The earlier number was inflated by compiling in the background
between runs. The conclusion is unchanged and is if anything sharper in the tail: plates
raise CPU **p95 by roughly 0.5–0.7 ms — a 3.7x increase** — while raising the mean by less
than 0.2 ms. The UI is not expensive on an average frame; it is expensive on the frames that
decide whether the frame rate holds.

**Draw calls.** 5–6 per frame in every scene: the world batches by mesh and material into
3–4 instanced draws, and the overlay adds one cached statistics layer plus one dynamic
nameplate layer.

**Allocations.** Not measurable from the game binary today — it has no counting allocator.
The gameplay pipeline is already covered by `game_bench` (0 to 0.0027 allocations per tick,
re-verified this phase). UI allocation is measured in Milestone 6, which builds the counting
harness the game lacks.

**Architectural decisions** — no optimisation was performed in this milestone, by design. A
baseline taken after a change is not a baseline.

**Known limitations** — the game reports no per-stage split in release
(`PROFILING_ENABLED` is false), so "UI CPU" here is inferred from an A/B of two builds
rather than measured directly. Milestone 6's benchmark measures it directly.

---

### Milestone 3 — UI architecture redesign: the presentation snapshot

Status: COMPLETE

**Implemented** — `crates/engine-presentation/src/snapshot.rs`. `UiSnapshot` is everything
the UI is allowed to know, and nothing else:

```text
UiSnapshot
 ├── player         health, max, resource, max, position, facing, movement state, alive, statuses
 ├── target         Option: entity, name, health, max, position, height, distance, alive, valid, statuses
 ├── abilities      [AbilitySlotView; 10]: id, name, hotkey, cooldown remaining/total, cost,
 │                                          availability, placeholder
 ├── cast           Option: id, name, elapsed, duration      (shape now, filled in Milestone 16)
 ├── combat         total damage, DPS, seconds, deaths
 ├── notifications  last refused cast
 └── time           simulated seconds — every animation is driven from this, never a frame counter
```

`Simulation::player_mut` was added alongside the existing `world_mut`, so a test can set up
a state (an unaffordable ability) that the command path would take seconds of simulated time
to reach.

**Tests** — 13 new, all without a device:

| Test | Asserts |
| --- | --- |
| `the_snapshot_reports_the_player_the_ui_would_draw` | health and resource reach the HUD |
| `the_nearest_dummy_is_the_target_and_carries_its_health` | target view is populated |
| `a_health_readout_never_says_zero_while_the_target_lives` | 1% floor — a living enemy is never drawn as dead |
| `a_dead_target_is_reported_as_invalid_rather_than_dropped` | the frame shows the corpse rather than vanishing at the moment of the kill |
| `every_ability_slot_is_described_with_its_hotkey` | ten slots, digits 1–0 |
| `a_used_ability_reports_its_cooldown_as_a_fraction_that_falls_to_zero` | the sweep starts near 1.0 and stops at 0.0 rather than going negative |
| `an_unaffordable_ability_is_distinguished_from_a_cooling_one` | `NoResource` vs `OnCooldown` — the bar draws them differently |
| `statuses_reach_the_snapshot_with_their_remaining_time` | debuff line data |
| `the_status_list_is_bounded_rather_than_growing` | fixed-size, drops the fifth |
| `the_combat_session_reaches_the_snapshot` | meter totals |
| `a_refused_cast_reaches_the_notifications` | `OnCooldown` surfaces |
| `building_repeatedly_allocates_nothing` | 1,000 rebuilds; the snapshot is a value, not a handle to heap storage |
| `a_snapshot_of_a_world_without_a_player_does_not_panic` | a despawned player mid-frame |

`engine-presentation`: 49 tests before, **62 after**.

**Architectural decisions**

* **No `Vec`, no `String` anywhere in the snapshot.** Every field is a fixed array or a
  scalar, so a per-frame snapshot costs no allocation by construction rather than by
  discipline. `StatusList` drops a fifth status rather than growing.
* **`SlotAvailability` is a reason, not a boolean**, and its evaluation order matches the
  simulation's own validation order in `use_ability`. Any other order would let the ability
  bar and the error message disagree about why a cast failed.
* **A dead target stays in the snapshot, marked invalid.** Dropping it would make the target
  frame vanish at the exact moment the player wants to see what happened.
* **`CombatView` is `f64`** because the meter accumulates in `f64`; narrowing here would
  make the HUD disagree with the meter it reports.
* **`cast` exists before the simulation can produce one.** The cast bar is written against
  its final shape in Milestone 16 rather than being retrofitted.

**Known limitations** — `hud.rs` (the *developer* overlay) still borrows `&Simulation`
directly. That is deliberate and stated in the crate docs: a debug readout may, game UI may
not. `UiSnapshot::build` returns a fresh value rather than writing into a caller-owned
buffer; it allocates nothing either way, and `refresh` exists for the per-frame path.

---

### Milestone 4 — Font atlas

Status: COMPLETE

**Implemented** — `crates/engine-renderer/src/font_atlas.rs`. The existing 5x7 bit-pattern
font is rasterized **once** into a single 256x32 R8_UNORM texture, 8 KiB:

```text
96 cells in a 32x3 grid, one per printable ASCII character plus one for everything else.
cell index = character.to_ascii_uppercase() as usize - 32,  or UNKNOWN_CELL
Each cell is 7x9 texels: a 5x7 glyph plus a one-texel border on every side.
```

The payoff is the point of the whole phase: a glyph becomes **one quad with a texture
coordinate** instead of one quad per lit cell. A 22-character nameplate title falls from
~330 quads to 22.

`font` was moved **out of** the `debug-overlay` feature gate. It is a bit-pattern table, it
is what game UI draws text with, and a shipping `--no-default-features` build that dropped it
would have no way to write a health percentage. The developer *statistics* overlay stays
gated. This is the first half of the "split game UI from the debug feature" work.

**Tests** — 10 new, no device:

| Test | Asserts |
| --- | --- |
| `the_atlas_fits_its_texture_and_its_row_pitch_is_upload_aligned` | 8 KiB; row pitch % 256 == 0; every cell inside the texture; the grid is exactly filled |
| `every_printable_character_gets_its_own_cell` | no collisions across ASCII 32–126 |
| `lowercase_shares_the_uppercase_cell` | one bitmap, one cell — not two answers to one question |
| `an_unsupported_character_resolves_to_a_visible_box_rather_than_a_blank` | a missing glyph is visible, not silent |
| `the_space_cell_exists_and_is_empty` | lookup never fails; the builder can skip it |
| `a_glyph_is_rasterized_where_its_bit_pattern_says` | the atlas is the font, not a picture of one |
| `every_cell_keeps_a_transparent_border_so_glyphs_cannot_bleed` | the padding that prevents the classic atlas artefact |
| `texture_coordinates_stay_inside_the_atlas_and_are_never_degenerate` | UVs in [0,1], one glyph wide |
| `lookup_is_stable_across_atlases_and_repeated_calls` | two atlases are byte-identical; repeated text gives identical coordinates |
| `every_character_the_ui_emits_has_a_cell_that_is_not_the_unknown_box` | the Phase 3 guarantee, restated against the atlas |

**Architectural decisions**

* **256 texels wide, not the 224 the columns need.** D3D12 requires a 256-byte-aligned row
  pitch for a texture upload; choosing the width to satisfy it makes the upload a straight
  copy with no per-row padding path to get wrong.
* **One-texel padding, not tight packing.** A bilinear sample at a glyph edge reaches half a
  texel past it; the border keeps that inside the cell.
* **Lookup is a pure function of the character** — no cache, no insertion, no state. There is
  no "add a glyph to the atlas" path to race or to invalidate.
* **Not anti-aliased.** The source is a bit pattern; inventing intermediate coverage would
  blur a font designed to be crisp.

**Known limitations** — ASCII only, and uppercase-only letterforms, because that is what the
font defines. Anything else draws the hollow box. One fixed size: scaling is done by the
quad, so large text is a magnified bitmap rather than a re-rasterization. Both are
appropriate for a HUD and neither is a text engine.

---

### Milestone 5 — Batched UI rendering

Status: COMPLETE

**Implemented** — the whole interface is now **one draw call**, through a pipeline that did
not exist before.

| Piece | Where | What it does |
| --- | --- | --- |
| `UiDrawList` | `engine-render-data/src/ui.rs` | A frame's UI as commands — rect, outline, text — in paint order. Plain data: no vertex, no handle, no GPU concept, so `engine-presentation` builds the entire HUD without linking D3D12. Text lives in one arena addressed by byte range. |
| `GameUi` | `engine-presentation/src/game_ui.rs` | The layout. Reads a `UiSnapshot` and nameplates, writes commands. Owns its list and scratch strings across frames. |
| `UiBatcher` | `engine-renderer/src/ui_pipeline.rs` | Expands commands into one vertex buffer. A glyph is one quad; a rect samples the atlas's opaque texel, so text and shapes share a batch. |
| `UiPipeline` | same | Root signature (CBV + SRV table + **static** point sampler), alpha blend state, depth test and write **off**. |
| `ui.hlsl` | `engine-renderer/shaders` | Pixels to clip space on the GPU; the atlas modulates alpha only, so a glyph is the vertex colour cut to the letter shape. |
| `StagingUploader::queue_texture` | `engine-renderer/src/upload.rs` | The texture upload path the engine did not have. Pads rows to D3D12's 256-byte pitch. |
| `Renderer::set_ui` / `draw_ui` | `engine-renderer/src/renderer.rs` | Batches, uploads into the frame's ring, records one `DrawInstanced` after the scene. |

The game was switched over: `App::draw_nameplates` became `App::build_ui`, and it is **no
longer behind the `debug-overlay` feature**. A `--no-default-features` build now draws
nameplates. That was a Phase 3 known limitation and it is gone.

`--no-ui` was added to the game: a measurement control that turns off the layout *and* the
draw, so the interface's cost is the difference between two runs of one binary rather than
two separately compiled builds. The Phase 3 nameplate figure was hard to trust for exactly
that reason.

**Tests** — 34 new.

| Suite | Count | Covers |
| --- | --- | --- |
| `engine-render-data::ui` | 11 | paint order; text arena and ranges; zero-area commands dropped; outline clamped rather than inverted; bar background/fill geometry; fraction clamping; `append` rebasing text ranges; clearing keeps every allocation; a malformed range reads empty |
| `engine-renderer::ui_pipeline` | 15 | vertex layout matches the input elements; **one quad per character, not one per lit cell**; a space advances without geometry; outline corners meet without overlapping; text and rects share one batch; centred and right-aligned text; scale; the quad budget drops work rather than growing mid-frame; rebuilds reuse the allocation; zero viewport gives a zero scale, not an infinity; depth off; straight alpha blend |
| `engine-renderer::ui_draw` (GPU) | 6 | the atlas uploads and the pipeline is created under the debug layer; **200 bars + 200 outlines + 200 text runs cost exactly one extra draw call**; an empty UI costs none; a disabled UI draws nothing; a translucent element blends; the UI survives a resize |
| `engine-presentation::game_ui` | 7 | a plate becomes a panel and its text; exactly one outline and it is the target's; the panel is translucent; plates painted furthest-first; health colour near death; rebuilds do not accumulate; numbers written without allocating |

Workspace total: **793 → 856 passing, 0 failed.**

**Performance.** Same protocol as Milestone 2, but now A/B within **one binary** using
`--no-ui`. Three interleaved rounds, median reported, 6,000 frames each.

CPU cost of the interface — the difference the UI makes to the frame:

| Scene | Plates | Before (M2) mean | After mean | Before p95 | After p95 |
| --- | --- | --- | --- | --- | --- |
| 20 dummies | 20 | +0.147 ms | **+0.030 ms** | +0.497 ms | **+0.132 ms** |
| 100 dummies | 20 | +0.183 ms | **+0.043 ms** | +0.703 ms | **+0.107 ms** |
| 1000 dummies | 20 | +0.229 ms | **+0.107 ms** | +0.551 ms | **+0.244 ms** |

**A 4–5x reduction in mean and roughly 4–6x in the p95 tail** — the tail being the number the
phase is actually about.

Whole-frame CPU, against the Milestone 2 baseline:

| Scene | M2 mean | Now mean | M2 p50 | Now p50 | M2 p95 | Now p95 | M2 p99 | Now p99 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 4 dummies | 0.239 | **0.214** | 0.206 | 0.188 | 0.439 | **0.357** | 0.866 | 0.804 |
| 20 dummies | 0.348 | **0.228** | 0.295 | 0.201 | 0.760 | **0.395** | 1.113 | 0.758 |
| 100 dummies | 0.386 | **0.253** | 0.320 | 0.221 | 0.968 | **0.462** | 1.326 | 0.798 |
| 500 dummies | 0.466 | **0.331** | 0.394 | 0.290 | 1.085 | **0.622** | 1.271 | 0.961 |
| 1000 dummies | 0.540 | **0.415** | 0.454 | 0.358 | 1.218 | **0.926** | 1.734 | 1.163 |

GPU, 20 plates: **0.091 ms with the UI against 0.068 ms without (+0.023 ms)**, where the
Phase 3 path cost +0.078 ms. At 1,000 dummies, 0.197 vs 0.130.

Internally measured UI CPU (layout + batching, reported by the game itself):
**0.010 ms for 20 nameplates — 204 quads from 41 commands.** Draw calls: **5**, of which the
entire interface is 1.

**Architectural decisions**

* **Commands, not vertices, cross the seam.** Layout code could emit vertices directly, but
  expanding a string into glyph quads needs the atlas, the atlas lives beside the texture it
  is uploaded into, and moving it into a data crate would drag the renderer's vertex layout
  across the boundary. A command list keeps the shader ABI on the renderer's side, where the
  rest of it already lives.
* **Painter's order, depth test off.** Phase 3 gave each plate a depth value and let the
  depth buffer sort them. That works for opaque quads and breaks the moment anything is
  translucent — a blended fragment that fails the test vanishes, and one that passes writes
  depth that occludes what should have blended over it. "What is on top" is now a property
  of the code that pushed it.
* **One opaque texel in the atlas.** It is what makes a solid rectangle and a line of text
  the same batch. Without it every panel behind every bar would split the draw in two.
* **A static sampler**, not a descriptor: it never changes, so it costs no heap space and no
  bind.
* **A quad budget that drops work rather than growing.** 65,536 quads is far past anything
  the HUD produces; a runaway UI degrades visibly instead of allocating inside the frame path.
* **`--no-ui` is a measurement control, not a game option.** The audit's 0.43 ms figure was
  wrong because it compared two builds on a busy machine.

**Known limitations**

* The **developer statistics overlay still uses the old per-glyph path**. It is a debug
  readout, it is off by default, and porting it is not what the phase is for — but it means
  two text renderers exist in the binary while `debug-overlay` is on.
* The nameplate *build* still scans every entity to find nearby enemies, which is why the
  interface costs more at 1,000 dummies (+0.107 ms) than at 100 (+0.043 ms) despite drawing
  the same twenty plates. Linear in the world, not in the UI.
* `set_ui` copies the caller's list into the renderer's. `ui_list_mut` exists to build in
  place and avoid it; the game does not use it yet because the copy is not measurable.
* No scissor or clipping: an element positioned off-screen is submitted and rasterizes to
  nothing rather than being rejected on the CPU.

---

### Milestone 6 — UI performance regression benchmark

Status: COMPLETE

**Implemented** — `crates/engine-bench/src/ui_bench.rs`, a fifth benchmark binary. Two
phases, because the two questions are different:

* **CPU phase** — layout and batching, no device, 5,000 measured iterations after 500
  warm-up. A distribution over thousands of samples, not a stopwatch reading.
* **GPU phase** — the same interface against a hidden 1920x1080 window, 600 frames after 60
  warm-up, measured with the GPU's own timestamps and run **twice**: with the UI and with it
  switched off. The number that matters is what the interface adds, and a frame that clears
  and presents is not free.

The scene is deterministic — element position and fill derive from the index and the frame
number — so two runs measure the same work. One "element" is a labelled bar (panel, bar
background, bar fill, name, right-aligned value, and an outline on every fourth), which is
what the real HUD is made of.

**Results** — release, 1920x1080, vsync off:

| Elements | Commands | Quads | CPU mean | p50 | p95 | p99 | max | Allocations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 20 | 104 | 419 | **2.47 µs** | 2.50 | 2.50 | 2.60 | 4.60 | **0** |
| 50 | 262 | 1,051 | 6.31 µs | 6.30 | 6.30 | 9.20 | 11.70 | **0** |
| 100 | 524 | 2,099 | 12.57 µs | 12.60 | 12.70 | 13.50 | 28.40 | **0** |
| 250 | 1,310 | 5,249 | 31.59 µs | 31.40 | 32.20 | 35.10 | 50.40 | **0** |
| 500 | 2,620 | 10,495 | 62.86 µs | 62.40 | 63.70 | 71.50 | 119.50 | **0** |
| 1,000 | 5,240 | 20,990 | 125.91 µs | 125.40 | 127.70 | 137.70 | 240.30 | **0** |

GPU, and the draw-call count that is the point of the batching:

| Elements | GPU with UI | GPU without | **Cost** | Draw calls |
| --- | --- | --- | --- | --- |
| 20 | 0.049 ms | 0.053 ms | below the noise floor | **2 vs 1** |
| 50 | 0.068 ms | 0.056 ms | +0.012 ms | **2 vs 1** |
| 100 | 0.094 ms | 0.056 ms | +0.037 ms | **2 vs 1** |
| 250 | 0.184 ms | 0.057 ms | +0.127 ms | **2 vs 1** |
| 500 | 0.215 ms | 0.053 ms | +0.162 ms | **2 vs 1** |
| 1,000 | 0.364 ms | 0.058 ms | +0.306 ms | **2 vs 1** |

**One draw call for the interface at every size**, from 419 quads to 20,990.

The layout/batch split, which says where the time goes:

| Elements | Layout | Batching | Ratio |
| --- | --- | --- | --- |
| 20 | 0.17 µs | 2.29 µs | 13x |
| 100 | 0.86 µs | 11.71 µs | 14x |
| 1,000 | 7.54 µs | 118.37 µs | 16x |

Batching dominates and scales linearly at about **6 ns per quad**. Layout — walking the
snapshot and pushing commands — is nearly free. If the UI ever needs to get faster, the
vertex expansion is the thing to attack, not the layout.

**Against the budget.** A realistic HUD is tens of elements, not thousands. At 20 elements
the interface costs **2.5 µs of a 2,083 µs stretch-target frame: 0.12%**. Even 1,000
elements — a UI nothing in this game would draw — costs 126 µs of CPU and 0.31 ms of GPU,
still inside the 240 FPS budget on its own.

**Architectural decisions**

* **Percentiles, not means.** The Phase 3 audit found a UI whose mean was comfortable and
  whose p95 was four times its median. Every row here carries p50, p95, p99 and max.
* **The GPU phase A/Bs within one process**, for the same reason `--no-ui` exists.
* **`engine-platform` became a dependency of `engine-bench`**, purely to open the hidden
  window the GPU phase renders into. The CPU phase still needs no device and is the part
  that runs everywhere.

**Known limitations** — the GPU phase renders an empty scene plus the UI, so it isolates the
interface but does not measure it competing with a real scene for bandwidth; the combat
stress test in Milestone 21 does that. The benchmark measures batching from an already-built
list, so it does not include the `set_ui` copy the game currently performs.

---

### Milestone 7 — Per-instance material data

Status: COMPLETE

**Implemented** — `InstanceStyle` in `engine-render-data/src/world.rs`: a tint (RGBA,
multiplied with the material's base colour) and an emissive scalar (added to the material's).
Five floats. It is not a second material system and the doc comment says so — no textures, no
shader selection, no parameter the lighting model does not already read.

It reaches the GPU through **both** draw paths, because a test asserts the two produce the
same image:

| Path | Carries the style as | Size |
| --- | --- | --- |
| Instanced (default) | two more elements in the per-instance vertex stream, `INSTANCE_TINT` and `INSTANCE_EMISSIVE` | `InstanceTransform` 64 → 96 bytes |
| Per-object (fallback) | two more fields in the `Object` constant buffer | `ObjectConstants` 64 → 96 bytes |

`RenderWorld` gained `set_style` and `set_material`, and `RenderList` carries a parallel
`styles` array that deliberately takes **no part in grouping** — two entities with different
tints still share a batch, which is the entire reason the tint is per-instance rather than
per-material.

The extractor was rewritten around it. Phase 3 despawned and respawned a render entity
whenever a dummy died, because a material handle was fixed at spawn. Now **only a changed
generation** — genuinely a different entity in a recycled slot — causes a respawn. Three
visual rules became expressible for the first time:

* **Health tint** — wounded dummies darken towards a floor of 0.45, not towards black. A tint
  that reached zero would make a nearly-dead dummy invisible against the arena floor, which
  reads as a rendering bug rather than as low health.
* **Target glow** — the selected enemy is emissive, so selection is visible on the enemy
  rather than only on a floating marker.
* **Hit flash** — a 0.18-second brighten, driven from **simulation** time so it lasts the same
  wall-clock duration at 60 FPS and at 1,400.

The flash is fed by *observing* `CombatEvent::Damage` in `engine-app`, exactly as the damage
meter does. `engine-game` has no idea a tint exists.

**Tests** — 13 new.

| Suite | Test | Asserts |
| --- | --- | --- |
| `render-data` | `styling_an_entity_keeps_its_handle_its_slot_and_its_material` | a visual change is a store, not a respawn |
| | `changing_a_material_is_no_longer_a_respawn` | the Phase 3 limitation, gone |
| | `a_stale_handle_cannot_style_the_entity_that_replaced_it` | generational safety extends to the new setters |
| | `a_spawned_entity_starts_unstyled` | the default is a multiply by one — a zero default would render everything black |
| | `a_negative_emissive_is_clamped_rather_than_darkening_the_surface` | emissive is added, so a negative one would subtract light |
| `presentation` | `dying_changes_how_a_dummy_is_drawn_without_respawning_its_render_entity` | same handle, 0 spawned, 0 despawned, 1 restyled |
| | `a_wounded_dummy_darkens_without_any_render_churn` | the health tint, and that it stays above the floor |
| | `the_current_target_glows_and_stops_glowing_when_it_is_dropped` | selection is not a respawn |
| | `a_hit_flashes_and_fades_on_simulation_time` | brightens, then returns exactly to rest |
| | `a_recycled_slot_does_not_inherit_the_previous_occupants_flash` | the generation check on a hit |
| | `a_long_fight_causes_no_render_entity_churn_at_all` | **600 ticks of constant damage, target cycling and deaths: spawned + despawned == 0** |
| `renderer` | `an_instance_is_the_size_the_input_layout_describes` | 96 bytes; tint at 64, emissive at 80 |
| | `an_unstyled_instance_carries_the_neutral_style` / `a_styled_instance_carries_the_style_it_was_given` | the constructors |

The five **instanced-vs-per-object pixel equivalence tests still pass**, which is the real
proof that the tint is implemented identically in both shaders rather than in one.

Workspace total: **856 → 869 passing, 0 failed.**

**Performance.** The instance record grew by 50%, and that is not free:

| Measure (1,000 dummies) | Before | After | Change |
| --- | --- | --- | --- |
| Extraction (`game_bench`) | 21.17 µs | 24.47 µs | +16% |
| Culling (`game_bench`) | 6.21 µs | 7.02 µs | +13% |
| Simulation | 19.65 µs | 19.59 µs | unchanged |
| Whole frame, CPU mean | 0.415 ms | 0.439 ms | **+0.024 ms** |
| Whole frame, GPU mean | 0.197 ms | 0.202 ms | +0.005 ms |

**0.024 ms of a 4.167 ms budget**, at a scene size 50x larger than the game's own. In exchange
the per-frame churn that used to accompany every death is zero, and three visual behaviours
that were impossible are now two stores per entity.

**Architectural decisions**

* **A style is per instance, not per material.** A tint expressed as a material would split
  the batch — the exact cost the Phase 3 extractor paid in a different currency.
* **The style takes no part in grouping.** `group_by_material_and_mesh` never reads it.
* **Both shaders changed together.** `mesh.hlsl` and `mesh_instanced.hlsl` must shade
  identically or an instanced frame and a non-instanced frame are different images; the
  equivalence test is what enforces it, and it caught nothing only because both were edited.
* **`INSTANCE_ALIGNMENT` is 16, not `size_of::<InstanceTransform>()`.** The record grew to 96
  bytes, 96 is not a power of two, and the upload ring's aligner requires one. Every element
  in the stream is a `float4` or smaller, so sixteen is both correct and stable against the
  record growing again.
* **A neutral style still costs the multiply.** The shader does not branch on it. A branch to
  skip a multiply-add would cost more than the multiply-add.

**Known limitations** — the tint cannot brighten past the material's own colour except
through the emissive term, because it is a multiply. `dead_material` is still a separate
material rather than a tint, since a corpse's colour is not a scaling of a live dummy's;
switching it is now a store rather than a respawn, which was the actual problem. The style is
written unconditionally every frame for every entity rather than only when it changes —
comparing first would cost a load and a branch to avoid a store of the same width.

---

### Milestone 8 — Alpha blending

Status: COMPLETE

**Implemented** — the blend state landed with the UI pipeline in Milestone 5; this milestone
is the part that makes it a claim rather than a hope: **pixel-level proof**.

| Piece | What |
| --- | --- |
| `alpha_blend_desc` | `src * a + dst * (1 - a)`. Alpha itself is `a + dst * (1 - a)` rather than replaced, so a translucent element over another accumulates coverage instead of punching a hole in it. |
| Ordering | Depth test and depth write **off**; the list is painted in order. See `engine_render_data::ui` for why a depth test and blending cannot coexist. |
| Opaque geometry | Untouched. The scene pipeline's `blend_desc` still has `BlendEnable: false` — blending was added beside the opaque path, not switched on globally. |
| `offscreen::render_ui` | Renders a `UiDrawList` into an offscreen target and reads the pixels back. |

**Tests** — 7 new, six of them reading real pixels off the GPU:

| Test | Asserts |
| --- | --- |
| `a_half_alpha_quad_lands_halfway_between_the_clear_and_its_colour` | the defining arithmetic. An opaque pipeline gives the source outright; an additive one gives something brighter than either input |
| `a_fully_transparent_quad_leaves_the_target_untouched` | what a damage number fades to — if alpha zero still wrote, a finished number would sit on screen forever |
| `an_opaque_quad_replaces_what_is_behind_it` | alpha one is exact; a panel letting 1% through would shimmer as the scene moved behind it |
| `two_translucent_quads_accumulate_in_paint_order` | 0.5 then 0.5 gives 0.25 of the first and 0.5 of the second — painter's order proven by arithmetic |
| `the_later_quad_is_the_one_on_top` | the property a HUD author relies on |
| `a_glyph_is_the_vertex_colour_cut_to_the_letter_shape` | the atlas modulates **alpha only**. Inside an `H`'s stroke the pixel is the vertex colour; in the gap the clear is untouched. Modulating colour instead would make text a grey wash |
| `opaque_scene_geometry_is_not_blended` | blending was not switched on globally |

Tolerance is three of 255 — one step of 8-bit UNORM quantization — rather than an
eyeballed epsilon.

Workspace total: **869 → 876 passing, 0 failed.**

**Architectural decisions**

* **Readback, not draw counts.** A recorded draw and a silent validation layer cannot
  distinguish a translucent panel from an opaque one, and that distinction is the milestone.
* **Straight alpha, not premultiplied.** Premultiplied composites more correctly through
  multiple layers, and it would mean every colour a layout author writes has to be
  pre-scaled. For a HUD drawn straight to the back buffer the difference does not arise.
* **The UI pass declares no depth-stencil format at all**, rather than declaring one and
  disabling the test. It does not read or write depth, so tying it to the scene's depth
  buffer would be a dependency for nothing.

**Known limitations** — no separate blended *scene* pass: translucent world geometry
(particles, translucent markers in world space) would need its own back-to-front list after
the opaque pass. The primitives are all here — the blend state, the ordering rule and the
per-instance alpha in `InstanceStyle.tint[3]` — but nothing builds that list yet, because
nothing in this phase draws translucent geometry. Sorting a blended scene pass is Phase 5's
problem, and it will inherit a blend state that is already correct.

---

### Milestone 9 — Presentation interpolation

Status: COMPLETE

**Implemented** — the engine has had `FixedTimestep::alpha` since Phase 3, tested and unread.
It is now what the frame is drawn at.

```text
tick N-1 state          tick N state
     |                       |
     +------- alpha ---------+        alpha = accumulator remainder / step
                 |
          drawn this frame
```

Three pieces:

* **`GameWorld::previous_bodies`** — every body as it was at the start of the current tick,
  refreshed by `begin_tick()` at the top of `tick_once`, before anything moves. One
  `extend_from_slice` over the array. Nothing in gameplay reads it: recording *where
  something was* is a simulation fact, and blending the two is presentation's job.
* **`lerp_position` / `lerp_angle`** in `engine-presentation::extract`. The angle version
  wraps the difference into (−pi, pi] first, because a plain lerp turns a character crossing
  the wrap point the long way round — two degrees of movement becomes 358 degrees of spin.
* **`interpolated_player_position`** — what the camera follows. Following the raw simulation
  position would step the camera at 60 Hz behind a character moving smoothly, which is the
  same artefact in a different place.

Spawning and `restore` both reset the history to the current body. Without that, a recycled
slot would slide its new occupant in from wherever the previous one stood, and **R** would
make every dummy visibly glide back to its spawn.

**The one-tick lag is deliberate.** `previous` is tick N−1 and `current` is tick N, so the
drawn frame is between them — never ahead of the simulation. Extrapolating instead would
overshoot and snap back every time a character changed direction, which is a far more
visible artefact than 16 ms of latency.

**Tests** — 8 new.

| Test | Asserts |
| --- | --- |
| `a_position_is_blended_by_the_alpha_and_clamped_outside_it` | the blend, and that an out-of-range alpha cannot extrapolate |
| `a_yaw_takes_the_short_way_round_the_wrap_point` | the classic whip-around bug, and that the ordinary case is unchanged |
| `a_moving_character_is_drawn_between_its_last_two_ticks` | history exists and covers the whole gap |
| `a_still_character_is_drawn_in_exactly_one_place` | interpolation introduces no jitter when nothing moves |
| `the_camera_follows_the_interpolated_position_not_the_stepped_one` | mid-tick the camera is ahead of the last simulated position |
| `a_freshly_spawned_entity_does_not_slide_in_from_its_slots_previous_occupant` | per-spawn history reset |
| `interpolation_is_deterministic` | two identical runs place the character identically — a replay renders like the run it recorded |
| `the_player_is_drawn_standing_on_the_floor_facing_where_it_moves` (updated) | the Phase 3 test, restated against the interpolated position |

The **four Phase 3 determinism tests still pass**, which is what proves `begin_tick` changed
the simulation's storage and not its behaviour.

Workspace total: **876 → 883 passing, 0 failed.**

**Performance** — the history copy is not free and is not pretended to be:

| Measure | Before M9 | After M9 | Change |
| --- | --- | --- | --- |
| Simulation tick, 1,000 entities | 19.59 µs | 20.15 µs | +0.56 µs (+2.9%) |
| Extraction, 1,000 entities | 24.47 µs | 27.79 µs | +3.3 µs (+14%) |
| Simulation tick, 100 entities | 2.26 µs | 2.29 µs | +1% |
| Allocations per tick | 0.0027 | 0.0027 | unchanged |

At 1,000 entities the copy is 32 KB a tick. The extraction rise is the two extra array reads
and the lerp per entity. Both are inside a gameplay pipeline that is still **under 50 µs per
tick at a scene size fifty times the game's own**, and a tick happens once every twenty-odd
frames at these rates.

**Architectural decisions**

* **History lives in the simulation, interpolation lives in presentation.** The alternative —
  presentation keeping its own copy of last frame's positions — breaks the moment a frame
  runs more than one tick: the "previous" it held would be several ticks old, and blending
  across it by an alpha in [0,1) would render the character moving at a fraction of its real
  speed.
* **`begin_tick` is called by the tick, not by the caller.** A step that forgot it would
  silently interpolate from the wrong state.
* **The marker interpolates too.** Anything anchored to a world position has to, or it steps
  at 60 Hz above something that does not.

**Known limitations** — only bodies are interpolated, because only bodies move. A future
system with its own continuous state (a projectile, a turret) will need the same treatment
and will not get it for free. Nothing interpolates *scale*, so a resizing entity would still
step. The camera's own smoothing is unchanged and remains a separate, render-rate filter on
top of the interpolated target.

---

### Milestones 10 to 20 — the combat HUD

Status: COMPLETE

These milestones are one coherent piece of work — a HUD is not a set of independent widgets —
so they were built together and are recorded together, with each milestone's own decisions
and tests called out.

#### Milestone 10 — Player and target UI snapshot

The snapshot from Milestone 3 already carried the player, the target and the ten ability
slots. This milestone completed it: `CastView` is now **filled from a real cast** rather than
being a shape waiting for one, and `TargetView` carries `distance` and `valid` so the frame
can say "23M" and can grey itself out without re-deriving a rule the simulation owns.

The UI reads `UiSnapshot` and nothing else. `GameUi` has no `&Simulation` in any signature,
which is the architectural point of the whole group.

#### Milestone 11 — Player HP and resource bars

Bottom left, stacked: health above resource, each a background, a fill, a label and a
`current/max` readout. The health bar **changes colour below 30%**; the fill is clamped, so a
full bar cannot overflow its frame and an empty one still draws the frame — dying must not
look like the HUD broke.

#### Milestone 12 — Target frame

Top centre, where the genre puts it and where the eye goes on TAB. Name, distance, health
bar, `current/max` and percentage. It **updates the moment the target changes** because it is
rebuilt from the snapshot every frame — there is no cached target to invalidate.

A dead target **keeps its frame and loses its outline**, and its name gains `DEAD`. Dropping
the frame at the moment of the kill would remove the information exactly when the player
wants it.

#### Milestone 13 — World-space health bars

A bar above every nearby enemy, anchored to the plate the nameplate system already placed —
so the projection happens once and the two can never disagree about where an enemy is. It
inherits the twenty-plate cap for free, which is what keeps the cost flat.

#### Milestone 14 — Proper ability bar

Ten slots centred at the bottom, each a panel, a coloured frame, its hotkey digit and a
four-character abbreviation. The abbreviation is where an icon goes: **the layout does not
change when textures arrive**, which is the reason to build it this way now rather than
later.

The frame colour is the slot's *availability*, and `Ready`, `OnCooldown` and
`NoResource`/`NoTarget` are three different colours, because they are three different answers
to "why can't I press this".

#### Milestone 15 — Cooldown visualization

A **bottom-up wipe** plus the remaining time to a tenth of a second. A radial sweep needs
either a mask texture or a per-slot triangle fan; a wipe is one rectangle whose height is the
fraction, and it reads the same way — the slot fills back up as the ability returns.

The wipe is **translucent**, so a cooling ability is still recognisable as itself. That is
only possible because of Milestone 8.

Cooldowns come from `AbilitySlotView::cooldown_fraction`, which is simulation state. Nothing
counts frames.

#### Milestone 16 — Cast bar

This one needed the simulation to grow a feature. `AbilityDef` gained `cast_time`; `Player`
gained `Option<ActiveCast>`; the tick gained `tick_cast`. Two abilities now cast: `LANCE` at
1.2 s and `SMITE` at 2.5 s. The three designed abilities stay instant.

A cast is a **commitment**, which is the only thing that makes a cast bar worth drawing:

| Rule | Behaviour |
| --- | --- |
| Cost and cooldown | Paid when the cast **starts**. Cancelling wastes them. |
| Moving | Cancels it. Read from the body's movement state, so a body still sliding to a stop still counts as moving. |
| Another ability | Cancels it — two abilities in flight at once is not a state the game has. |
| The caster dies | Cancels it. |
| Target dies mid-cast | The cast ends without landing. Targeting is re-resolved when it lands, not assumed. |
| Validation | Happens when the cast **starts**. A cast begun against nothing and silently dropped two seconds later looks like a broken ability. |
| **R** | Clears a cast in progress — "mid-cast" is not the known state the control exists to return to. |

Both ends are announced as `CombatEvent::CastStarted` and `CastEnded { end }`, so the bar, a
future animation selector and a network client all read the stream everything else reads.
`CastEnd` distinguishes `Completed`, `Moved`, `Replaced` and `CasterDied`, so interruption is
already expressible — the presentation architecture will not need rewriting when a mob learns
to interrupt.

The bar is **absent when not casting**: its presence is the signal.

#### Milestone 17 — Damage numbers

`engine-presentation::damage_numbers`. Driven entirely by **observing**
`CombatEvent::Damage`, exactly as the damage meter is. `engine-game` has no idea a floating
number exists.

| Property | Behaviour |
| --- | --- |
| Lifetime | 1.1 simulated seconds |
| Motion | Rises 1.6 m, drifts 0.45 m to alternating sides so two hits in one tick do not smear together |
| Fade | Full opacity for the first 45%, then linear — a number that faded from the instant it appeared would be least readable exactly when the player looks at it |
| Anchoring | To where the hit landed, not to the target. A number pinned to a moving enemy is harder to read, and it lets a number outlive the entity it describes |
| Critical distinction | A killing blow is a different colour **and 1.4x the size**, so it reads without distinguishing two shades mid-fight |
| Zero damage | Draws nothing. A hit on a corpse is a legitimate zero and "0" floating up is noise |
| Capacity | A fixed ring of 128. A thousand-target hit **drops the oldest** rather than allocating |
| Behind the camera | Skipped, not drawn — the mirrored-ghost bug the nameplates already avoid |

The fade lives in the alpha channel, which only works because of Milestone 8.

#### Milestone 18 — Target feedback

Three signals, none of which respawns anything:

* the floating marker above the target (Phase 3, now interpolated),
* the white outline on its nameplate and on the target frame,
* an **emissive glow on the enemy itself**, through `InstanceStyle` from Milestone 7.

`the_current_target_glows_and_stops_glowing_when_it_is_dropped` asserts that selecting and
deselecting costs zero spawns.

#### Milestone 19 — Hit feedback

When damage lands: the target **flashes** (0.18 s, simulation-timed), a damage number
appears, its world-space health bar drops, the target frame's numbers change, and the enemy's
health tint darkens. All five come from the same slice of events.

#### Milestone 20 — Combat HUD composition

```text
                    +-------------------------------+
                    | DUMMY                     23M |
                    | 1892/2000              95%    |
                    +-------------------------------+

                          20  20  20  98
                      [ health bar over each enemy ]
                      [ DUMMY 95%   BURNING 4.8    ]

  HP  500/500  ####################
  MP   67/100  #############             LANCE  ########----  0.6S
                          [1][2][3][4][5][6][7][8][9][0]
```

Paint order *is* the composition: world-space bars, nameplates, target frame, player bars,
ability bar, cast bar, and damage numbers last so they float over everything.

**Tests for Milestones 10–20** — 41 new:

| Suite | Count | Covers |
| --- | --- | --- |
| `engine-game::cast_tests` | 10 | instant abilities still land on the tick they are used; a cast starts and lands; progress on simulation time; moving cancels and the effect never lands; a cancelled cast keeps its cost and cooldown; another ability replaces it; an impossible cast is refused with its reason; a dead target ends it without landing; both ends are announced; **R** clears it |
| `engine-presentation::damage_numbers` | 11 | a damage event becomes a number; **an area hit produces one per target**; rise and fade; the opacity hold; consecutive numbers drift apart; retirement keeps the allocation; the ring is bounded and keeps the newest; zero draws nothing; a killing blow is marked; a vanished entity is skipped; the system is a pure reader |
| `engine-presentation::game_ui` | 20 | player bars exist and track their values; a dead player keeps an empty bar; the target frame names, measures and reports; it updates on a target change; a dead target keeps its frame and loses its outline; nothing targeted draws no frame; every slot with its hotkey; a cooling ability draws a wipe and a time; unavailable is coloured differently from cooling; the cast bar appears only while casting; every plate carries a world bar above it; a damage number is drawn where it projects and fades in its alpha; one behind the camera is not drawn; the HUD follows a resize without resizing; the HUD is over the world; a whole frame stays under 400 commands |

Workspace total: **883 → 921 passing.**

**Known limitations** — nameplates still **overlap** when enemies line up, which is visible in
the arena's own group of four; the fix is a screen-space push-apart pass and it is cosmetic
rather than architectural. Ability slots show four letters instead of icons, by design. The
cast bar has no interrupt *animation* — the state exists, the flourish does not. Damage
numbers do not merge, so twenty hits on one target in a second produce twenty numbers rather
than a running total.

---

### Milestone 21 — Combat feedback stress test

Status: COMPLETE

**Implemented** — a `COMBAT HUD` phase was added to `ui_bench`. It builds the **real**
interface — `NameplateSet`, `UiSnapshot`, `GameUi` and a full ring of 128 floating damage
numbers — over an arena that has been fought in for 600 ticks, so health bars, statuses,
cooldowns and target state are all populated rather than sitting at their cheap defaults.

**Results** — release, 1920x1080, 5,000 measured iterations after 500 warm-up:

| Dummies | Commands | Quads | CPU mean | p50 | p95 | p99 | max | Allocations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 199 | 524 | 9.04 µs | 9.00 | 9.10 | 9.70 | 21.2 | **0** |
| 20 | 271 | 725 | 12.27 µs | 12.30 | 12.30 | 12.90 | 24.7 | **0** |
| 100 | 271 | 731 | 15.88 µs | 15.80 | 16.00 | 16.60 | 32.1 | **0** |
| 500 | 270 | 718 | 26.92 µs | 26.90 | 27.00 | 27.70 | 44.7 | **0** |
| 1,000 | 270 | 719 | 35.98 µs | 36.00 | 36.20 | 37.00 | 50.3 | **0** |

The command and quad counts are **flat** from 20 dummies to 1,000 — the cap does its job. The
time still grows because the nameplate builder scans every entity to find the nearby ones;
that is linear in the *world*, not in the interface.

**Whole-frame, in the game itself.** Three interleaved rounds, `--no-ui` as the control,
6,000 frames, median reported:

| Scene | CPU mean | p50 | p95 | p99 | Without UI (mean) | **UI cost** |
| --- | --- | --- | --- | --- | --- | --- |
| 4 dummies | 0.219 | 0.194 | 0.390 | 0.762 | 0.198 | +0.021 ms |
| 20 dummies | 0.245 | 0.215 | 0.454 | 0.843 | 0.196 | +0.049 ms |
| 100 dummies | 0.272 | 0.239 | 0.547 | 0.885 | 0.213 | +0.059 ms |
| 500 dummies | 0.370 | 0.320 | 0.876 | 1.081 | 0.273 | +0.097 ms |
| 1,000 dummies | 0.476 | 0.403 | 1.094 | 1.472 | 0.346 | +0.130 ms |

Live in the game at 20 plates the report line reads **`ui 0.004 ms (214 quads, 77 cmds)`,
5,000 FPS, 5 draw calls** — one of which is the entire interface.

**The headline.** The Milestone 2 baseline, drawing **only nameplates** through the Phase 3
path, was 0.348 ms mean / 0.760 ms p95 at 20 dummies. The full combat HUD — target frame,
player bars, ten ability slots with cooldown wipes, cast bar, world-space health bars,
nameplates and 128 damage numbers — now costs **0.245 ms mean / 0.454 ms p95**.

> The complete interface is cheaper than Phase 3's nameplates alone.

**Against the budget**, worst case (1,000 dummies, a scene fifty times the game's own):

| Goal | Budget | CPU mean | CPU p99 | Headroom at p99 |
| --- | --- | --- | --- | --- |
| 240 FPS minimum | 4.167 ms | 0.476 ms | 1.472 ms | **65% spare** |
| 480 FPS aspirational | 2.083 ms | 0.476 ms | 1.472 ms | **29% spare** |

In the game's own scene (4 dummies) the p99 is 0.762 ms — **37% of the 480 FPS budget.**

**Known limitations** — the stress scenario drives combat deterministically but is not the
game's own input; a human playing badly (spamming a target cycle every frame) is not
simulated. The GPU phase renders the UI over an empty scene, so it isolates the interface
rather than measuring it competing with a full scene for bandwidth; the whole-frame table
above covers that case.

---

### Milestone 22 — Performance budget tests

Status: COMPLETE

**Implemented** — `crates/engine-presentation/tests/ui_budgets.rs`, six tests.

They are **regression detectors, not benchmarks**, and the distinction shaped every one of
them: each asserts a quantity that does not vary with the machine — command counts, quad
counts, capacities, ratios. **Wall-clock time is deliberately never asserted.** A timing test
that fails on a busy build agent teaches everyone to ignore the suite.

| Test | Asserts |
| --- | --- |
| `a_full_combat_hud_stays_within_its_command_budget` | ≤ 400 commands (measured: 271), and that the list is not empty — a budget test that passes because nothing was drawn is worthless |
| `the_interface_does_not_grow_with_the_size_of_the_world` | 20 enemies to 1,000 grows the HUD by < 1.5x. The property the cap exists for |
| `a_steady_hud_allocates_nothing_after_the_first_frame` | 1,000 rebuilds leave the command count and every capacity unmoved |
| `the_whole_interface_is_one_batchable_unit` | every command is drawable without a state or texture change — the one-draw-call property, as a property of the vocabulary rather than of one frame |
| `a_thousand_simultaneous_hits_are_bounded_rather_than_unbounded` | the ring caps at 128 and reports 872 dropped, and the HUD still fits its budget |
| `the_hud_survives_a_world_with_nothing_in_it` | no target, no enemies: the player's own bars still draw and nothing divides by a count it does not have |

The measured baselines are recorded in the file's own header, so a future reader sees what
the numbers were when the thresholds were chosen.

**Two real findings, both caught by these measurements rather than by review:**

1. **`Nameplate::write_title` allocated a `String` per plate per frame** — a `collect()` to
   truncate a name. Twenty plates at 1,400 FPS is 28,000 allocations a second for a name that
   never changes. Now written character by character into the reused buffer.
2. **`sort_by` on the plate candidates allocated a merge buffer** once the list passed about
   twenty entries — one allocation per frame in any scene with more than twenty enemies in
   range. Now `sort_unstable_by`; the tie-break on entity id already made the order total, so
   stability bought nothing.

Together those took the combat HUD from 21 allocations per frame to **zero**, and tightened
the p99 at 20 dummies from 19.9 µs to 12.9 µs.

---

### Milestone 23 — Full regression

Status: COMPLETE

```text
cargo fmt --all -- --check                                    PASS
cargo clippy --workspace --all-targets -- -D warnings          PASS (0 warnings)
cargo clippy --workspace --all-targets --no-default-features   PASS (0 warnings)
cargo test --workspace                                         927 passed, 0 failed
cargo build --workspace                                        PASS
cargo build --workspace --release                              PASS
cargo build --release --no-default-features --bin mygame       PASS
```

**Phase 1–3 behaviour, re-verified:**

| Area | Check | Result |
| --- | --- | --- |
| Determinism | `engine-game::determinism`, 4 tests | pass — including with the new cast events in the ordered snapshot |
| Headless simulation | `engine-game::headless_simulation`, 6 tests | pass, still no renderer in the graph |
| Gameplay pipeline | `game_bench`, 5 scenarios | see below |
| Culling | `cull_bench --objects 100000` | 6.42 ns/object at 50% visible |
| Renderer | 19 GPU integration suites, debug layer on | pass |
| Instanced vs per-object | 5 pixel-equivalence tests | pass — the tint is implemented identically in both shaders |
| Showcase (Phase 2 scene) | `--frames 3000 --preset 4`, 3 runs | 0.431 ms median (Phase 2 baseline 0.401–0.514) — **no regression** |

**`game_bench`, against the Phase 3 baseline:**

| Scenario | Sim µs (P3) | Extraction µs (P3) | Culling µs (P3) | Allocations |
| --- | --- | --- | --- | --- |
| 1 dummy | 0.385 (0.40) | 0.149 (0.12) | 0.070 (0.07) | 0 (0) |
| 100 | 2.274 (2.24) | 2.844 (2.16) | 0.982 (0.84) | 9 (9) |
| 1,000 | 20.078 (19.22) | 27.766 (20.80) | 7.085 (5.99) | 8 (8) |

**Two regressions, both understood, both paid for deliberately:**

* **Extraction, +33% at 1,000 entities** (20.8 → 27.8 µs). Two causes, in equal parts: the
  per-entity style computation from Milestone 7, and the interpolation reads from
  Milestone 9. It buys zero render churn, health tints, target glow, hit flash and a smooth
  1,400 FPS from a 60 Hz simulation.
* **Culling, +16%** (5.54 → 6.42 ns/object). The `RenderList` gained a 20-byte `styles`
  column, so the cull's gather copies more per survivor.

Both are inside a gameplay pipeline that totals **under 56 µs per tick at 1,000 entities**,
against a 4,167 µs frame budget, in a scene fifty times the size of the game's own. No
regression was ignored; these two were measured, attributed and accepted.

---

### Milestone 24 — Final visual verification

Status: COMPLETE

The game was launched at 1280x720, focused, and driven with synthetic input. Screenshots were
captured from the live window.

| # | Check | Result |
| --- | --- | --- |
| 1 | Player moves smoothly | **PASS** — and now interpolated; the character no longer steps at 60 Hz |
| 2 | Player jumps smoothly | **PASS** |
| 3 | Target selection works | **PASS** — TAB cycles; frame, outline, marker and enemy glow all follow |
| 4 | Target HP updates | **PASS** — `2000/2000 100%` → `1892/2000 95%` as damage lands |
| 5 | Player HP/resource UI updates | **PASS** — `HP 500/500`, `MP 100/100` → `67/100` after casting |
| 6 | Ability bar works | **PASS** — ten slots, hotkeys `1`–`0`, names `FIRE`…`SMIT` |
| 7 | Cooldowns work | **PASS** — wipe plus `4.8` / `7.6` counting down on slots 2 and 3 |
| 8 | Cast bar works | **PASS** — `LANCE` filling with `0.6S` remaining, above the ability bar |
| 9 | Damage numbers appear | **PASS** — `45` over the dummy on Fireball |
| 10 | Damage numbers fade | **PASS** — `10` from Ignite's impact caught mid-fade |
| 11 | Target feedback works | **PASS** — outline, marker, and the target visibly brighter than its neighbours |
| 12 | AoE produces multiple numbers | **PASS** — Detonate on a burning group produced **`20  20  20  98`** simultaneously |
| 13 | Combat meter remains correct | **PASS** — F5 overlay totals agree with the health actually lost |
| 14 | Reset still works | **PASS** — **R** restores health, cooldowns, resource and the meter |
| 15 | UI responsive at high FPS | **PASS** — `CPU mean 0.200 ms (5008 FPS) … ui 0.004 ms (214 quads, 77 cmds) … draws 5` |

Screenshots: `hud_damage.png` (target frame, damage number, cooldown wipe, player bars),
`hud_casting.png` (cast bar, burning status, three cooldowns), `aoe_numbers.png` (four
simultaneous damage numbers). They live in the session scratchpad and are not committed.

**Observed defect, unchanged from Phase 3:** nameplates for the group of four overlap into an
unreadable run. Recorded as a known limitation; the fix is a screen-space push-apart pass.

---

## Current baseline

See Milestone 2 above. Summary of the numbers Phase 4 must beat or hold:

```text
Normal scene   (4 dummies,   5 plates):  CPU 0.239 ms mean / 0.866 p99   GPU 0.086 ms
UI scene       (20 dummies, 20 plates):  CPU 0.348 ms mean / 1.113 p99   GPU 0.148 ms
Stress scene   (1000 dummies, 20 plates): CPU 0.540 ms mean / 1.734 p99  GPU 0.249 ms
Cost of 20 nameplates:  +0.15..0.23 ms CPU mean, +0.50..0.70 ms CPU p95, +0.08 ms GPU
Draw calls: 5-6.  Gameplay: 0.044 ms of the frame at 1,000 entities.
```

## Performance after implementation

Release, 1920x1080, vsync off, three interleaved rounds of 6,000 frames, median reported.
`--no-ui` is the control, so the interface's cost is the difference between two runs of one
binary.

**CPU frame time, milliseconds — before (Milestone 2) and after:**

| Scene | mean before | **mean after** | p50 before | **p50 after** | p95 before | **p95 after** | p99 before | **p99 after** |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 4 dummies | 0.239 | **0.219** | 0.206 | **0.194** | 0.439 | **0.390** | 0.866 | **0.762** |
| 20 dummies | 0.348 | **0.245** | 0.295 | **0.215** | 0.760 | **0.454** | 1.113 | **0.843** |
| 100 dummies | 0.386 | **0.272** | 0.320 | **0.239** | 0.968 | **0.547** | 1.326 | **0.885** |
| 500 dummies | 0.466 | **0.370** | 0.394 | **0.320** | 1.085 | **0.876** | 1.271 | **1.081** |
| 1000 dummies | 0.540 | **0.476** | 0.454 | **0.403** | 1.218 | **1.094** | 1.734 | **1.472** |

Before, the interface was **nameplates only**. After, it is nameplates, world-space health
bars, a target frame, player health and resource bars, ten ability slots with cooldown wipes,
a cast bar and up to 128 floating damage numbers. Every column improved anyway.

**GPU frame time, milliseconds:**

| Scene | mean before | **mean after** | p50 | p95 | p99 |
| --- | --- | --- | --- | --- | --- |
| 20 dummies | 0.148 | **0.091** | 0.082 | 0.086 | 0.377 |
| 1000 dummies | 0.249 | **0.202** | 0.181 | 0.189 | 0.605 |

**FPS, from the CPU mean:**

| Scene | before | **after** |
| --- | --- | --- |
| 4 dummies | 4,184 | **4,566** |
| 20 dummies | 2,874 | **4,082** |
| 100 dummies | 2,591 | **3,676** |
| 1000 dummies | 1,852 | **2,101** |

**Draw calls:** 5 in the game's own scene, of which the **entire interface is 1** — from
419 quads to 20,990, measured. Before: 2 UI layers, each a separate submission of per-glyph
geometry.

**Allocations:**

| Path | Per frame |
| --- | --- |
| Combat HUD layout + batching (`ui_bench`, every scene size) | **0** |
| UI benchmark, 20 to 1,000 synthetic elements | **0** |
| Gameplay pipeline (`game_bench`) | 0 to 0.003 per tick, unchanged from Phase 3 |

**UI benchmark, synthetic scaling:**

| Elements | Commands | Quads | CPU mean | p50 | p95 | p99 | GPU cost | Draw calls | Allocs |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 20 | 104 | 419 | 2.47 µs | 2.50 | 2.50 | 2.60 | noise floor | 2 vs 1 | 0 |
| 100 | 524 | 2,099 | 12.57 µs | 12.60 | 12.70 | 13.50 | +0.037 ms | 2 vs 1 | 0 |
| 500 | 2,620 | 10,495 | 62.86 µs | 62.40 | 63.70 | 71.50 | +0.162 ms | 2 vs 1 | 0 |
| 1,000 | 5,240 | 20,990 | 125.91 µs | 125.40 | 127.70 | 137.70 | +0.306 ms | 2 vs 1 | 0 |

**Combat HUD benchmark, the real interface:**

| Dummies | Commands | Quads | CPU mean | p50 | p95 | p99 | Allocs |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 20 | 271 | 725 | 12.27 µs | 12.30 | 12.30 | 12.90 | 0 |
| 1,000 | 270 | 719 | 35.98 µs | 36.00 | 36.20 | 37.00 | 0 |

**Against the targets:**

| Goal | Budget | Game scene p99 | Stress scene p99 | Verdict |
| --- | --- | --- | --- | --- |
| 240 FPS minimum | 4.167 ms | 0.762 ms | 1.472 ms | **realistic, with 65% spare at 50x the scene size** |
| 480 FPS aspirational | 2.083 ms | 0.762 ms | 1.472 ms | **aspirational and currently met**, 63% spare in the game's own scene |

## Known limitations

* **Nameplates overlap.** Two enemies in a line produce two plates that run together, visible
  in the arena's own group of four. A screen-space push-apart pass is the fix; it is cosmetic
  and was not worth spending the phase's budget on.
* **The developer statistics overlay still uses the Phase 3 per-glyph path.** It is a debug
  readout, off by default, behind a feature — but it means two text renderers exist in the
  binary while `debug-overlay` is on.
* **Extraction is 33% slower** than Phase 3 and **culling 16% slower**, for the per-instance
  style and the interpolation reads. Measured, attributed, accepted.
* **Ability slots have no icons.** Four letters stand in. The layout does not change when
  textures arrive — that was the point of building the bar now.
* **The interface is linear in the world, not in itself.** The nameplate builder scans every
  entity to find the nearby ones, which is why the HUD costs 36 µs at 1,000 dummies and 12 µs
  at 20 despite drawing the same number of elements. A spatial index would fix it and nothing
  yet needs one.
* **No blended scene pass.** Translucent *world* geometry would need its own back-to-front
  list after the opaque pass. The blend state, the ordering rule and per-instance alpha all
  exist; nothing builds that list.
* **No scissor or clipping.** An element positioned off-screen is submitted and rasterizes to
  nothing rather than being rejected on the CPU.
* **Damage numbers do not merge.** Twenty hits on one target in a second produce twenty
  numbers rather than a running total.
* **One font, one size, ASCII, uppercase.** The atlas is what a HUD needs and is not a text
  engine.
* **The cast bar has no interrupt flourish.** `CastEnd` distinguishes why a cast ended and
  the UI ignores everything but the progress.

## Phase 5 recommendations

Do not start these yet; this is the handover, not a plan of record.

1. **The asset pipeline is the natural next phase, and the seams for it exist.** `MeshData`
   is already the shape a loader produces, `create_mesh` already uploads it, and
   `StagingUploader::queue_texture` — written this phase for the font atlas — is already the
   path a texture takes to the GPU. The work is a glTF reader, an asset crate, and a material
   that samples a texture rather than a constant.
2. **Textures need what the UI pass already proved.** The UI pipeline has a descriptor table,
   a static sampler and an SRV; the scene pipeline has none of the three. Copying that
   arrangement into the mesh pipeline is a known-good change rather than a design problem.
3. **Land skeletal animation one character at a time, and measure between each step.** It is
   the single largest risk to the 480 FPS target: a bone palette per instance, a second
   vertex path, and CPU pose evaluation per character. `MovementState` is already derived
   every tick and is the animation selector waiting for clips.
4. **Fix the nameplate scan before the world gets bigger.** The HUD's cost is linear in the
   entity count because finding nearby enemies is a full scan. A spatial index is cheap now
   and gets more expensive with every system that wants one.
5. **A blended scene pass, when there is something to put in it.** Back-to-front, after the
   opaque pass, its own list. The blend state is correct already.
6. **Keep asserting p99.** Every regression this phase found — the 0.43 ms audit figure, the
   per-plate allocation, the sort's merge buffer — was invisible in a mean.
7. **Do not introduce an ECS, a job-based simulation or a custom allocator** without a
   measurement that demands it. The gameplay pipeline is still under 56 µs per tick at 1,000
   entities, and the interface allocates nothing.

## Final acceptance checklist

### Rendering foundation

* [x] Font atlas exists — `engine-renderer::font_atlas`, 256x32 R8, 96 cells, 8 KiB.
* [x] UI rendering is batched — one vertex buffer, one pipeline, one texture.
* [x] UI does not submit one draw per glyph — **one draw call for 20,990 quads**, asserted on
      the GPU in `ui_draw.rs`.
* [x] Per-instance material data exists — `InstanceStyle`, in both draw paths.
* [x] Entity visual state does not require entity respawn — asserted by
      `a_long_fight_causes_no_render_entity_churn_at_all`: 600 ticks, 0 spawns, 0 despawns.
* [x] Alpha blending exists — proven by offscreen pixel readback, not by draw counts.
* [x] UI supports transparency — translucent panels, cooldown wipes and fading numbers.

### Presentation architecture

* [x] Gameplay does not directly construct UI rendering resources — `engine-game` depends on
      `engine-core` and `log`, unchanged.
* [x] UI reads a presentation snapshot — `GameUi` never takes a `&Simulation`.
* [x] Render extraction uses fixed timestep interpolation — `FixedTimestep::alpha`, previously
      unread, now drives every drawn position.
* [x] Gameplay/rendering seam remains intact — `cargo tree` unchanged; no gameplay noun
      appears in `engine-renderer`.

### Combat UI

* [x] Player HP bar. * [x] Player resource bar. * [x] Target frame.
* [x] Target HP bar. * [x] World-space dummy HP bars. * [x] Ability bar.
* [x] Cooldown visualization. * [x] Cast bar.

### Combat feedback

* [x] Damage numbers. * [x] Damage fade. * [x] Target marker.
* [x] Target visual state. * [x] Hit feedback. * [x] AoE feedback — verified on screen as
      `20 20 20 98`.

### Performance

* [x] UI benchmark exists — `ui_bench`, six sizes, CPU and GPU phases.
* [x] Combat presentation benchmark exists — the `COMBAT HUD` phase of the same binary.
* [x] Mean reported. * [x] p50 reported. * [x] p95 reported. * [x] p99 reported.
* [x] Allocations measured — **0 per frame** for the whole interface.
* [x] Draw calls measured — **1** for the whole interface.
* [x] No unexplained major regression — two regressions found, both attributed and recorded.
* [x] 240 FPS remains a realistic target — 65% spare at p99 in a scene 50x the game's own.
* [x] 480 FPS remains an aspirational target — currently met, 63% spare in the game's scene.

### Testing

* [x] Existing tests pass — 793 Phase 3 tests, all still green.
* [x] New UI tests pass. * [x] Presentation snapshot tests pass.
* [x] Interpolation tests pass. * [x] Material-instance tests pass.
* [x] Alpha/blending renderer tests pass. * [x] Combat presentation tests pass.
* [x] Regression tests pass — **927 passed, 0 failed, 0 ignored.**
