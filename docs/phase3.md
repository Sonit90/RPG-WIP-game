# Phase 3 — Gameplay and combat foundation

**Phase 3 is complete.** The `mygame` executable launches into a playable local prototype:
a character in a training arena that moves, jumps, targets, uses ten abilities with
cooldowns and costs, damages training dummies with single-target, damage-over-time,
area-of-effect and synergy mechanics, and reports it all in a developer overlay with a
Recount-style damage meter.

The headline result, 1,000 gameplay entities at 1920×1080 windowed: **2941 FPS**, with the
simulation costing **19 µs per 60 Hz tick** — 0.06% of a 300 FPS frame budget.

## Mission

Phase 2 ended with a renderer whose cost is known:

```text
RenderWorld -> Culling -> RenderList -> Renderer -> D3D12
```

Phase 3 put a game in front of it:

```text
Input -> Commands -> Gameplay simulation -> RenderWorld -> Culling -> RenderList
      -> Renderer -> D3D12 -> GPU
```

No networking. No ECS. No custom allocator. Each of those was considered and rejected on
evidence, recorded below.

---

## Architecture

### Dependency graph

```text
                        engine-core
                       /     |      \        \
                      /      |       \        \
        engine-platform      |   engine-render-data
                      \      |      /  ^        ^
                       \     |     /   |        |
                     engine-renderer   |    engine-game   (simulation, no D3D12)
                             ^         |        ^
                             |         |        |
                             |    engine-presentation  (extraction, camera, HUD text)
                             |              ^
                             \             /
                              engine-app  (mygame)
```

Two new crates:

| Crate | Purpose | Depends on |
| --- | --- | --- |
| `engine-game` | The gameplay simulation. Entities, movement, combat, abilities, statuses, events, telemetry, the fixed timestep loop. | `engine-core` only |
| `engine-presentation` | The seam between gameplay and rendering: render extraction, the third-person camera controller, HUD text. | `engine-core`, `engine-game`, `engine-render-data` |

`engine-game` depends on `engine-core` and nothing else — not `windows`, not
`engine-platform`, not `engine-renderer`, and deliberately **not** `engine-render-data`
either. It cannot name a `MeshHandle`, so it cannot start owning one. Checkable:

```text
cargo tree -p engine-game
engine-game
├── engine-core
│   └── log
└── log
```

`engine-presentation` is the only place that knows both worlds. It links no D3D12 either,
because `engine-render-data` does not, so extraction, the camera and the HUD are all unit
tested with no device.

### The frame

```text
process_events   platform messages          -> InputState
update  [Input]       InputState            -> PlayerCommand      (engine-game::input_map)
        [Simulation]  PlayerCommand         -> N x fixed 60 Hz tick
                                               movement -> cooldowns -> abilities
                                               -> statuses -> deaths -> CombatEvent
                                               -> CombatMeter
        [Extraction]  Simulation            -> RenderWorld        (engine-presentation)
        [Camera]      player position       -> Camera
render                RenderWorld           -> cull -> RenderList -> D3D12
present               swap chain
```

### Simulation / render separation

| Gameplay (`engine-game`) | Presentation (`engine-render-data`) |
| --- | --- |
| position, velocity, grounded, facing, movement state | `Transform` |
| health, max health, alive | `MeshHandle`, `MaterialHandle` |
| abilities, cooldowns, resource, target | bounds, visibility |
| statuses, stacks, remaining time | — |
| faction, entity kind, radius, height | — |

Extraction maps one to the other. Gameplay never writes a `RenderWorld` field and the
renderer never sees a `Health`; neither crate can name the other's types.

### Determinism and future networking

* Input becomes **commands** before it reaches the simulation. `engine-game::input_map` is
  the only module in the project that knows which key does what, so a replay or a network
  client supplies the same commands from another source.
* The simulation advances in **fixed 60 Hz ticks** from an accumulator
  (`engine_core::time::FixedTimestep`). Gameplay never reads the render frame delta.
* No RNG anywhere in the simulation.
* Every state change is announced as a `CombatEvent`, so telemetry — and later VFX, a combat
  log or replication — observes rather than hooks.

### Decisions and their reasons

| Decision | Why |
| --- | --- |
| 60 Hz fixed tick | 16.67 ms is finer than any ability timing in the design and coarse enough to be invisible in the frame budget. A frame-rate-tied simulation would give a player at 1400 FPS a different jump arc than one at 240 FPS. |
| No ECS | Two entity kinds, one component set, `Vec` per attribute indexed by a slot allocator. Measured to 1,000 entities: 19 µs a tick. An archetype system would spend its machinery on a world that has no archetypes. |
| Player body lives in the world | Not duplicated into `Player`. Two truths about where the character is would make the movement system pick one. |
| Abilities are data | `AbilityDef` in a slot; the controller emits `UseAbility(slot)`. Ten slots cost the same as three, and the synergy works because Detonate reads a status rather than knowing about Ignite. |
| Damage is an event | The meter reads a slice. Nothing in the ability system mentions telemetry, which is the Milestone 10 requirement. |
| Statuses refresh, not stack | Two burns from one caster ticking independently doubles the value of spamming one button — a balance decision this prototype has no reason to make. |
| Death is not a despawn | The meter, the overlay and the reset control all need the entity to still be there. A corpse falls over and changes material. |

