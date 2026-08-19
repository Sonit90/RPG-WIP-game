# Targeted-Ability Line of Sight

Technical design. Written before the code, kept beside `collision.md` because the query it
adds is a collision query and the rule it enforces is an ability rule.

---

## What this is, and what it is not

One sentence of scope:

> When a targeted ability explicitly requires line of sight, the caster must have an
> unobstructed path to the selected target, or the ability is refused.

It is **not** a visibility or perception system. Nothing here is called by enemy AI, by
aggro, by threat, by patrol or by target acquisition, and there is no `can_see`. The only
consumer is the targeted-ability validation path in `engine-game`.

---

## Layering

```text
engine-collision      ray_vs_shape, CollisionWorld::segment_blocked (geometry, no gameplay)
        |
engine-game::los      has_line_of_sight(collision, world, a, b)  (combat points, no abilities)
        |
firemage_exec         validate_aim -> AbilityError::NoLineOfSight (which abilities, when)
        |
sim::tick_cast        final check at cast completion              (what happens on failure)
```

Each layer only knows the one below it. `engine-collision` learns nothing about abilities;
`los` learns nothing about FireMage; the executor decides the policy.

---

## The query

```rust
los::line_of_sight(collision, world, source, target) -> Option<LineOfSightCheck>
los::has_line_of_sight(collision, world, source, target) -> bool
```

`None` only when one of the two entities has no position — a target that was despawned
between one check and the next — and every caller treats that as "not visible", because a
target that is gone has already been refused a step earlier.

`LineOfSightCheck` carries the answer *and* what produced it — the two endpoints, the
blocker's `StaticId` and its label — because the debug readout must report the same query
that gameplay ran, not a second one performed for the picture.

The check is a single ray:

```text
caster combat point  o=================>  target combat point
                                    ^
                        first blocking shape within the segment, if any
```

- **Endpoints.** A body's `position` is at its **feet**. Both endpoints are therefore
  `position + Y * height * fraction`, with the fraction coming from `CombatPoint`
  (`Head` 0.9, `Chest` 0.65, `Center` 0.5). Targeted abilities use `CombatPoint::CHEST` on
  both ends; the constant is one place so every ability agrees. An entity with no recorded
  height falls back to a fixed 1.0 m offset rather than firing from the floor.
- **Length.** The ray is exactly as long as the distance between the two points. A shape
  behind the target is not between the two, so it cannot block.
- **The caster does not block.** Characters are not in the collision world at all — only
  static geometry is — so the source can never be its own blocker. The endpoint rule keeps
  the target from blocking itself for the same reason.
- **Degenerate case.** Endpoints closer than a millimetre are visible by definition: a ray
  with no length has no direction, and the alternative is a normalisation of zero.

### Filtering

`CollisionMask::LINE_OF_SIGHT` = `WORLD | LOS_BLOCKER`.

- `WORLD` is every wall, pillar, building, rock and door the level already builds collision
  for. They block movement, and blocking sight is the same statement about the same box.
- `LOS_BLOCKER` is a new layer for the thing that blocks a spell without blocking a body: a
  boss barrier, an encounter wall. `CollisionFilter::los_blocker()` collides with nothing,
  so adding one never changes where the player can walk.
- Decorative geometry has no collision shape at all — `world_collision()` returns `None` for
  it — so it is not in the world and cannot block anything. No extra rule needed.

Dynamic obstacles are the existing `add_static` / `remove_static` pair plus a broad-phase
rebuild. A door that opens removes its shape; the next LoS query finds nothing there. The
LoS system hard-codes no obstacle type.

---

## Ability configuration

`AbilityDef` gains one field:

```rust
pub requires_line_of_sight: bool,   // default in spirit: false
```

Nothing infers it. It is set to `true` for the four single-target spells that should be
stopped by a wall (Firebolt, Flame Lance, Pyroblast, Detonate) and `false` for everything
else, including every aimed, ground, self and movement ability. A cone or a line is aimed
rather than targeted, so it is out of scope by definition, and Blink is a movement.

A check is only ever run when `target == TargetMode::Single && requires_line_of_sight`. An
ability that does not ask for it never casts a ray, which is the whole of the performance
story.

---

