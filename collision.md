# Collision & Character Movement Phase

Living document. Updated after every milestone.

---

## Milestone status

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Audit existing movement | DONE |
| 2 | Collision foundation | DONE |
| 3 | Static collision world | DONE |
| 4 | Broad-phase abstraction | DONE |
| 5 | Character controller | DONE |
| 6 | Horizontal collision | DONE |
| 7 | Sliding | DONE |
| 8 | Gravity and ground detection | DONE |
| 9 | Jumping | DONE |
| 10 | Step handling | DONE |
| 11 | Slopes | DONE |
| 12 | Continuous movement / tunneling | DONE |
| 13 | Collision debug visualization | DONE |
| 14 | Integrate with current prototype | DONE |
| 15 | Collision tests | DONE |
| 16 | Collision stress benchmark | DONE |
| 17 | Regression | DONE |
| 18 | Manual verification | DONE |
| + | Collision demo scene | DONE |

---

## Milestone 1 — Audit of the existing movement

Read before a line of production code was changed. Everything below is what the code
*does* today, not what it was supposed to do.

### Current movement flow

```text
Win32 message  ->  engine_platform::WindowEvent
                        |
                        v
              engine_core::input::InputState          (crates/engine-app/src/app.rs:481)
                        |
   Bindings::write_commands(input, camera_yaw, &mut out)   (engine-game/src/input_map.rs:75)
                        |  W A S D -> PlayerCommand::Move { forward, right, yaw }
                        |  SPACE   -> PlayerCommand::Jump
                        v
        Simulation::push_commands                     (engine-game/src/sim.rs:389)
                        |
                        |  Move is *held*, not queued: intent.direction is overwritten
                        |  (sim.rs:379). Jump is queued in a CommandBuffer.
                        v
        Simulation::advance(real_delta)               (sim.rs:399)
                        |
                        v
        FixedTimestep::accumulate -> N whole 60 Hz ticks
                        |
                        v
        Simulation::tick_once                         (sim.rs:482)
              1. world.begin_tick()      copy bodies -> previous_bodies (interpolation)
              2. apply_pending_commands  Jump sets intent.jump = true
              3. integrate_bodies(step)  <-- THE ONLY PLACE POSITION IS UPDATED
              4. abilities / cast / retarget
              5. statuses
              6. telemetry
                        |
                        v
        Simulation::integrate_bodies                  (sim.rs:561)
              for every slot: movement::step(body, intent-or-NONE, &config, step)
              then intent.jump = false
                        |
                        v
        movement::step                                (engine-game/src/movement.rs:201)
              accelerate_horizontally(...)      max_speed 7, accel 60, decel 50,
                                                air_control 0.35
              if intent.jump && body.grounded { velocity.y = jump_speed (8.0);
                                                grounded = false }
              if !grounded { velocity.y -= gravity(25.0) * dt, clamped to
                             -terminal_velocity(60.0) }
              body.position += body.velocity * dt      <-- integration
              resolve_ground(body, config)             y <= ground_height + 1e-3
                                                       -> y = ground_height, vy = 0,
                                                          grounded = true
              confine_to_arena(body, config)           independent X/Z clamp to
                                                       +/- arena_half_extent (40)
              face_movement(body)                      yaw from velocity
              body.state = classify(body)              Idle/Running/Jumping/Falling
                        |
                        v
        RenderExtractor::extract                      (engine-presentation/src/extract.rs:269)
              body.position (feet) + height*0.5  ->  Transform.translation
              interpolated between previous_body and body by timestep alpha
                        |
                        v
        RenderWorld -> Culler -> RenderList -> D3D12
```

### Exactly where player position is updated

One place, and only one:

* `crates/engine-game/src/movement.rs:219` — `body.position += body.velocity * seconds;`

Followed by two correction passes in the same function:

* `resolve_ground` (`movement.rs:268`) — the entire existing "collision": a scalar
  `position.y <= ground_height + 1e-3` clamp.
* `confine_to_arena` (`movement.rs:281`) — an independent X and Z clamp to a square
  half-extent. This is the *only* thing stopping the player leaving the arena, and it is
  exactly the "clamp X/Z independently" that Milestone 6 forbids.

Nothing else in the workspace writes `Body::position` during gameplay. The other writers
are all deliberate teleports:

* `GameWorld::spawn` / `GameWorld::restore` (`world.rs:167`, `world.rs:393`) — spawn and
  the `R` dummy reset.
* tests.

### What exists today