---

## Milestone log

### Milestone 1 — Gameplay crate / simulation boundary

Status: COMPLETE

**Implemented** — `crates/engine-game`, depending on `engine-core` alone. Module map in its
`lib.rs`. `Simulation` owns the world, the player, the fixed timestep, the event stream, the
combat meter and the target-query scratch.

**Tests** — the whole crate's 125 unit tests and 10 integration tests compile and run
without `engine-renderer`, `engine-platform` or `windows` in the dependency graph.

**Architectural decisions** — the crate cannot depend on `engine-render-data`. That is
stricter than the phase brief asked for and it is what stops gameplay from acquiring a mesh
handle "just for the extraction step".

**Known limitations** — one simulation per process; no scenario loader.

**Files** — `crates/engine-game/{Cargo.toml,src/lib.rs,src/sim.rs}`.

---

### Milestone 2 — Entity identity

Status: COMPLETE

**Implemented** — `EntityId` (index + generation, 8 bytes, `Copy`, `Ord`, `Hash`) and
`EntityAllocator` with a free list. Generation 0 is reserved for `EntityId::INVALID`, so a
default-constructed id can never address a live slot.

**Tests** — creation, uniqueness across 64 live ids, invalid id, free, double free (which
would otherwise corrupt the free list and hand one slot to two entities), stale access after
slot reuse, and clear.

**Architectural decisions** — no pointers, no `Rc<RefCell<_>>`. A combat event outlives the
entity it names, and a stale id must be *detected* rather than dereferenced.

**Known limitations** — the generation wraps at `u32::MAX`; a slot would have to be reused
four billion times in one session.

**Files** — `crates/engine-game/src/entity.rs`.

---

### Milestone 3 — Player state

Status: COMPLETE