## Validation flow

```text
use_slot / cast
   -> alive, cooldown, GCD, energy
   -> validate_aim
        Single:  NoTarget -> TargetInvalid -> TargetFriendly -> OutOfRange
                 -> FacingWrongWay -> [requires LoS] NoLineOfSight -> NotEnoughIgnite
   -> instant: land immediately
   -> cast time: ActiveCast, then at completion re-run the LoS check only
```

LoS is checked **after** range, so a target across the level behind a wall reports the
distance problem it would report today rather than a new one.

`AbilityError::NoLineOfSight` — "TARGET IS NOT IN LINE OF SIGHT" — is the existing refusal
mechanism; no second error channel is introduced.

### Cast-time abilities

At completion, `tick_cast` re-runs the LoS check against the cast's own target before the
effect lands. If it fails, the cast ends as `CastEnd::LineOfSightLost`, nothing lands, and
the cast is counted as interrupted. Because a cast-time ability pays its charge, its Heat
and its energy in `land`, a cast that never lands pays none of them, and `end_cast` gives
back the global cooldown — the same refund a cancelled cast gets, for the same reason: the
cast produced nothing.

That is deliberately *not* the same as a cast that completes into a corpse, which does land,
does spend its charge and is counted as wasted. A dead target is something the player
committed to; a wall that appeared is something they can react to.

There is no per-frame raycast during the cast. Two checks, at the start and at the end,
which is what §12 of the requirement asks for and what the existing interruption rules
(move, cancel, death) already look like.

---

## Debug

- `Simulation::last_line_of_sight()` returns the last check any targeted ability ran,
  including the ability, the target, whether LoS was required, the result and the blocker's
  label.
- `CollisionDebug` (F3) draws it: a thin bar along the ray, green when clear and red when
  blocked, a marker at each endpoint, and a highlight on the blocking shape.
- `los_debug::append_lines` formats the readout the overlay appends while F3 is on:

```text
LOS       FIREBOLT -> #3   AT CAST
RANGE     OK        REQUIRED YES
RESULT    BLOCKED   DIST 20.0 M
BLOCKER   DUNGEON WALL
```

Developer tooling only: it lives behind the application's `debug-overlay` feature and never
appears on the combat HUD.

---

## Explicitly out of scope

Enemy AI LoS, perception, aggro, threat, stealth, repositioning, projectile flight, AoE and
ground-target LoS, navigation, per-frame polling, multi-point visibility, LoS caching.

---

## Measured cost

`cargo run --release --bin collision_bench`, a 35 m ray — the longest range on the bar —
fired through the dense part of each scenario:

| Static objects | Broad phase | One LoS ray |
| --- | --- | --- |
| 10 | linear | 0.04 us |
| 100 | grid | 0.31 us |
| 1,000 | grid | 1.27 us |
| 10,000 | grid | 0.29 us |

Per **ray**, not per tick: one on activation, one more when a cast lands, and none in
between. At two casts a second that is under 3 us a second against a 4,167 us frame budget.

`cargo run --release --bin dungeon_bench -- --frames 600` — the real dungeon, 23 enemies,
the player casting the whole bar — is unchanged within noise at 2.4 us mean simulation time
and a 29.5 us p99 frame.

---

## Known limitations

- **One ray, one point.** A target whose chest is behind a pillar but whose head and legs
  are not is out of line of sight. Multi-point visibility is explicitly out of scope.
- **A caster inside geometry sees nothing.** A ray that starts inside a solid shape hits it
  at distance zero. That is the correct answer for a caster stuck in a wall, and it means a
  target clipping into a pillar cannot be hit.
- **Every single-target ability currently opts in.** The `requires_line_of_sight = false`
  path is exercised by unit tests over `AbilityDef` rather than by a shipped ability, because
  the class has no targeted spell that should reach through a wall yet.
- **Enemies do not check.** Enemy abilities have their own execution path and were left
  alone, per the scope restriction. An enemy can still attack through a wall.
- **The floor is a blocker.** The ground plane is on the `WORLD` layer, so a caster below a
  target's feet firing steeply upward through the floor plane would be blocked. It cannot
  happen with both combat points above the plane, which is every case gameplay produces
  today.