| Concern | Where | State |
| --- | --- | --- |
| Fixed timestep | `engine-core/src/time.rs`, `FixedTimestep`, default 60 Hz | Solid. Gameplay is already frame-rate independent, and `alpha()` drives render interpolation. |
| Player transform | `movement::Body { position, velocity, grounded, facing_yaw, state }`, stored in `GameWorld::bodies` (SoA) | Position is the **feet**, not the centre. Documented at `movement.rs:13`. |
| Gravity | `MovementConfig::gravity = 25.0`, terminal velocity 60 | Applied only when `!grounded`. |
| Jump | `jump_speed = 8.0`, allowed only when `grounded` | Already gated correctly. Consumed once per tick (`sim.rs:582`). |
| Ground detection | `position.y <= ground_height + GROUND_EPSILON` | Scalar. No normals, no slopes, no surfaces. Exactly what Milestone 8 says to replace. |
| Collision vs. geometry | none | The player walks through every dummy and every object. There is no world geometry at all. |
| Camera | `engine-presentation::camera::ThirdPersonCamera`, follows `interpolated_player_position` | Presentation-only. Does not collide and is out of scope for this phase. |
| Target selection | `engine-game/src/targeting.rs`, `TargetQuery::nearest_hostile` / `next_hostile` | Horizontal distance, edge-corrected by radius. Independent of collision. |
| Training dummies | `engine-game/src/arena.rs`, deterministic grid | Have `radius` and `height` used by area targeting and by extraction only. Never block movement. |
| Render extraction | `engine-presentation/src/extract.rs` | Reads bodies, writes `RenderWorld`. One-way. |
| Existing sim tests | `engine-game/tests/{determinism,headless_simulation}.rs`, plus unit tests in each module | 12 movement unit tests already cover jump/fall/land/air-control/arena-clamp; they must keep passing. |

### Dependency direction as it stands

```text
engine-core   (math, time, input, jobs — no windows, no D3D12)
    ^                        ^
    |                        |
engine-game            engine-render-data
(simulation only)            ^
    ^                        |
    |                   engine-renderer (D3D12)
engine-presentation ---------^
    ^
engine-app (the only place all three meet)
```

`engine-game` depends on `engine-core` and `log`, and nothing else — asserted by
`crates/engine-game/tests/headless_simulation.rs` and stated in `Cargo.toml`. The
collision system must not disturb this.

### Consequences for the design

1. **One integration point.** Replacing `movement::step`'s integrate-then-clamp with a
   collision-aware move touches exactly one function, which is why this phase is
   tractable.
2. **Feet-origin positions** make a capsule easy: the capsule's lower cap centre is
   `position + (0, radius, 0)`.
3. **The arena clamp must stay for the combat scene.** It is what makes the 1,000-dummy
   benchmark scenario reproducible. It becomes belt-and-braces behind real collision
   rather than the only barrier.
4. **Collision needs its own crate.** `engine-game` already carries combat, abilities and
   telemetry; a collision system inside it could not be tested or benchmarked without
   dragging the simulation along. A new `engine-collision` crate that depends only on
   `engine-core` keeps the seam and gives the dedicated test suite Milestone 15 asks for
   a home outside production gameplay code.
5. **Dummies stay non-blocking** (see the decision record in Milestone 14 below).

No production code was modified during this milestone.

---

## Milestones 2–12 — the `engine-collision` crate

All eleven landed together in one new crate, because they are one system: a shape
vocabulary nobody queries is untestable, a world with no controller has no behaviour to
assert, and sliding, ground detection, steps and slopes are four properties of the same
sixty lines of movement resolution.

```text
crates/engine-collision/
  src/layer.rs        CollisionLayer, CollisionMask, CollisionFilter      (filtering)
  src/shape.rs        Capsule, Obb, Segment, Shape, Aabb/Plane re-use     (M2)
  src/contact.rs      Contact, StaticContact, QueryStats                  (M2)
  src/narrow.rs       capsule vs plane / aabb / obb                       (M2)
  src/broad.rs        BroadPhase::Linear | Grid(UniformGrid), Proxy       (M4)
  src/world.rs        CollisionWorld, StaticId, StaticDesc, QueryBuffers  (M3)
  src/controller.rs   CharacterConfig/State, move_character               (M5-M12)
```

### Architecture

```text
                gameplay (engine-game)
                        |  displacement for this tick
                        v
        controller::move_character
                        |
      +-----------------+------------------+
      |                 |                  |
   horizontal        vertical          ground probe
   (+ step up)      (land/ceiling)     (stay on slopes)
      |                 |                  |
      +--------> world.overlap_capsule <---+
                        |
              broad::BroadPhase::query      -> candidate indices
                        |
              narrow::capsule_vs_shape      -> Option<Contact>
                        |
                   StaticContact
```

`engine-collision` depends on `engine-core` and nothing else — no `windows`, no D3D12, no
renderer, no render data, no UI. `cargo tree -p engine-collision` is two nodes deep.

### Collision shapes (M2)

| Shape | Used for | Test |
| --- | --- | --- |
| `Capsule` | the character, only | — (it is always the moving side) |
| `Plane` (from `engine-core`) | the floor | deepest cap's signed distance vs. radius |
| `Aabb` (from `engine-core`) | walls, steps, platforms | closest point between spine and box |
| `Obb` | ramps | the same test, in the box's own space |

`engine_core::math::Aabb` and `Plane` are reused rather than redefined; a second `Aabb` in
the workspace would be a conversion function waiting to be written wrongly.

The capsule is stored as **a segment plus a radius**, not as a position plus a height,
because that is the form every test wants. It is always upright: the controller never tilts
it. `Capsule::upright(feet, radius, height)` clamps `height` up to `2 * radius` so a
degenerate input becomes a sphere rather than a capsule with an inverted spine — which
would turn every contact normal inside out.