**Implemented** — `Body` (position of the feet, velocity, grounded, facing yaw, movement
state) in `GameWorld`'s arrays, and `Player { entity, abilities, resource, target,
last_error, last_ability }` in the simulation.

**Tests** — spawn carries every field; every accessor reports `None` for a despawned entity;
a stale id cannot damage the entity that replaced it; `restore` returns a dummy to its spawn
state.

**Architectural decisions** — the player's spatial state is *not* duplicated into `Player`.
It lives in the world beside every other entity's, so the movement system has one input.
`Simulation::player_body()` reads it.

**Known limitations** — no mesh or material on the gameplay player, by design.

**Files** — `crates/engine-game/src/{world.rs,sim.rs}`.

---

### Milestone 4 — Input → commands

Status: COMPLETE

**Implemented** — `PlayerCommand::{Move, Jump, UseAbility, SelectNearestTarget, CycleTarget,
ResetDummies}`, `CommandBuffer`, and `Bindings::write_commands`, which is the only code in
the project that maps a key to an action. Movement is camera-relative: the command carries
the camera yaw and `move_direction()` resolves it, so a replayed command produces the same
direction the live one did.

**Tests** — WASD produces one held intent rather than one command per key; opposite keys
cancel; releasing everything still issues a neutral `Move` (its absence must not mean "keep
running"); jump and every ability key are edge-triggered; all ten ability keys are distinct
and map to their own slot; an idle frame costs exactly one command.

**Architectural decisions** — `Move` is a *held* intent replaced by the newest one;
everything else is discrete and applied on the first tick after arrival. A catch-up batch of
eight ticks must not turn one jump press into eight jumps, which is tested.

**Files** — `crates/engine-game/src/{command.rs,input_map.rs}`.

---

### Milestone 5 — Character movement

Status: COMPLETE

**Implemented** — `movement::step`: horizontal acceleration towards intent, deceleration
without it, reduced air control, gravity, jump from the ground only, terminal velocity,
ground contact, arena confinement, and facing derived from velocity. `MovementState` is
derived every step rather than stored.

**Tests** — 13, including the sequence the brief asks for as one story:
`standing → jump → airborne → fall → grounded`, with the apex compared against
`jump_height()` computed from the configuration. Also: top speed is reached and not
exceeded, diagonal movement is not faster, a held jump key is not a flight control, a body
spawned 30 m up never passes the floor, air control is weaker than ground control but not
zero, the arena confines without trapping, and a zero-length step changes nothing.

**Performance** — 1,000 bodies integrate in ~15 µs a tick (the movement share of the
simulation column below).

**Architectural decisions** — position is the **feet**, so ground contact is `y <= 0` and
the mesh offset is presentation's problem. Order within a step is load-bearing: jump before
gravity, ground contact after integration.

**Known limitations** — a flat floor and a square boundary. No slopes, no obstacles, no
entity-to-entity collision.

**Files** — `crates/engine-game/src/movement.rs`.

---

### Milestone 6 — Gameplay → RenderWorld extraction

Status: COMPLETE

**Implemented** — `crates/engine-presentation` and `RenderExtractor`. Each gameplay slot
keeps its render entity for as long as the gameplay entity lives; a slot whose generation or
alive-state changed is despawned and respawned, because a `RenderWorld` entity's material is
fixed at spawn. The floor and the target marker are permanent entities.

**Tests** — every gameplay entity becomes something drawable; the player is drawn standing
on the floor facing where it moves; **damage alone changes nothing about what is drawn**; a
dead dummy falls over and changes material; a despawned entity stops being drawn; a reused
gameplay slot gets its own render entity; the target marker follows and hides; a steady
scene spawns and despawns nothing; 1,000 dummies extract with no device.

**Performance** — 0.116 µs at 3 entities to 20.8 µs at 1,002.

**Architectural decisions** — extraction is incremental rather than clear-and-refill, which
keeps the culler's slot ordering stable across frames.

**Files** — `crates/engine-presentation/src/extract.rs`.

---

### Milestone 7 — Third-person camera

Status: COMPLETE

**Implemented** — `ThirdPersonCamera`: yaw, pitch (clamped to −1.2..1.1 rad), orbit distance
(2.5–25 m, mouse wheel), field of view, and an exponentially smoothed focus at chest height.
It drives an `engine_render_data::Camera` by setting a position and a rotation; the camera
learns nothing about a player.

**Tests** — the eye sits behind and above the character; the camera looks at the character
(asserted through the real view matrix); yaw orbits rather than translating; pitch and
distance clamp; **following is frame-rate independent** (60 FPS and 480 FPS reach the same
focus after the same elapsed time); a zero-delta first frame places the focus exactly.

**Architectural decisions** — the camera is presentation, not gameplay. Where it points
changes what "forward" means, and that is carried into the simulation as a yaw on the
movement command rather than as a camera the simulation can see. It is followed at the
*render* rate, which is what stops a 60 Hz simulation from looking like 60 FPS.

**Known limitations** — no collision: the camera passes through the floor at extreme pitch
and would pass through walls if there were any.

**Files** — `crates/engine-presentation/src/camera.rs`.

---

### Milestone 8 — Training arena

Status: COMPLETE

**Implemented** — `arena::training_arena`: a player at the origin, a lone dummy 12 m ahead,
and a group of four 22 m away, laid out on a deterministic grid. `ArenaConfig::with_group(n)`
scales the group and grows the arena to fit it. The floor is not an entity — it is the
`ground_height` the controller stands on; the visible plane is presentation's.

**Tests** — the arena has a player, a lone dummy and a group; the nearest dummy starts
targeted; **the group is inside one blast radius of itself and the lone dummy is not** (the
property that makes area damage observable by playing); every dummy is hostile, alive and on
the floor; the layout is deterministic; 1,000 dummies form a grid that fits inside the arena
with no two sharing a position.

**Architectural decisions** — the arena is gameplay data, so a benchmark and the game build
the same one.

**Files** — `crates/engine-game/src/arena.rs`.

---

### Milestone 9 — Combat entities

Status: COMPLETE

**Implemented** — `Health` (current, max, invariants enforced in one place), `Resource`
(pool, regeneration, payment), `Faction`, `DamageType`, `DamageResult { applied, overkill,
killed }`.

**Tests** — damage reduces health by exactly what it reports; zero health is death and is
reported once (a second hit on a corpse must not report a second kill); overkill is reported
rather than clamped away; **invalid damage does nothing instead of healing** — a negative
damage number that healed is how a damage buff becomes immortality; healing stops at the
maximum and never revives; a degenerate maximum is rejected rather than dividing by zero;
restore; resource spend, refusal and regeneration.

**Files** — `crates/engine-game/src/combat.rs`.

---

### Milestone 10 — Damage events

Status: COMPLETE

**Implemented** — `CombatEvent::{AbilityUsed, Damage, Healing, StatusApplied, StatusRemoved,
Death}` and `DamageEvent { source, target, amount, overkill, ability, damage_type, killed,
tick }`. `EventStream` is a buffer that is cleared, never freed, with a cursor so a per-tick
observer reads only its tick while the whole frame's events stay readable.

**Tests** — emission order; the cursor reads only what came after it; clearing keeps the
allocation and the lifetime counter; every variant reports its tick.

**Architectural decisions** — events are values, not callbacks. A callback would run inside
the ability system, and an observer that mutated the world from there would make the order of
two abilities in one tick depend on registration order.

**Files** — `crates/engine-game/src/event.rs`.

---

### Milestone 11 — Ability framework

Status: COMPLETE

**Implemented** — `AbilityDef { id, name, cooldown, cost, range, targeting, effect,
damage_type, placeholder }`, `AbilityBook` (ten definitions plus ten cooldowns, fixed
arrays), `AbilityError` naming all seven refusals, and `Simulation::use_ability`, which
validates, resolves targets, spends, and emits.

**Tests** — every slot exists and names itself; ability execution is driven entirely by the
data in the slot; the pipeline `UseAbility(slot) → validation → execution → CombatEvents`
runs headlessly.

**Architectural decisions** — no ability-specific code in the player controller. The
validation order is deliberate — unknown slot, caster dead, cooldown, resource, then
targeting — so the most actionable refusal is the one reported.

**Files** — `crates/engine-game/src/ability.rs`, `crates/engine-game/src/sim.rs`.

---

### Milestone 12 — Ability timing and cooldowns

Status: COMPLETE

**Implemented** — per-slot cooldowns ticked by the fixed timestep, `cooldown_remaining`,
`cooldown_fraction` for the overlay, `is_ready`, and a resource cost checked before anything
is spent.

**Tests** — available initially; unavailable after use; the cooldown decreases with
simulation time and stops at zero rather than going negative; available again; **cannot be
spammed** (`Err(OnCooldown)`); one slot's cooldown does not affect another; a refused
ability spends neither resource nor cooldown; and — the important one — cooldowns advance
identically whether a simulated second arrives as 300 frames or 30.

**Architectural decisions** — a `TIME_EPSILON` of 0.1 ms snaps a countdown to zero. A fixed
step of 1/60 s is not exactly representable in `f32`, and without the snap a "finished"
cooldown is still 0.0000003 s from ready and a six-second burn ticks five times. This was
found by a failing test, not by reasoning.

**Files** — `crates/engine-game/src/ability.rs`, `crates/engine-game/src/lib.rs`.

---

### Milestone 13 — First three abilities

Status: COMPLETE

**Implemented** — three abilities demonstrating three different mechanics:

| Slot | Name | Cooldown | Cost | Mechanic |
| --- | --- | --- | --- | --- |
| 1 | FIREBALL | 1.5 s | 12 | Direct single-target damage, 45 |
| 2 | IGNITE | 6 s | 18 | 10 on impact, then Burning: 8 a second for 6 s |
| 3 | DETONATE | 8 s | 25 | Area, 8 m radius: 20 base, **+15 per second of Burning remaining**, consuming it |

**Tests** — each effect type is exercised; ignite's burn damage is credited to IGNITE rather
than to an anonymous damage-over-time bucket; **detonate on a burning target deals more than
twice what it deals on a cold one**, and the burn is gone afterwards; the synergy is
observable from the event stream alone (`StatusApplied` → `StatusRemoved { Consumed }` →
damage above base), which is what a future VFX or replication layer would see.

**Architectural decisions** — Detonate does not know Ignite exists. It consumes a
`StatusKind::Burning` whoever applied it, which is what makes the framework a framework.

**Files** — `crates/engine-game/src/{ability.rs,status.rs,sim.rs}`.

---

### Milestone 14 — Targeting

Status: COMPLETE

**Implemented** — `Targeting::{SingleTarget, AreaAroundTarget { radius }, SelfCentered
{ radius }}` and `TargetQuery`, which gathers into reusable buffers. Range tests are
edge-corrected (a wide target is reachable from its edge) and horizontal (jumping cannot put
a target out of range). Selection is `SelectNearestTarget` / `CycleTarget`, and the
simulation keeps the nearest hostile selected when nothing valid is.

**Tests** — valid target; **every refusal is named and asserted** — no target, out of range,
an ally, a corpse, a dead caster; nearest-first ordering; cycling walks the list and wraps;
cycling from a dead target starts over; an area ability hits a cluster of four and misses a
dummy 30 m away; an area ability that finds nobody is a miss rather than an error; repeated
resolution does not reallocate.

**Performance** — see Milestone 24: the first implementation sorted every hostile by distance
on every tick with an insertion sort, which was measured at a 398 µs p99 against a 17 µs
median at 1,000 dummies. `nearest_hostile` became a single scan and the ranking became
`sort_unstable_by` over an array of pairs. **p99 fell from 398 µs to 49 µs and the mean from
39 µs to 19 µs.**

**Files** — `crates/engine-game/src/targeting.rs`.

---

### Milestone 15 — Multi-target damage

Status: COMPLETE

**Implemented** — area abilities damage every hostile whose edge is within the radius, in
slot order, emitting one `AbilityUsed` with the target count and one `Damage` per target.

**Tests** — the arena's group of four all take damage from one cast while the lone dummy
outside the radius takes none, asserted in both the targeting unit tests and end to end in
the simulation tests.

**Files** — `crates/engine-game/src/{targeting.rs,sim.rs}`.

---

### Milestone 16 — Combat telemetry event stream

Status: COMPLETE

**Implemented** — the tick emits into `EventStream`; `CombatMeter::observe` reads a slice at
the end of the tick. Nothing in the ability system mentions the meter.

**Tests** — status events move no damage number; healing is tracked apart from damage; a
steady stream of events does not reallocate; and the end-to-end check that matters — **the
meter's total equals the health the dummies actually lost**, asserted in
`headless_simulation.rs`.

**Architectural decisions** — an ability used outside a tick (tests, a future scripted
encounter) is flushed to the meter before the next batch clears the stream. Damage applied to
the world and missing from the meter is the one inconsistency a damage meter must never have.

**Files** — `crates/engine-game/src/{event.rs,telemetry.rs,sim.rs}`.

---

### Milestone 17 — Recount-style damage meter

Status: COMPLETE

**Implemented** — total damage, DPS, per-ability totals (damage, healing, casts, hits),
per-target totals (damage, hits, died), overkill, deaths, and session reset. Per-target
attribution is an entity-slot index rather than a scan, so a 1,000-target area hit is 1,000
lookups rather than 500,000 comparisons.

**Tests** — accumulation by total, ability and target; **DPS uses simulation time between
the first and last hit**; a single instant of damage reports 45 DPS rather than an absurd
rate (a one-second floor); casts and hits are counted separately — one area cast on four
dummies is one cast and four hits; ranking; reset keeps every allocation; a reused entity
slot gets its own row rather than inheriting the dead occupant's damage.

**Architectural decisions** — every timestamp is simulation time. A per-frame or wall-clock
rate would report a different number on a faster machine for identical combat.

**Files** — `crates/engine-game/src/telemetry.rs`, `crates/engine-presentation/src/hud.rs`.

---

### Milestone 18 — Dummy reset

Status: COMPLETE

**Implemented** — **R** issues `PlayerCommand::ResetDummies`, which restores every dummy's
health, position and velocity, clears their statuses, clears every cooldown, refills the
resource, and starts a new meter session.

**Tests** — the reset returns the arena to its starting state (health, position, statuses,
cooldowns, meter, DPS), and a determinism test asserts the state afterwards equals the state
a freshly built arena has.

**Architectural decisions** — cooldowns and resource are reset too. The point of the control
is to try a rotation again from a known state, and waiting eight seconds for Detonate is not
part of that state.

**Files** — `crates/engine-game/src/sim.rs`.

---

### Milestone 19 — Developer ability UI

Status: COMPLETE

**Implemented** — the overlay's ability bar: all ten slots, two per row, each showing its
hotkey digit, name and either `READY` or the remaining cooldown to a tenth of a second. Plus
energy, the current target and its health, and the reason the last cast was refused.

**Tests** — every slot appears with its name; at least one reads `READY`; the exact
remaining cooldown of a recently cast ability appears; a refused cast prints `ON COOLDOWN`;
**every character has a glyph in the renderer's 5×7 font**; rewriting reuses the same
`String` buffers; the overlay fits 28 lines by 56 columns.

**Files** — `crates/engine-presentation/src/hud.rs`, `crates/engine-app/src/app.rs`.

---

### Milestone 20 — Remaining seven ability slots

Status: COMPLETE

**Implemented** — ten slots from the start. Slots 4–10 are working placeholders (STRIKE,
CLEAVE, SHOCK, LANCE, QUAKE, NOVA, SMITE) with distinct cooldowns, costs, ranges, damage
types and — for the melee-shaped ones — self-centred area targeting. They validate, cost,
cool down and report to the meter exactly like the designed three.

**Tests** — every slot exists, is bound to its own key, and fires only its own ability;
exactly three are non-placeholder; the loadout's three designed abilities use three distinct
effect types.

**Architectural decisions** — the requirement was to prove the system supports ten abilities
without architectural change. `AbilityBook` is `[AbilityDef; 10]` and `[f32; 10]`; filling a
slot in is a data change.

**Files** — `crates/engine-game/src/ability.rs`.

---

### Milestone 21 — Simulation/render decoupling verification

Status: COMPLETE

**Implemented** — `crates/engine-game/tests/headless_simulation.rs`. It links `engine-game`
and `engine-core` only, so a future change that makes the simulation depend on the renderer
stops it compiling.

**Tests** — six, covering `Input → Command → Simulation → Ability → Damage → CombatEvent →
Telemetry` with no device, window or GPU: a keypress becomes damage; movement and jumping;
the full three-ability rotation with the meter cross-checked against the health lost; the
synergy read from the event stream alone; 1,000 dummies simulated; the reset control.

**Architectural decisions** — this is the acceptance criterion the whole crate split exists
for, and it is enforced by the compiler rather than by review.

---

### Milestone 22 — Determinism tests

Status: COMPLETE

**Implemented** — `crates/engine-game/tests/determinism.rs`, with a fixed-point `Snapshot`
of tick, time, player body, resource, all ten cooldowns, every dummy's health and burn
remaining, and the whole meter. Fixed-point because two `f32`s differing in the last bit are
not the same state, and comparing them loosely is what makes a determinism test useless.

**Tests** — the scripted sequence `Move, Move, Jump, UseAbility1, UseAbility2, UseAbility3`
run twice from the same initial state produces an identical snapshot after 600 ticks; the
same script produces **the same events in the same order** (state equality alone would allow
two runs to arrive at the same health by different routes); the frame rate does not change
the outcome (960 frames of 2.08 ms against 120 frames of 16.67 ms, identical totals); a reset
returns to the state a fresh arena has.

**Results** — all four pass. There is no RNG in the simulation; if one is introduced it must
be an explicit simulation RNG seeded from simulation state.

---

### Milestone 23 — Performance instrumentation

Status: COMPLETE

**Implemented** — the engine loop already scoped `Events`, `Update`, `Render`, `Present`.
`engine-app::update` now scopes `Input`, `Simulation`, `Extraction` and `Camera`, and the
simulation measures its own internals into `SimStats { ticks, total, movement, abilities,
statuses, telemetry, dropped_ticks }`. Culling and render preparation are measured by the
renderer as before (`last_cull_ms`, `last_group_ms`).

The overlay and the log report every one of them:

```text
SIM  0.031 MS   TICKS 1   60 HZ
  MOVE 0.004  ABIL 0.001  STAT 0.002  TELE 0.001
