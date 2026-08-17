# Warden of the Ashen Gate

## Encounter Fantasy

Something enormous has been standing in front of this gate since the reliquary burned, and it
has not moved because nothing has made it. It fights like the building: slow, wide, and
inevitable. It does not chase well and it does not need to — it makes the room smaller until
there is nowhere the player would rather be.

The encounter has three beats. It opens legible: one cone, one circle, one filler, all at a
pace the player can read. At two thirds health it starts leaving fire on the floor, and the
room the player has been circling stops being available. At one third it begins detonating
itself, and the fight becomes about being somewhere else on a timer.

It is not a Brute with more health. It has five abilities, three phases, two mechanics that
require different answers, and one that requires the player to run further than they have run
in the whole dungeon.

## Arena

**The Ashen Gate.** A square hall 48 m across, centred at (0, −266), at the far end of the
Reliquary Vault corridor.

| | |
|---|---|
| Shape | Square, 24 m from the centre to each wall |
| Terrain | Flat. No steps, no slopes — every mechanic in this fight is a footprint on the floor, and vertical variation would make a ring ambiguous. |
| Hazards | None built in. Every hazard in the fight is one the boss creates. |
| Movement constraints | The room's own walls, built from the same eight-metre segments as the rest of the dungeon, with one 16 m doorway on the entrance side. |
| Environmental objects | Four pillars on a 13 m radius at the diagonals, four corner pillars, and the arch in the doorway. |
| Boss position | Centre, (0, −266), facing +Z towards the entrance. |

The four inner pillars matter: Immolation Nova reaches 14 m, and the pillars stand at 13 m, so
a player who runs *to* a pillar is still inside the nova. Running past them is the correct
answer. The hall is 24 m from centre to wall specifically so that the escape is possible from
the middle with room to spare and impossible if the player starts moving late.

## Base Stats

| | |
|---|---|
| Health | 6,000 |
| Movement speed | 4.8 m/s (5.2 in phase 2, 5.5 in phase 3) |
| Radius / height | 1.1 m / 3.0 m |
| Attack range | 30 m (Ember Lance) down to 3.0 m (Swing) |
| Basic attack damage | 14 (Swing) |
| Attack speed | one Swing per 1.8 s (1.53 s phase 2, 1.26 s phase 3) |
| Aggro range | 28 m |
| Leash range | 70 m |
| Preferred range | 3.0 m (± 0.8 m dead band) |
| Alert time | 0.8 s |

It is slower than the player in every phase. That is deliberate: nothing in this fight kills
a player who is moving, and everything in it kills a player who is not.

## Phases

```text
   Phase 1                Phase 2                  Phase 3
  (100 – 66 %)  ──────▶  (66 – 33 %)  ──────▶     (33 – 0 %)
   "The Gate"            "The Ash"                "The Burning"
      │                      │                        │
  cone + circle        + lingering pools      + self-centred nova
```

| Phase | Trigger | Abilities (priority order) | Speed | Cooldowns |
|---|---|---|---|---|
| 1 | Encounter start | Warden's Sweep, Ashfall, Ember Lance, Swing | ×1.00 | ×1.00 |
| 2 | Health ≤ 66 % | Cinder Rain, Warden's Sweep, Ashfall, Ember Lance, Swing | ×1.08 | ×0.85 |
| 3 | Health ≤ 33 % | Immolation Nova, Cinder Rain, Warden's Sweep, Ashfall, Swing | ×1.15 | ×0.70 |

**Transitions are deterministic and one-way.** They are health thresholds, checked every
tick. Healing the boss above a threshold does not send it back a phase. Each transition:

1. cancels the cast in flight and takes its telegraph down,
2. clears the boss's own recovery timer,
3. staggers the boss for 1.5 s during which it does nothing,
4. emits one phase-change event.

Ember Lance drops out of the priority list in phase 3 — by then the boss has three things it
would rather be doing, and a filler at the bottom of a five-entry list never fires anyway.

## Abilities

### WARDEN'S SWEEP

| | |
|---|---|
| Purpose | The melee-range punish. The reason standing in front of it is a choice. |
| Cooldown | 8.0 s (6.8 s phase 2, 5.6 s phase 3) |
| Cast time | 1.7 s |
| Damage | 42 physical |
| Target selection | Aimed along the boss's facing when the cast starts |
| Area | Cone, 11 m long, 60° wide |
| Telegraph | Cone on the floor for the full 1.7 s |
| Interruptible | **No** |
| Counterplay | Walk out of the wedge. It is fixed when the cast starts, and 11 m of cone is 1.6 s of running at the edge — sideways is much cheaper than backwards. |
| Phase availability | 1, 2, 3 |
| Recovery | 0.9 s |