**The collision shape is independent of the visual mesh.** `CharacterConfig::radius` and
`height` are collision numbers. Nothing in the crate can name a mesh.

### Collision filtering (M2)

```text
A collides with B  <=>  A.mask contains B.layer  AND  B.mask contains A.layer
```

Three layers today — `PLAYER`, `WORLD`, `DUMMY` — as single bits in a `u32`, with a
`CollisionMask` as the set of layers something cares about. Both directions are required,
so a one-sided mask cannot produce "the player is blocked by something that is not blocked
by the player". The crate never learns what a player or a dummy *is*; `CollisionFilter`
carries the two numbers and `collides_with` is two ands.

### Static collision world (M3)

`CollisionWorld` holds parallel arrays of shapes, filters, labels, generations and
broad-phase proxies. `StaticId` is generational, so a handle to a removed object cannot
address whatever is added into its slot.

* `add_static(StaticDesc)`, plus `add_box`, `add_block`, `add_ramp`, `add_ground`,
  `add_plane` for readable level code.
* `remove_static(id)` — frees the slot; a stale handle is reported, never a panic.
* `overlap_capsule(&capsule, mask, &mut buffers) -> QueryStats` — the only query the
  controller uses.

The API never exposes how objects are stored. Gameplay writes `world.add_block(...)` and
reads `StaticContact`; the arrays, the free list and the grid are private.

`QueryBuffers` is the caller's scratch space, because a query needs somewhere to put
candidates and contacts, and allocating that per query would be dozens of allocations a
second for one character.

### Broad and narrow phase (M4)

```text
BroadPhase::Linear             scan every proxy's AABB.     N < 64
BroadPhase::Grid(UniformGrid)  bin proxies over X and Z.    N >= 64
```

Static dispatch on an enum, not a trait object: this is the inner loop of character
movement and a v-table call per query would cost more than the query.

The grid is compressed-sparse-row — one offsets array and one items array, not a `Vec` per
cell — and two-dimensional, because level geometry is a floor plan and a Y axis would
multiply the cell count while rejecting almost nothing X and Z had not. Objects with no
finite bounds (planes) and objects spanning more than 64 cells go in an always-candidate
list. Candidates are sorted and deduplicated, so an object straddling a cell boundary is
offered once — otherwise its contact would be resolved twice and push the character out
twice as far.

`BroadPhase::build` chooses; nothing above it knows which was chosen. The world's broad
phase is rebuilt explicitly (`rebuild_broad_phase`) after a scene is built, and a world
that has been edited since answers with a linear scan in the meantime — **forgetting to
rebuild costs speed, never an answer**, which is asserted by
`a_dirty_world_answers_the_same_as_a_rebuilt_one`.

### Character controller and movement resolution (M5-M7)

```text
move_character(world, config, &mut state, displacement, &mut buffers) -> MoveResult
```

State is `position` (the **feet**), `velocity`, `grounded`, `ground_normal`, `ground_id`.
Tuning is `CharacterConfig`. The controller owns neither — it is a function over them,
which is what makes it testable without a game and replayable with one. It holds no
rendering state and cannot name any.

One tick, in order:

1. **Depenetrate** at the starting position. Beginning a step inside a wall and then moving
   would resolve the move against geometry the character should never have been inside.
2. **Horizontal** — `(dx, 0, dz)`, swept, then step-up if it was blocked.
3. **Vertical** — `(0, dy, 0)`, swept. Landing and ceilings.
4. **Ground probe** — if it is no longer grounded, was grounded, and is not moving upwards.

Resolution itself, per substep:

```text
repeat up to max_resolution_iterations (6):
    contacts = world.overlap_capsule(capsule at position)
    deepest  = the contact with the most penetration      (ties: slot order)
    if none: done
    position += normal * (penetration + skin_width)
    velocity -= normal * min(0, velocity . normal)        <- this is sliding
```

**Sliding is not a special case.** Removing the velocity component that points into a
surface leaves a vector along it, and pushing out along the normal leaves the position on
it. A diagonal run at a wall therefore slides, a run straight at it stops, and a corner is
two normals applied in successive iterations. Nothing clamps X or Z independently anywhere
in the crate.

The **deepest** contact is resolved first and the query repeated, rather than applying every
contact at once: pushing out of two surfaces simultaneously double-counts the corner
between them and ejects the character out of it.

### Ground detection (M8)

A contact is ground when `normal · up >= cos(max_slope_angle)`. Never a position test —
`position.y == 0` is not a fact about a world with ramps in it.

* `max_slope_angle` is configurable, 50° by default.
* The **most level** walkable contact wins, so standing where a ramp meets the floor
  reports the floor rather than whichever was tested first.
* A contact with `normal · up < -0.001` is a ceiling; anything else unwalkable is a wall.
  Both are reported in `MoveResult`.
* **Ground snapping**: after the move, a character that was grounded, is no longer, and is
  not moving upwards probes down by `ground_snap_distance` (0.3 m). Without it, walking
  down a ramp is a sequence of small falls and the grounded flag flickers every tick.
  Skipped when `velocity.y > 0`, so a jump is always allowed to leave.

### Jumping (M9)