EXTRACT 0.012 MS   CULL 0.021 MS
```

A readout must not report the *last advance*: at 300 FPS against a 60 Hz tick, four frames
in five run no ticks and their timings are legitimately zero, so an overlay reading them
would print `SIM 0.000 MS` most of the time — which says "the simulation is free" rather
than "the simulation did not run this frame". `Simulation::last_tick_stats()` returns the
last advance that actually ticked, and the overlay shows that alongside *this* frame's tick
count. Found by reading the log of a real run, not by a test.

**Architectural decisions** — the simulation times itself with `Instant`, four samples a
tick. At 60 Hz that is 240 clock reads a second, which is below the noise floor of what it
measures; the alternative — inferring the split from the profiler's scope tree — would only
work in a build with profiling compiled in.

---

### Milestone 24 — Gameplay performance benchmarks

Status: COMPLETE

**Implemented** — `crates/engine-bench/src/game_bench.rs`, a headless benchmark over five
scenarios that plays the *same deterministic rotation* on each, so the scenes differ only in
size: movement every tick, five abilities on their cooldowns, a target cycle every 15 ticks
and a reset every 600.

**Results** — release, Ryzen 7 5800X / RX 6700 XT, 3,000 measured ticks per scenario after
600 warm-up ticks. All times are microseconds **per 60 Hz tick**.

| Scenario | Dummies | Entities | Sim mean | of which combat | Extraction | Culling | Sim p99 | Allocations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A | 1 | 3 | 0.40 µs | 0.10 µs | 0.12 µs | 0.07 µs | 0.5 µs | 0 |
| B | 10 | 12 | 0.55 µs | 0.13 µs | 0.29 µs | 0.14 µs | 0.8 µs | 2 |
| C | 100 | 102 | 2.24 µs | 0.51 µs | 2.16 µs | 0.84 µs | 5.0 µs | 9 |
| D | 500 | 502 | 9.71 µs | 2.18 µs | 10.35 µs | 3.43 µs | 24.0 µs | 8 |
| E | 1,000 | 1,002 | 19.22 µs | 4.30 µs | 20.80 µs | 5.99 µs | 49.0 µs | 8 |

Whole-frame numbers come from the game itself, because FPS and GPU time are properties of a
frame including presentation. 1920×1080 windowed, 3,000 frames, release, vsync off:

| Scenario | Dummies | CPU frame mean | CPU p99 | GPU frame mean | FPS |
| --- | --- | --- | --- | --- | --- |
| A | 1 | 0.208 ms | 0.728 ms | 0.076 ms | 4753 |
| B | 10 | 0.203 ms | 0.645 ms | 0.074 ms | 4427 |
| C | 100 | 0.214 ms | 0.666 ms | 0.079 ms | 4654 |
| D | 500 | 0.264 ms | 0.805 ms | 0.116 ms | 3769 |
| E | 1,000 | 0.340 ms | 1.156 ms | 0.150 ms | 2941 |

Against the phase budgets, at 1,000 entities:

| Goal | Frame budget | Frame | Simulation share |
| --- | --- | --- | --- |
| Minimum, 240 FPS | 4.17 ms | 0.34 ms | 0.03% |
| Target, 360 FPS | 2.78 ms | 0.34 ms | 0.04% |
| Stretch, 480 FPS | 2.08 ms | 0.34 ms | 0.06% |

A 60 Hz tick happens roughly once every five frames at 300 FPS, so the 19 µs tick costs
about 4 µs of an average frame — **0.06% of the 480 FPS stretch budget**. The gameplay
simulation is not the bottleneck and is not close to being one.

**What the measurement changed** — one real finding, described under Milestone 14: the
target ranking was O(n²) and ran every tick. Fixing it cut the p99 tick at 1,000 dummies
from 398 µs to 49 µs. That is the whole reason the benchmark exists.

---

### Milestone 25 — Allocation discipline

Status: COMPLETE

**Implemented** — every per-frame buffer is cleared, never freed: the event stream, the
command buffer, the ability hit list, the status outcome list, the target query's two
scratch arrays, the meter's target list and slot index, the extractor's slot mapping, the
overlay's `String`s and its two ranking buffers.

**Measured** — `game_bench` counts every process allocation across the measured ticks with a
counting global allocator:

| Scenario | Allocations over 3,000 ticks | Per tick |
| --- | --- | --- |
| A (1 dummy) | 0 | 0.0000 |
| B (10) | 2 | 0.0007 |
| C (100) | 9 | 0.0030 |
| D (500) | 8 | 0.0027 |
| E (1,000) | 8 | 0.0027 |

The residue is amortized growth, not per-frame work: the meter appends a row the first time
each distinct target is hit, and the target cycle reaches new dummies for tens of seconds in
the larger scenarios. Scenario A, where every target has been seen during warm-up, allocates
**exactly zero** over 3,000 ticks. Unit tests assert the same property directly — 
`a_steady_state_simulation_stops_allocating`, `a_steady_scene_stops_allocating`,
`resolving_repeatedly_reuses_the_buffer`, `a_steady_stream_of_events_does_not_reallocate`,
`rewriting_reuses_the_same_buffers`.

**Architectural decisions** — no custom allocator and no object pool. Neither is justified
by a measurement of ~0.003 allocations per tick.

---

### Milestone 26 — Full integration

Status: COMPLETE

**Implemented** — `engine-app` was rewired from the Phase 0 stress scene to the game:

```text
Application -> Input -> Commands -> Fixed Simulation -> Gameplay -> Combat
            -> Render Extraction -> RenderWorld -> Culling -> RenderList
            -> Renderer -> D3D12