### ASHFALL

| | |
|---|---|
| Purpose | The interrupt. The one thing in the fight the player's interrupt can answer. |
| Cooldown | 10.0 s (8.5 s phase 2, 7.0 s phase 3) |
| Cast time | 2.2 s |
| Damage | 55 fire |
| Target selection | Centred on its victim's position when the cast starts |
| Area | Circle, 6 m radius |
| Telegraph | Ring on the floor for the full 2.2 s |
| Interruptible | **Yes** |
| Counterplay | Interrupt it, or leave the ring. Interrupting costs the boss 2.0 s of recovery on top. |
| Phase availability | 1, 2, 3 |
| Recovery | 1.0 s |

### EMBER LANCE

| | |
|---|---|
| Purpose | Reaches a player who has run away. Stops kiting from being free. |
| Cooldown | 3.5 s (2.98 s phase 2) |
| Cast time | 1.0 s |
| Damage | 20 fire |
| Target selection | Its current victim |
| Area | Single target, 30 m |
| Telegraph | Cast bar only |
| Interruptible | Yes |
| Counterplay | Accept it, interrupt it, or be outside 30 m — which in a 48 m hall means the far wall. |
| Phase availability | 1, 2 |
| Recovery | 0.3 s |

### CINDER RAIN

| | |
|---|---|
| Purpose | Takes the room away, one 4.5 m circle at a time. |
| Cooldown | 11.0 s (9.35 s phase 2 value, 7.7 s phase 3) |
| Cast time | 2.0 s |
| Damage | 10 fire on impact, then **16 fire per second** for 10 s |
| Target selection | Centred on its victim's position when the cast starts |
| Area | Circle, 4.5 m radius, both impact and pool |
| Telegraph | Ring for the 2.0 s cast; the pool is drawn for its full 10 s |
| Interruptible | **No** |
| Counterplay | Move out before it lands, and treat the pool as wall for ten seconds. |
| Phase availability | 2, 3 |
| Recovery | 0.8 s |

### IMMOLATION NOVA

| | |
|---|---|
| Purpose | The climax mechanic. The only thing in the dungeon that requires a full sprint. |
| Cooldown | 15.0 s (10.5 s at phase 3 scaling) |
| Cast time | 3.2 s |
| Damage | 120 fire |
| Target selection | Centred on **the boss itself** |
| Area | Circle, 14 m radius |
| Telegraph | Ring around the boss, filling for the full 3.2 s |
| Interruptible | **No** |
| Counterplay | Be more than 14 m from the boss when it resolves. From melee range that is 14 m of running in 3.2 s — trivially possible at 7 m/s, and impossible if the player spends the first second finishing a cast. |
| Phase availability | 3 |
| Recovery | 1.6 s |

## Mechanics

Each mechanic is documented independently because each has a different answer.

### Mechanic 1 — The Sweep (phases 1–3)

A wide, uninterruptible cone from a boss that is standing still. It exists to make the
default melee position wrong. The correct habit is to be at the boss's flank, not its front,
and the Sweep is what teaches it. Because it is aimed when the cast starts, a player already
moving sideways is never hit.

### Mechanic 2 — The Interrupt Window (phases 1–3)

Ashfall is the only interruptible ability the boss has. It is the fight's rhythm: roughly
once every 8–10 seconds the player gets one meaningful decision, and the decision is whether
to spend the interrupt or spend the movement. Both are correct; spending neither is 55
damage.

### Mechanic 3 — Ground Denial (phases 2–3)

Cinder Rain pools last 10 s on an 11 s cooldown, so from phase 2 onward there is almost
always exactly one pool on the floor, and in phase 3 usually two. The arena is 48 m across
and a pool is 9 m across, so the room never becomes unplayable — it becomes *directed*. The
pools are what stop the player from fighting the whole encounter standing on one spot.

### Mechanic 4 — The Nova (phase 3)

A 14 m circle centred on the boss with a 3.2 s wind-up. It is the only mechanic that requires
the player to fully disengage, and it is the reason the hall is 24 m from centre to wall. Combined
with a pool on the floor it becomes a real routing problem: run out of the nova, but not into
the fire.

### Mechanic 5 — The Stagger

Every phase transition stops the boss for 1.5 s. It is not a damage window in the reward
sense — there is no loot in this phase and no enrage timer — it is a *readability* mechanic.
The fight visibly pauses and restarts as something else.

## Player Counterplay

### Warden's Sweep

- **Notice:** a cone drawn on the floor pointing where the boss is facing.
- **Do:** move sideways out of the wedge within 1.7 s.
- **Fail:** 42 physical damage.

### Ashfall