Deliberately **not** in this crate. A jump is a gameplay decision — how fast, from what,
after what — and the controller's part is only to answer "are you standing on something
walkable". `CharacterState::grounded` is that answer; `engine-game` sets `velocity.y` and
keeps `jump_speed` in `MovementConfig`, where it already was.

### Step handling (M10)

Attempted only when the horizontal move was blocked *along the direction asked for* — a
slide is not blocked, it made progress. Three traces:

```text
1. lift by max_step_height + skin, and abort if the raised capsule overlaps anything
   (a low ceiling must fail before anything moves horizontally)
2. move horizontally again at the raised height; abort if it made no progress
3. descend until something stops it, up to the lift distance
accept if it landed and rose by no more than max_step_height
```

The descent is **not** an ordinary sweep. A capsule put down against a step meets the
step's *edge*, and depenetrating an edge pushes the character backwards — undoing the step
it just took. So the descent advances in substeps and stops at the first overlap, bisecting
that substep six times to find the resting height (a third of a millimetre). An edge is not
a walkable surface, so a character resting on one would otherwise be reported airborne
while demonstrably supported; after an accepted step the controller takes whatever is
immediately below the feet as its ground. One tick later it has advanced far enough to be
over the face proper.

This is the difference between a capsule controller and the AABB controllers the technique
comes from, and getting it wrong is what made a step of exactly `max_step_height` stutter
against its own edge forever.

### Slope handling (M11)

`max_slope_angle` is one number and it does two things: it decides what counts as ground,
and therefore it decides what a wall is. A 45° ramp is walkable at the default 50°; a 70°
face is a wall, is reported as `hit_wall`, and cannot be climbed. Ramps are `Obb`s, and a
tilted slab's top-face normal is `rotation * +Y`, which is what the demo scene's labels are
derived from.

### Tunnelling prevention (M12)

Conservative advancement. Movement is split into substeps of at most half the capsule
radius (0.2 m by default) and every substep resolves before the next begins, so a wall the
capsule has passed is still within a radius of its spine and is therefore still a contact.
A 200 m/s character against a 5 cm barrier and a 120 m/s fall onto a 2 cm floor are both
asserted. Substeps are bounded at 256 so an absurd displacement is a wrong position rather
than a hung frame.

Not an exact swept test. The trade is recorded under *Known limitations*.

### Performance properties

* **No allocation** in the steady state: `QueryBuffers` is caller-owned and reused, and
  `moving_costs_no_allocation_once_the_buffers_are_warm` asserts the capacity never moves
  over 2,000 ticks of movement against geometry.
* **No dynamic dispatch**: no trait objects anywhere in the crate.
* **No heap graph**: the world is parallel arrays and the grid is two `Vec<u32>`.
* Every loop has a fixed bound: 6 resolution iterations, 12 closest-point iterations, 6
  bisection steps, 256 substeps.

### Tests

90 unit tests in the crate, all passing.

```text
layer       6   filtering, symmetry, bit hygiene
shape      13   capsule construction, degenerate inputs, bounds, ramps, local/world
contact     5   levelness, slope angle, NaN guards, statistics
narrow     15   plane/aabb/obb, corners, deep penetration, boundaries, path agreement
broad      10   scan vs. grid agreement, masks, duplicates, empty worlds, choice by size
world      15   handles, generations, filtering, dirty rebuilds, allocation, determinism
controller 26   floor, wall, slide, corner, step (0 / small / limit / too high), slopes
                (0/15/30/45/70), gravity, landing, ledges, ceilings, tunnelling (x2),
                spawn-inside, zero-move, determinism, allocation
```

### Architectural decisions recorded here

1. **A separate crate, not a module of `engine-game`.** Collision inside the gameplay crate
   could not be benchmarked or tested without dragging combat, abilities and telemetry
   along, and Milestone 15 asks for a suite outside production gameplay code.
2. **An enum of shapes, not a trait.** Three variants matched in the hottest loop.
3. **Caller-owned query buffers.** The only way to promise no per-frame allocation without
   making the world `&mut` for a read.
4. **A grid that is built, not maintained.** The geometry is static; incremental insertion
   would be machinery for a case that does not exist.
5. **Jump stays in gameplay.** See M9.

### Known limitations at this point

* One moving shape: an upright capsule. No box or sphere controller.
* Static geometry only. Nothing moves, so there are no moving platforms.
* Conservative advancement rather than exact sweeps (M12).
* The deep-penetration exit is a per-axis minimum translation, which is exact for a box and
  approximate for a rotated one.
* A character spawned fully inside solid geometry that reaches the floor has no free
  position to be pushed to; `resolve_overlaps` is not a search.

---

## Milestone 13 — Collision debug visualization

`crates/engine-presentation/src/collision_debug.rs`. **F3** toggles it.

```text
CollisionWorld (simulation)          RenderWorld (presentation)
-------------------------            --------------------------
Aabb, Obb                    --->    one box per shape
the character's capsule      --->    a cylinder and two caps
contacts from a live query   --->    a marker and an arrow per contact
grounded / airborne          --->    the capsule's colour (green / amber)
walkable / not walkable      --->    the contact's colour (green / red)
```