```

The player is controllable and visible, jumps, casts, damages dummies, and the meter
updates. Mouse look captures on left click and releases on Escape; the wheel changes camera
distance; `--dummies <n>` sizes the group.

**Files** — `crates/engine-app/src/{app.rs,args.rs}`.

**Removed** — the `--triangles` option and the F1–F8 stress-level keys. The game binary no
longer draws the Phase 0 stress scene, so options that set its triangle count would name
nothing. The stress scene itself remains in `engine-renderer` and is still exercised by
`crates/engine-renderer/tests/stress_scene.rs` and by `engine-showcase`.

---

## Test results

`cargo test --workspace`, debug, on a machine with a D3D12 adapter:

```text
771 tests passed, 0 failed, 0 ignored
```

New in Phase 3:

| Suite | Tests | Covers |
| --- | --- | --- |
| `engine-game` unit | 126 | entities, world, movement, combat, statuses, abilities, targeting, events, telemetry, commands, bindings, arena, simulation |
| `engine-game::headless_simulation` | 6 | the whole pipeline with no renderer |
| `engine-game::determinism` | 4 | repeatability, event order, frame-rate independence, reset |
| `engine-presentation` unit | 30 | extraction, third-person camera, HUD text |
| `engine-core::time` | +6 | the fixed timestep |
| `engine-app` unit | 12 | argument parsing, aspect ratio |

Checks run, all green:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace
cargo build --workspace
cargo build --workspace --release
cargo build --release --no-default-features
```