- **Notice:** a 6 m ring under the player's own feet, and an interruptible cast bar.
- **Do:** interrupt, or step 6 m in any direction within 2.2 s.
- **Fail:** 55 fire damage.

### Ember Lance

- **Notice:** a cast bar with no floor telegraph.
- **Do:** nothing, or interrupt it if the interrupt is otherwise idle.
- **Fail:** 20 fire damage. Intended to be affordable.

### Cinder Rain

- **Notice:** a 4.5 m ring under the player, then a burning pool where it landed.
- **Do:** leave the ring during the cast; do not walk back through the pool for 10 s.
- **Fail:** 10 on impact and 16 a second for as long as the player stands in it.

### Immolation Nova

- **Notice:** a ring 14 m across centred on the boss and a 3.2 s cast bar.
- **Do:** run. Start immediately; stop casting.
- **Fail:** 120 fire damage — a quarter of the player's health, and the mechanic most likely
  to be the last thing that happens in a failed attempt.

### Phase transition

- **Notice:** the boss stops, its current cast disappears, and a phase-change readout updates.
- **Do:** use the 1.5 s to reposition, not to cast — phase 2 opens with Cinder Rain and phase
  3 opens with the Nova.
- **Fail:** nothing directly. Failing to reposition means the next mechanic starts with the
  player in the wrong place.

## Failure Conditions

| Condition | What happens |
|---|---|
| Player death | The player's health reaches zero. Combat stops for them; the boss loses its target, cancels any cast, and leashes home. |
| Wipe | The player is dead and no other hostile-of-the-boss target remains. The encounter is considered failed and can be reset. |
| Encounter reset | Requested by the player (the reset control) or automatically when the boss leashes. Restores the boss fully — see below. |
| Phase reset | There is no partial phase reset. Phases only move forward within an attempt, and a full encounter reset returns the boss to phase 1. |
| Dungeon reset | Restores every pack in the dungeon *and* the boss, including phase, cooldowns, threat, casts, telegraphs and hazards. |

### What a boss reset restores

1. Health to 6,000.
2. Phase to 1.
3. Position to the arena centre, facing +Z.
4. AI state to Idle, threat table empty, target none.
5. Every ability cooldown to zero.
6. The cast in flight cancelled, and its telegraph event ended.
7. Every Cinder Rain pool the boss created removed, with its area-effect-ended event emitted.
8. Encounter state back to `NotStarted`, so the boss can be pulled again immediately.

No stale state may survive: no telegraph on the floor, no pool, no cast bar, no nameplate
showing a phase.

## VFX

Every mechanic must be legible from the floor without reading the interface.

| Element | Visual requirement |
|---|---|
| Warden's Sweep | Cone telegraph on the floor, exactly the 11 m × 60° gameplay wedge, filling over 1.7 s. A heavy impact along the wedge on resolution. |
| Ashfall | 6 m floor ring, filling over 2.2 s. Fire burst on resolution; nothing at all when interrupted. |
| Ember Lance | Projectile from the boss to the player, and a small impact. No floor marking. |
| Cinder Rain | 4.5 m ring during the cast; a persistent burning-ground effect at exactly the same radius for the pool's whole 10 s, removed the instant it expires. |
| Immolation Nova | 14 m ring centred on the boss, filling over 3.2 s — the largest telegraph in the game, and it must read as such. A full-arena burst on resolution. |
| Phase transition | A one-off burst at the boss, and a change in its persistent effect so the three phases look different from across the room. |
| Death | The generic enemy death effect, plus removal of every hazard it owns. |

**No sound requirements.** No cue is authored for any of the above in this phase; existing
generic combat audio continues to play unchanged.

## Testing

- Starts in phase 1 with the phase-1 ability list, and Cinder Rain and the Nova unavailable.
- Crossing 66 % advances to phase 2 exactly once and emits exactly one phase-change event.
- Crossing 33 % advances to phase 3 exactly once.
- Healing the boss above a threshold does not revert its phase.
- Damage that crosses two thresholds in one hit still ends in the correct phase.
- Each transition cancels the in-flight cast and ends its telegraph.
- The stagger lasts 1.5 s, during which the boss uses no ability.
- Warden's Sweep and Immolation Nova cannot be interrupted; Ashfall can.
- Every telegraph's drawn shape equals the shape that deals the damage.
- Immolation Nova hits at 13.9 m and misses at 14.1 m.
- Cinder Rain pools tick at a fixed cadence, expire at exactly 10 s, and are removed on
  death, on reset, and on dungeon reset.
- A full reset restores all eight items listed under *What a boss reset restores*.
- The boss can be killed, and its death completes the encounter exactly once.