Two properties matter more than what it looks like:

1. **It draws the collision world, not a copy of it.** Every box comes from the shape the
   simulation is actually testing against. A debug view built from a second description of
   the level lies exactly when it is needed.
2. **It costs nothing when it is off.** `set_enabled(false)` *despawns* the render entities
   rather than hiding them — an invisible entity still occupies a culler slot and is still
   walked every frame. Off, the render world is exactly what it was before the view existed.

It is developer-only by construction: `engine-app` builds it behind the `debug-overlay`
feature, so `cargo build --no-default-features` cannot reach the module at all. That build
is compiled as part of the regression run.

Contacts are queried with a capsule inflated by two skin widths. Resolution leaves the
character a skin width clear of every surface, so a character standing perfectly still
touches nothing — and a debug view that draws no contact under the feet of a character
visibly standing on the floor is one nobody will trust. The inflation is view-only and
feeds nothing back into movement.

Entities are pooled: the pool grows to the scene's size and then stops, which
`a_steady_scene_stops_spawning_render_entities` asserts over 300 frames.

## Milestone 14 — Integration with the prototype

### What changed in gameplay

| Where | Change |
| --- | --- |
| `movement.rs` | `step_collided` added beside `step`. Same acceleration, same jump, same gravity; the integrate-and-clamp is replaced by `engine_collision::move_character`. |
| `movement.rs` | `Body` gained `ground_normal`, and `Body::slope_angle()`. |
| `sim.rs` | `Simulation` owns a `CollisionWorld`, its `QueryBuffers` and the last `MoveResult`. `SimConfig` gained a `CharacterConfig`. |
| `sim.rs` | The player is moved with `step_collided`; every other body still uses `step`. |
| `sim.rs` | `PlayerCommand::Teleport { position }` — a developer control, applied in command order like everything else. |
| `arena.rs` | The training arena builds a floor plane and four boundary walls. |
| `world.rs` | `GameWorld::reset_history`, so a teleport does not slide across the level over one frame. |
| `input_map.rs` | `write_movement_commands`: movement and jump only, for the demo scene. |

The dependency direction is unchanged. `engine-game` now depends on `engine-core` **and
`engine-collision`**, which itself depends on `engine-core` alone — so the rule the crate
docs state ("no `windows`, no renderer, no render data") still holds, and
`headless_simulation.rs` still passes.

### Everything the prototype could do, it still does

Asserted by `crates/engine-game/tests/collision_gameplay.rs` and by the existing suites:
WASD movement, jumping, target selection and cycling, casting, area abilities, cooldowns,
the damage meter, dummy reset, the game UI, nameplates and damage numbers.

### Decision: training dummies are non-blocking

**Dummies do not block player movement.** `CollisionFilter::dummy()` has an empty mask, and
the simulation moves only the player through the collision world.

Why:

* A dummy is a *target*, and a target the player can get wedged against is a target that
  ruins the fight it exists for. The prototype's whole loop is standing in front of a dummy
  and pressing 1, 2, 3.
* Blocking dummies would need a dynamic broad phase — 1,000 moving proxies rebuilt every
  tick — for a scenario that exists to benchmark *combat*.
* The brief prefers it, and nothing in the existing gameplay requires otherwise.

If a future encounter needs a solid enemy, the layer already exists: give the dummy
`CollisionFilter::new(DUMMY, WORLD)`, give the player a mask containing `DUMMY`, and add its
shape to the collision world. No other code changes.

### Behaviour changes a player would notice

1. **The arena boundary is geometry now.** Running into it at an angle slides along it
   instead of stopping dead on one axis. The old clamp is still there behind it, which is
   what keeps the 1,000-dummy benchmark scenario inside the arena it was measured in.
2. **A resting character sits `skin_width` (5 mm) above the floor** rather than exactly on
   it. That is what stops a character alternating between touching a surface and being one
   ulp inside it, which is a visible shudder. Two existing tests were updated to a tolerance
   and say why.

## Collision demo scene

`crates/engine-game/src/demo.rs`. **Completely separate from the combat arena** — no
dummies, no abilities used, no damage, no telemetry, no combat HUD.

### How to launch it

```text
cargo run --release --bin mygame -- --scene collision
cargo run --release --bin mygame -- --scene collision --overlay      # readout visible
cargo run --release --bin mygame -- --scene collision --width 1280 --height 720
```

`--scene combat` (the default) is the training arena, unchanged.

### The course

```text
  x=0        25        50        75       105       135       165
 WALL      CORNER    STEPS     SLOPES     DROP    HIGH SPEED  COMBINED
 SLIDE
  |          |         |         |          |         |          |
 ===========================================================================  platform
                                                                             (top at y=0)
                           .............................  void floor at y = -15
```