---

## Regression: Phase 1 and Phase 2

| Behaviour | Check | Result |
| --- | --- | --- |
| Application lifecycle, input, D3D12 init, depth, camera, lighting, culling, render list, overlay | `cargo test --workspace` — every Phase 1 and Phase 2 test | pass, unchanged |
| Showcase | `showcase --frames 600 --preset 4` (10,000 objects) | 0.469 ms mean CPU, 2132 FPS — matches the Phase 2 baseline of 0.401–0.514 ms |
| Frustum culling | `cull_bench --objects 100000 --iterations 200` | 5.54 ns/object at 50% visible against the Phase 2 baseline of ~6.0 ns; 0 allocations |
| Profiling | overlay and log report every stage | pass, with four new gameplay scopes |

No rendering regression. The one change to shared code is `engine_core::time::FixedTimestep`,
which is additive.

**One pre-existing issue found and fixed in passing**: `cargo clippy --workspace
--all-targets --no-default-features` reported 11 dead-code warnings in `engine-showcase`,
whose HUD is dead by construction when the overlay feature is off. Verified against a clean
tree that these predate Phase 3. A crate-level `cfg_attr` now documents and silences them, so
the configuration matrix in `docs/testing.md` is true again.

---

## Known limitations

* **Nothing fights back.** Dummies have health and factions but no AI, no abilities and no
  attacks. The player cannot die outside a test.
