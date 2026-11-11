# Monster navigation, sight and charges

Working document for the pass that taught monsters about walls: finding a way round them,
not shooting through them, and charging without a wind-up. Also covers the combat-text
overlap fix that shipped with it.

Status: **COMPLETE**. See [Known limitations](#known-limitations).

## What changed for the player

| before | now |
| --- | --- |
| a monster behind a wall walked into it and stayed there | it walks round the end of the wall, through the doorway, up the ramp |
| a caster shot through walls and pillars | it only uses an ability on somebody it can see; otherwise it comes round until it can |
| stepping behind a pillar did nothing to an enemy's cast | a single-target cast whose victim goes out of sight is lost (`CastEnd::LineOfSightLost`) |
| an explosion reached through a wall | area damage only hits victims with a clear line from where it went off |
| charges had a 0.7–1.5 s cast bar | charges are instant: no cast bar, no wind-up, and only at somebody in sight |
| a burst of numbers piled on top of itself | floating combat text never covers other floating text if it can move up |

## Navigation (`engine-game/src/nav.rs`)

```text
CollisionWorld ──probe──► Navigator (lazy 2.5D grid, cached) ──A*──► corners ──► NavFollow
                                                                     (string-pulled)  (per monster)
```

* **No baked mesh.** The collision world bodies are moved through is the map. A 0.5 m cell's
  ground is found with a ray down from the height the search arrived at, and a capsule of the
  character's size lifted by a step must fit there. Answers are cached, and the cache is
  dropped whenever the collision world is handed out for editing
  (`Simulation::collision_world_mut`), so editor levels, the boss gate and test walls need no
  build step.
* **2.5D.** Cells are keyed by height band, so a path over a bridge stays on it and one under
  it stays under. Neighbours connect when the step between them is climbable — the
  controller's own `max_step_height`, or a slope within `max_slope_angle`. No diagonal cuts a
  corner.
* **Bounded.** A search expands at most 3,000 cells; one that cannot reach the goal (or runs
  out) returns the way to the nearest reachable cell, and the monster waits there instead of
  walking into the wall. A straight walkable line skips the search entirely. At most six
  searches run per tick; the rest follow the corners they have.
* **Following.** `EnemyRuntime::nav` (a `NavFollow`) is re-planned every 0.6 s (staggered per
  monster) or when the goal moves 1.5 m. Only **Chase** and **Returning** navigate; standing to
  cast and backing off are straight lines. While going round something the monster faces where
  it walks; heading straight at its victim it keeps facing them.
* **Cost.** Graveyard Crypt, release: 143 routes of 20–70 m between packs, 138 found, about
  0.5 ms per search cold and 0.45 ms warm.

## Sight

The player's line-of-sight query (`los::has_line_of_sight`, chest to chest, `WORLD` and
`LOS_BLOCKER`) now also governs monsters:

* `AiTarget::visible` — computed only for monsters in a fighting state. The brain treats an
  unseen victim as out of position (so it chases, via navigation) and never picks an ability
  aimed at somebody unseen: swings, bolts, heals and charges alike.
* A single-target cast completing on a victim out of sight ends `LineOfSightLost`.
* Single-target hits, channel ticks and heals are skipped for an unseen recipient.
* Area damage (`hit_shape`) hits a victim only with a clear line from a metre above where it
  went off. A circle already telegraphed on the floor still lands where it was placed.

**Terrain fix.** Rays against a heightfield used to be tested against its bounding box, which
made every sight line over a valley below the highest hill "blocked" and every downward ray
find the hilltop. `ray_vs_heightfield` now walks the cells under the ray and tests their
triangles.

## Charges

The seven charge abilities have `cast_time: 0.0`, and the executor starts the run on the tick
the brain chooses it (`Simulation::begin_dash`). A table invariant test keeps charges instant.

## Combat text

See [combat_ui.md](combat_ui.md): entries are placed in importance order and moved up to clear
each other; each keeps the room it was given and relaxes back down. Every entry has a unique
`serial` so this is tracked across frames.

## Tests

`engine-game/src/nav.rs` (open floor, round a wall's gap, closed room gets as close as it can,
step vs ledge, terrain with a sheer block, determinism, the follower), `tests/monster_navigation.rs`
(a ghoul walks round a wall and hits; a caster never damages through a wall and comes round;
a cast is lost to a wall that drops mid-cast; the crypt's packs can be walked between),
`engine-collision` ray tests (valley not blocked by a hill, ray down finds the surface),
`game_ui` (twelve simultaneous numbers on two neighbours never overlap over 40 frames),
`enemy/mod.rs` (charges are instant), `tests/graveyard.rs` (the warrior's charge has no cast).

## Known limitations

- **Monsters do not avoid each other while pathing.** A pack funnelling through a doorway
  queues against the collision it already has, not a crowd simulation.
- **One body size.** Paths are found for the character controller's capsule, which every
  monster moves with today; a larger monster would need its own navigator.
- **Numbers can still cover nameplates.** Only floating text is kept off floating text.
- **An idle monster notices through walls.** Aggro is by radius, as before; sight is only
  asked once it is fighting.