| Key | Station | Geometry | Tests |
| --- | --- | --- | --- |
| 1 | WALL / SLIDE | a 1 m x 20 m wall | collision, sliding |
| 2 | CORNER | two perpendicular walls, plus a 1.6 m corridor | corner handling, narrow gaps |
| 3 | STEPS | blocks of 0.15, 0.30, 0.45 (the limit) and 0.90 m, separated by floor | step-up, rejection |
| 4 | SLOPES | ramps at 15, 30, 45 and 70 degrees | slope walking, steep rejection |
| 5 | DROP | a 4 m ledge, and the platform edge at z = 15 | leaving ground, falling, landing |
| 6 | HIGH SPEED | a 5 cm slab 52 m below the spawn, and a 5 cm wall | tunnelling prevention |
| 7 | COMBINED | a ramp into a corner with a step across the approach | all of it at once |

The demo course brings its own floor — a slab whose top face is at `y = 0` — so the
extractor's visual floor plane is dropped to the catch floor at `y = -15` in this scene.
Two coplanar surfaces at the same height are a depth-buffer tie, and a tie reads as the
whole floor flickering. Down there it doubles as the thing a long fall lands on.

Every piece of geometry is labelled in world space (`SLOPE 45`, `STEP 0.45M MAX`,
`0.90M TOO HIGH`, `THIN WALL 5CM`, …) through the same projection the enemy nameplates use.

### Controls in the demo scene

```text
W A S D     move            1 - 7   stand at a test station
SPACE       jump            R       back to station 1
F3          collision debug drawing on/off
F5          the collision readout on/off
F9          wireframe on/off
```

The digits are the ability bar in the combat scene and the test stations here, because the
demo scene issues movement and jump only (`Bindings::write_movement_commands`).

### The developer readout

```text
COLLISION DEMO - STATION 3 STEPS
WALK +X OVER 0.15, 0.30, 0.45, THEN STOP AT 0.90

POS         50.00     0.01     0.00
VEL          4.00    -0.02     0.00   SPEED  4.00
GROUNDED YES   STATE RUNNING
NORMAL       0.00     1.00     0.00   SLOPE   0.0 DEG
SURFACE  STEP 0.30M

CAPSULE  RADIUS 0.40   HEIGHT 1.80
LIMITS   STEP 0.45   SLOPE 50.0 DEG

BROAD    LINEAR   CANDIDATES 6
NARROW   TESTS 6   CONTACTS 2
MOVE     SUBSTEPS 2   ITERATIONS 4   STEPPED 0.30
WORLD    STATICS 31   DRAWN 34   WALL NO    CEILING NO

1-7 TEST STATIONS   R RESET   F3 COLLISION DRAW   F5 THIS READOUT
```

It goes through the renderer's existing developer-overlay text channel — the same one
`hud::write_lines` uses — because the project already has one debug text path and a second
would be a second thing to keep working. It **replaces** the combat readout in this scene
and never appears in the combat one.

### Reproducibility

The layout is a list of constants, so two runs build the same course to the bit
(`the_scene_is_built_identically_every_time`). Each station has a spawn point and pressing
its number teleports there **at rest**, with the interpolation history reset so the
character arrives rather than sliding. No station spawns inside geometry — asserted, because
a spawn inside a wall is a character that pops sideways on its first tick.

## Milestone 15 — Collision tests

All outside production gameplay code.

| Suite | Where | Tests |
| --- | --- | --- |
| unit | `engine-collision/src/**` | 90 |
| geometry | `engine-collision/tests/geometry.rs` | 18 |
| movement | `engine-collision/tests/movement.rs` | 27 |
| determinism | `engine-collision/tests/determinism.rs` | 5 |
| gameplay integration | `engine-game/tests/collision_gameplay.rs` | 14 |
| debug view | `engine-presentation/src/collision_debug.rs` | 6 |
| readout | `engine-presentation/src/collision_hud.rs` | 7 |
| demo scene | `engine-game/src/demo.rs` | 9 |

**Geometry** — AABB/AABB including shared faces, edges and corners; capsule/plane including
tilted planes and ceilings; capsule/AABB faces, edges, deep penetration and monotonic
penetration; capsule/OBB agreeing with the axis-aligned path; degenerate inputs (empty box,
zero-radius capsule, flat box); closest-point answers checked against a dense sample.

**Movement** — floor, walking, wall stop, diagonal slide (and that a slide keeps >90% of the
distance), inside corner, narrow corridor, ledge, long fall, jump height, jump refused in
mid-air, jump against a wall, jump over an obstacle too tall to step onto, steps at 0 /
0.15 / 0.30 / 0.45 / 0.75 / 0.9 / 2.5 m, a flight of five steps, slopes at 0 / 15 / 30 / 45
/ 70 degrees, walking downhill without bouncing, tunnelling at 300 m/s and at terminal
velocity, a fast diagonal at a thin corner, allocation, and bounded work.

**Determinism** — the same 1,200-tick input script run three times must produce *identical*
state after every tick, not similar state. Plus: a deliberately perturbed run must differ
(so the comparison can actually fail), a world built twice answers identically, the linear
scan and the grid produce the same contacts, and swapping the query buffers mid-run changes
nothing (so the controller has no hidden state).

## Milestone 16 — Collision stress benchmark

`cargo run --release --bin collision_bench`. Release mode, no device, no window.

A character walks a 7.2 m circuit past four pillars while the level around it grows from 10
to 10,000 static objects. The circuit is identical in every scenario, so the only thing that
changes is how much geometry the broad phase has to *reject* — which is what makes the rows
comparable.