* **No entity-to-entity collision.** Characters pass through each other and through dummies.
* **Flat floor only.** No slopes, stairs, obstacles or camera collision.
* **One status kind.** `Burning` is the only one; the mechanism is general, the content is
  not.
* **Boxes, not characters.** Everything is a scaled unit cube. Animation, skinning and models
  are Phase 5.
* **The corpse pose is a rotation.** A dead dummy is drawn lying down and recoloured; there
  is no death animation and no ragdoll.
* **Ability effects are single-shot.** No projectiles, no travel time, no cast bars, no
  channelling — an ability applies at the tick it is used.
* **No interpolation between simulation ticks.** `FixedTimestep::alpha` exists and is tested,
  but extraction reads the current state directly. At 60 Hz against 300+ FPS this is visible
  as slight stepping on fast movement; it is the first thing to fix if it becomes distracting.
* **Targeting is nearest/cycle only.** No mouse picking, no tab-target facing rules.
* **The meter has no window.** It reports a session total and its DPS, not a rolling 5-second
  figure.
* **Single-threaded gameplay.** The simulation runs on the main thread. At 19 µs a tick there
  is no case for anything else.

---

## Phase 4 recommendations

Do not start these yet; this is the handover, not a plan of record.

1. **Networking is the natural next phase, and the seams for it exist.** Commands are already
   the only input to the simulation, the tick is fixed and deterministic, and every state
   change is an event. The work is a transport, a command channel with tick numbers,
   server-side re-simulation, and client prediction with reconciliation — not a rewrite of
   any gameplay system. Determinism tests already assert what replication depends on.