Measured on the target machine (Ryzen 5800X), 6,000 ticks per scenario plus 600 warm-up:

```text
 statics |  phase |    broad |   narrow | controller |  cands |  tests | allocs
      10 | LINEAR |    0.016 |    0.021 |      0.189 |    1.3 |    1.3 |      0
     100 |   GRID |    0.031 |    0.023 |      0.259 |    1.3 |    1.3 |      0
    1000 |   GRID |    0.031 |    0.024 |      0.270 |    1.3 |    1.3 |      0
   10000 |   GRID |    0.029 |    0.023 |      0.283 |    1.3 |    1.3 |      0
```

Microseconds per tick. `broad` is `BroadPhase::query` alone; `narrow` is the rest of a full
`overlap_capsule`; `controller` is a whole `move_character` — sweeps, resolution, step
probes and all — which runs several queries a tick and is therefore not the sum of the two.

What the numbers say:

* **A thousandfold more geometry costs 50% more.** 0.189 us at 10 objects, 0.283 us at
  10,000. The broad phase is flat because the grid rejects by cell, not by count.
* **Candidate and test counts do not grow at all** — 1.3 per tick at every size. That is the
  broad phase doing its job, and it is counted rather than asserted.
* **Zero allocations** in all four scenarios over 6,000 measured ticks.
* Against a 240 FPS budget of 4,166 us per frame, character collision at 10,000 static
  objects is **0.007%** of it. At the 480 FPS aspiration it is 0.014%.
* The linear scan is genuinely faster below the threshold — 0.016 us against the grid's
  0.031 — which is why `GRID_THRESHOLD` exists rather than always binning.

Gameplay's own benchmark is unchanged by collision: `game_bench` at 1,000 dummies reports a
20.98 us simulation tick with 0.0025 allocations per tick.

## Milestone 17 — Regression

Run in this order, all clean:

```text
cargo fmt --all -- --check                              OK
cargo clippy --workspace --all-targets -- -D warnings   OK (no warnings)
cargo test --workspace                                  1108 passed, 0 failed
cargo build --workspace                                 OK
cargo build --workspace --release                       OK
cargo build --release -p engine-app --no-default-features   OK (shipping build)
cargo run --release --bin collision_bench               OK (table above)
cargo run --release --bin game_bench                    OK (no regression)
```

1,108 tests against 1,044 before the phase: 64 net new, and every pre-existing test still
passes. Three were amended, each with the reason written beside it:

| Test | Why |
| --- | --- |
| `headless_simulation::movement_and_jumping_run_headlessly` | `position.y == 0.0` became a 2 cm tolerance: a resting character sits a skin width above the floor. |
| `extract::the_player_is_drawn_standing_on_the_floor…` | Same, one layer up. |
| `extract::a_still_character_is_drawn_in_exactly_one_place` | "Still" is now still to within an ulp: the resting push lands on one of two adjacent `f32` values, twenty nanometres apart. |

## Milestone 18 — Manual verification

Both scenes launched from the release build on the target machine (RX 6700 XT, D3D12,
feature level 12_1).

```text
mygame --scene collision --frames 400 --overlay --width 1280 --height 720
  -> 400 frames in 0.112 s, 3583 FPS average, CPU p99 0.998 ms, GPU mean 0.068 ms
  -> clean shutdown

mygame --frames 900 --dummies 4 --width 1280 --height 720
  -> 900 frames in 0.232 s, 3872 FPS average, CPU p99 1.530 ms, GPU mean 0.062 ms
  -> clean shutdown
```

Everything below is covered by an automated test as well as being manually reachable; the
tests are named in brackets so a regression has a name.

### Movement

* WASD works — both scenes.
* The player cannot walk through walls (`walking_straight_into_a_wall_stops_at_it`,
  `the_player_is_stopped_by_the_arena_wall_rather_than_clamped_at_it`).
* Sliding along walls (`walking_diagonally_into_a_wall_slides_along_it`,
  `running_diagonally_at_an_arena_wall_slides_along_it`).
* Cannot fall through the floor (`a_long_fall_lands_rather_than_passing_through_the_world`).
* Falls from ledges (`walking_off_the_demo_scenes_ledge_falls_and_lands`).
* Jumps, and lands (`a_jump_leaves_the_ground_reaches_its_height_and_comes_back`).
* Walks over small steps, blocked by high ones
  (`the_demo_scenes_wall_stops_the_character_and_its_steps_do_not`).
* Slopes behave: 15/30/45 walkable, 70 not
  (`the_demo_scenes_slopes_are_walkable_up_to_the_limit_and_not_past_it`).

### Combat

Target selection, cycling, spells, area damage, cooldowns and telemetry all still work
(`combat_still_works_with_collision_in_the_way`, plus the whole pre-existing suite).

### Rendering

* The drawn player position is the collision position, interpolated between ticks — the
  extraction path is unchanged and its tests still pass.
* The camera follows the interpolated position (unchanged).
* No jitter at high frame rates: 3,583 FPS with a 0.998 ms CPU p99 in the collision scene.
* No simulation/render disagreement: the debug view draws the *same* `CollisionWorld` the
  simulation queries.

### Debugging

F3 turns the collision drawing on and off; F5 turns the readout on and off. Both were
exercised in the automated tests as well
(`turning_it_off_gives_the_render_world_back`).

---

## Acceptance criteria

### Collision

* [x] Collision module exists — `crates/engine-collision`.
* [x] Collision does not depend on the renderer — `engine-core` and nothing else.
* [x] Static collision world exists — `CollisionWorld`.
* [x] AABB — `engine_core::math::Aabb`, reused.
* [x] Capsule — `shape::Capsule`.
* [x] Plane — `engine_core::math::Plane`, reused.
* [x] Collision filtering — `CollisionLayer` / `CollisionMask` / `CollisionFilter`.
* [x] Broad-phase abstraction — `BroadPhase::Linear | Grid`, chosen by size.
* [x] Narrow-phase collision — `narrow::capsule_vs_shape`.
* [x] Contact normals — `Contact { point, normal, penetration }`.
* [x] Character controller — `controller::move_character`.

### Movement

* [x] Player collision against walls.
* [x] Sliding.
* [x] Corner handling.
* [x] Gravity.
* [x] Ground detection — from normals, against a configurable slope threshold.
* [x] Jumping — gated on `grounded`, tuned in gameplay.
* [x] Falling.
* [x] Step-up — `max_step_height`, with a surface probe.
* [x] Slope handling — `max_slope_angle`.
* [x] High-speed tunnelling prevention — conservative advancement.

### Architecture

* [x] Collision is simulation-side.
* [x] Rendering does not participate in collision decisions — the debug view only reads.
* [x] The player's visual mesh is independent of its collision shape.
* [x] The character controller is deterministic — asserted three ways.
* [x] The fixed timestep is respected — collision runs inside `tick_once`.
* [x] No per-frame allocations in normal character movement — counted, zero.

### Tests

* [x] Geometry, character controller, movement, ground, jump, step, slope, high-speed,
      determinism and regression tests. 1,108 workspace tests, 0 failures.

### Performance

* [x] Benchmark exists, at 10 / 100 / 1,000 / 10,000 objects, with CPU time and allocations
      measured. No unexplained regression: `game_bench` is unchanged.

---

## Known limitations

Honest list of what this is not.

1. **One moving shape.** An upright capsule, and only the player has one. No box or sphere
   controller, and no second character.
2. **Static geometry only.** Nothing in the collision world moves, so there are no moving
   platforms, no doors and no lifts. The world would need a dynamic broad phase for that.
3. **Conservative advancement, not exact sweeps.** Tunnelling is prevented by substepping at
   half the capsule radius, which is correct for any speed but does more work than a true
   swept test at very high speeds, and resolves a hair inside the surface rather than
   exactly at the moment of contact.
4. **Dummies do not collide.** By decision (see Milestone 14), not by omission.
5. **A step is climbed over several ticks.** A capsule against a 0.45 m step rides its edge
   for a few ticks rather than arriving on top instantly, and overshoots the top by a few
   centimetres on the way. Visible as a small hop at low frame rates; correct, and it settles.
6. **Narrow steps are refused.** The step probe is placed a little more than one radius
   ahead, so a step shallower than about half a metre in the direction of travel reports the
   surface beyond it instead. Real stair treads are deeper than that.
7. **Deep penetration on a rotated box is approximate.** The exit direction is the smallest
   axis translation in the box's own space, which is exact for an AABB and an approximation
   for an OBB. Only reached when something is already badly overlapping.
8. **No crouching, no ceilings-aware step-down, no ledge grab, no wall-running.**
9. **A character spawned fully inside solid geometry that reaches the floor stays there.**
   `resolve_overlaps` pushes out of the nearest face; it is not a search for free space.
10. **The collision demo scene reuses the full `Simulation`.** Combat, abilities and
    telemetry are present and inert rather than absent. That is deliberate — a second kind
    of simulation would be a scene-management system, which the brief rules out — but it
    does mean the scene is not literally the minimum set of systems.
11. **The debug view is a developer tool.** Boxes and spheres, no wireframe outlines, and
    no way to click a shape and inspect it.

---

## Future improvements

Recommended, not implemented.

* **A BVH or a tighter grid** if levels reach six figures of static objects, or if geometry
  stops being a floor plan and starts being stacked. The `BroadPhase` enum is where it goes,
  and nothing above it changes.
* **Triangle meshes**, so level geometry can come from an artist rather than from boxes and
  ramps. A `Shape::TriangleMesh` variant plus a per-mesh BVH; the narrow phase gains a case
  and everything else is untouched.
* **A dynamic broad phase** for moving obstacles — doors, platforms, and solid characters.
  This is the change that unlocks the "dummies block movement" decision being reversed.
* **True swept tests** to replace conservative advancement, if a projectile or a dash ever
  needs an exact time of impact rather than a position that did not pass through anything.
* **Crouching**, which is a second capsule height plus a headroom check before standing.
* **Multiple characters** through the collision world, which the API already allows: the
  controller is a free function over caller-owned state, so N characters are N states and N
  buffer sets — or one buffer set reused, since the moves are sequential.