2. **Interpolate between ticks before adding content.** Extraction should blend the previous
   and current simulation states by `FixedTimestep::alpha`. It is a small change in one
   place, it removes the only visible artefact of the fixed timestep, and it is much easier
   to do before there are many entity kinds to interpolate.
3. **Snapshot and rewind.** A `GameWorld` clone plus a command log is a replay and the basis
   of both server reconciliation and a bug-report format. The world is plain arrays, so this
   is cheap now and gets more expensive with every system added.
4. **Give the dummies behaviour only when it is needed for something.** An AI that attacks
   changes what the player's health, the camera and the meter are for; do it when the phase
   goal needs it, not because the arena looks static.
5. **Per-instance material data in the renderer** is the standing Phase 2 limitation and is
   what would let the arena's dummies show health-based colour without a render-entity
   respawn per state change. The extractor's `restyled` counter exists to say how often that
   currently happens.
6. **Do not introduce an ECS, a job-based simulation or a custom allocator** without a
   measurement that demands it. The current numbers say the simulation is 0.06% of the frame
   at 1,000 entities.

---

## Final acceptance checklist

### Architecture

* [x] Gameplay simulation does not depend on D3D12 — `cargo tree -p engine-game` is
      `engine-core` and `log`.
* [x] Gameplay can run headlessly in tests — `tests/headless_simulation.rs`.
* [x] `RenderWorld` remains presentation/render data.
* [x] Gameplay state is separate from `RenderWorld` — asserted by
      `gameplay_state_does_not_leak_into_the_render_world`.
* [x] Commands separate input from simulation — `engine-game::command`, `input_map`.
* [x] Simulation is not tied to render FPS — fixed 60 Hz, asserted at 60/240/480/1400 FPS.
* [x] Future networking remains architecturally possible — commands, ticks, events, no RNG.

### Character

* [x] Character can move. * [x] Character can jump. * [x] Gravity works.
* [x] Ground detection works. * [x] Character appears in the renderer.
* [x] Third-person camera works.

### World

* [x] Training arena exists. * [x] Single-target dummy exists.
* [x] Multi-target dummy group exists.

### Combat

* [x] Health. * [x] Damage. * [x] Damage events. * [x] Abilities. * [x] Cooldowns.
* [x] Targeting. * [x] AoE. * [x] Ability synergy. * [x] Ten ability slots.

### Telemetry

* [x] Combat events are observable. * [x] Total damage. * [x] DPS. * [x] Damage by ability.
* [x] Damage by target. * [x] Combat session can reset.

### Developer UX

* [x] Ability bar. * [x] Cooldowns visible. * [x] Dummy reset (**R**). * [x] FPS visible.
* [x] Simulation timing visible.

### Performance

* [x] Simulation benchmark — `game_bench`, five scenarios.
* [x] Combat benchmark — the combat column of the same benchmark.
* [x] No unexplained render regression — showcase and `cull_bench` match Phase 2.
* [x] Allocation behaviour measured — 0 to 0.003 allocations per tick, counted.

### Testing

* [x] All existing tests pass. * [x] New gameplay tests. * [x] Combat tests.
* [x] Ability tests. * [x] Targeting tests. * [x] Determinism tests.
* [x] Headless simulation tests. * [x] Integration tests.

---

## How to run it

```powershell
cargo run --release --bin mygame                    # the prototype
cargo run --release --bin mygame -- --dummies 100   # a bigger group
cargo run --release --bin game_bench                # the gameplay benchmark
```

| Control | Action |
| --- | --- |
| W A S D | move, relative to the camera |
| SPACE | jump |
| 1 / 2 / 3 | Fireball / Ignite / Detonate |
| 4 – 0 | placeholder abilities |
| TAB | cycle target |
| R | reset the dummies and the combat meter |
| LEFT CLICK | capture the mouse for camera control |
| ESC | release the mouse |
| MOUSE WHEEL | camera distance |
| F5 | developer overlay on/off |
| F9 | wireframe on/off |
