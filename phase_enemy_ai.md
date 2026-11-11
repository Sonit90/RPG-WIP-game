# Phase 6 — Enemy AI & Encounter Foundation

Living document. Every milestone appends to it as it completes: what was implemented, what
was tested, what was measured, what was decided and what was left undone.

Target: something that fights back. A small, **scalable** foundation — one melee enemy, one
caster, threat, telegraphs, an encounter that resets — built so that elites, bosses, packs
and sophisticated behaviour are later *content and rules*, not a rewrite.

```text
                       Encounter
                           │
                    ┌──────┴──────┐
                    ↓             ↓
                 Player         Enemy
                                  │
                              AI State
                                  │
                       ┌──────────┼──────────┐
                       ↓          ↓          ↓
                    Threat     Target     Ability
                       │          │          │
                       └──────────┼──────────┘
                                  ↓
                              Gameplay
                                  │
                           Gameplay Events
                                  │
                  ┌───────────────┼───────────────┐
                  ↓               ↓               ↓
              Animation          VFX            Audio
                                  │
                                  ↓
                                  UI
```

---

## Milestone status

| # | Milestone | Status |
| --- | --- | --- |
| 1 | Engine audit | **complete** |
| 2 | Enemy data model | **complete** |
| 3 | Health & damage model | **complete** |
| 4 | Player damage & death | **complete** |
| 5 | Enemy state machine | **complete** |
| 6 | Threat / aggro | **complete** |
| 7 | Aggro / leash | **complete** |
| 8 | Enemy movement | **complete** |
| 9 | First melee enemy | **complete** |
| 10 | Enemy attack model | **complete** |
| 11 | Telegraph system | **complete** |
| 12 | First telegraphed attack | **complete** |
| 13 | Interruptible cast foundation | **complete** |
| 14 | Enemy targeting rules | **complete** |
| 15 | Enemy death | **complete** |
| 16 | Encounter controller | **complete** |
| 17 | First real encounter | **complete** |
| 18 | Multi-enemy encounter | **complete** |
| 19 | Enemy combat UI | **complete** |
| 20 | Encounter demo scene | **complete** |
| 21 | AI debug tools | **complete** |
| 22 | Determinism | **complete** |
| 23 | Automated tests | **complete** |
| 24 | AI stress test | **complete** |
| 25 | Combat stress test | **complete** |
| 26 | Full regression | **complete** |
| 27 | Final manual playtest | **partly** — see below |

---

## Milestone 1 — Audit of the existing engine

Read before writing. Nothing below is a plan; it is what the engine already does, and the
rest of the phase is written against it.

### What exists and is reused unchanged

| Thing | Where | How this phase uses it |
| --- | --- | --- |
| `EntityId` (index + generation) | `engine-game/src/entity.rs` | enemy identity, threat rows, targets — a stale id is *detectably* stale, which threat tables need |
| `GameWorld` (structure of arrays) | `engine-game/src/world.rs` | enemies are entities in the same arrays: body, health, statuses, radius, height, spawn position |
| `Health` (current/max, invariants) | `engine-game/src/combat.rs` | **one** health type for player and enemy |
| `Faction::is_hostile_to` | `engine-game/src/combat.rs` | the only place "who may attack whom" is decided |
| `Body` + `movement::step_collided` | `engine-game/src/movement.rs` | enemies move with the character controller the player uses |
| `TargetQuery` | `engine-game/src/targeting.rs` | hostile lists by distance, already allocation-free |
| `CombatEvent` / `EventStream` | `engine-game/src/event.rs` | every enemy action is announced on the stream five observers already read |
| `Simulation::deal_damage` | `engine-game/src/sim/mod.rs` | **the single authoritative damage path** — it already takes `source` and `target` and cares about neither's kind |
| `FixedTimestep` | `engine-core/src/time.rs` | AI ticks on the simulation's fixed step, which is what makes it deterministic |
| `VfxDirector` / `AudioDirector` | `engine-presentation` | telegraphs, casts, impacts and deaths are *observations*; no AI code touches a particle or a voice |
| `GameUi` / `UiSnapshot` | `engine-presentation` | enemy health, cast bar and the player's own health already have a home |
| `arena::training_arena` / `ArenaConfig` | `engine-game/src/arena.rs` | the encounter scene is built the same way every other scene is |
| Scenes (`--scene …`) | `engine-app/src/args.rs` | `encounter` joins combat, collision, character, environment, vfx, audio |
| Benchmarks (`*_bench`) | `engine-bench` | `ai_bench` joins them; headless, no device |

### What the simulation's tick looks like today

```text
tick_once:
   world.begin_tick()            record previous bodies for interpolation
   apply_pending_commands()      the player's discrete commands
   integrate_bodies(step)        every body: movement::step; the player: step_collided
   tick_firemage(step)           cooldowns, Heat, Ignite, zones
   tick_cast(step)               the player's cast, if any
   retarget_if_needed()
   tick_statuses(step)           burns, and the damage they deal
   flush_observations()          telemetry reads the tick's events
```

**Everything above is single-actor.** `Player` holds `target`, `cast` and `queued`; `tick_cast`
reads `self.player.cast`; `integrate_bodies` gives every non-player body `MoveIntent::NONE`.
Nothing is *wrong* with that — it is what one-actor gameplay needs — but it means the phase's
work is not "make dummies smart"; it is **generalising three single-actor mechanisms** (an
intent, a cast, a target) to a second kind of actor without disturbing the first.

### Where the audit changed the plan

Four findings, each of which removed work:

1. **The event stream already carries telegraph-shaped events.** `ImpactScheduled { position,
   radius, delay }` and `ImpactLanded` were built for Meteor, and the VFX director already
   draws a closing ring for the first and an explosion for the second — with audio already
   playing a whistle at the impact point. A circular enemy telegraph is therefore *free* in
   presentation. Only the cone needed a new shape, so the new events carry a
   `TelegraphShape` rather than a radius.
2. **`deal_damage` is already actor-agnostic.** It takes `source` and `target` and asks the
   world for health. Player damage needed **no new damage path** — only a caller.
3. **`AbilityId` is a dense index and telemetry looks it up with `.get()`**, so enemy
   abilities can take indices above the player's bar without any bounds problem.
4. **The combat meter counts every `Damage` event.** With enemies that would silently mean
   "the player's DPS includes what the player was hit for". This is a *pre-existing* latent
   bug that only an enemy could reveal, and it is fixed here (Milestone 3) by teaching the
   meter who its owner is.

### What does *not* exist and must be built

* No enemy actor: `EntityKind` is `Player | Dummy`, and a dummy is deliberately inert.
* No AI of any kind, no state machine, no threat, no aggro, no leash.
* No enemy movement intent: non-player bodies are integrated with no intent at all.
* No non-player cast: casting is a field on `Player`.
* No player damage source: nothing in the game can damage the player, and nothing checks
  whether the player is alive before letting them act.
* No encounter abstraction: `arena::populate` spawns dummies and that is all.
* No telegraphs as a gameplay concept (Meteor's ring is an event, not a system).

### The dependency rules this phase must not break

* `engine-game` depends on `engine-core` **only**. AI lives inside `engine-game`; it can name
  an `EntityId` and a `Vec3` and nothing about a mesh, a particle, a voice or a widget.
* Presentation observes. `AudioDirector` and `VfxDirector` gain telegraph handling by reading
  new *events*; neither is called by AI.
* One damage path, one health type, one event stream, one settings file, one camera.

### Conclusion

The foundation to build is: an **actor model** (enemy = entity + brain), a **brain**
(state machine + threat + cooldowns + cast), an **ability table** (data, not code), a
**telegraph** (gameplay shape announced as an event and resolved by the same shape), and an
**encounter** (membership, lifecycle, reset). Everything else is reuse.

---

## Milestone 2 — The enemy data model

An enemy is **an entity that happens to have a mind**. It is spawned into the same
`GameWorld` as everything else, with the same body, health, statuses, radius, height and
spawn position; what it additionally gets is a row in the simulation's `enemies` array — an
`EnemyRuntime { brain, intent, telegraph }`, indexed by entity slot, exactly like every other
per-entity store in this crate.

| Piece | Where | Note |
| --- | --- | --- |
| `EntityKind::Enemy` | `world.rs` | joins `Player` and `Dummy`; nothing branches on it in gameplay |
| `enemy_desc(kind, position)` | `world.rs` | health, radius and height come from the archetype table |
| `EnemyKind` / `EnemyArchetype` / `ARCHETYPES` | `enemy/mod.rs` | two rows today: `BRUTE` and `EMBER ADEPT` |
| `EnemyBrain` | `enemy/brain.rs` | state, timers, cooldowns, threat, cast |
| `EnemyRuntime` | `sim/enemy_exec.rs` | brain + this tick's movement intent + its telegraph |

**Dummies are untouched.** They are still `EntityKind::Dummy`, still have no brain, still
stand there. The combat scene is unchanged, and its tests pass unmodified.

**Nothing renderer-shaped entered gameplay.** `engine-game` still depends on `engine-core`
alone; the enemy's *visual* is presentation's decision, and it currently reuses the dummy
model with a red-shifted tint (`extract.rs`), which costs no upload, no binding and no draw
call.

### Decided

* **No ECS.** The world is still structure-of-arrays with the same component set per entity,
  and an enemy adds one optional array. The measurement that would change this is thousands
  of entities with divergent components; `ai_bench` reaches a hundred.
* **The archetype's ability list is the decision policy.** The brain casts the first ability
  that is off cooldown and in range, in list order. A boss with a rotation is a longer list.

---

## Milestone 3 — Health and damage

There was already exactly one damage path, and it needed no change to serve enemies:

```text
anything ─▶ Simulation::deal_damage(source, target, amount, ability, type, critical)
                 │
                 ├─ world.damage(target)      Health enforces its own invariants
                 ├─ threat.add(source)        NEW: the one place threat is generated
                 ├─ CombatEvent::Damage       what every observer reads
                 └─ on a kill: statuses cleared, brain stopped, CombatEvent::Death
```

`Health` is unchanged: current, max, never negative, never above max, never `NaN`. The
player and every enemy use the same type and the same function — **there is no
`deal_damage_to_player`**, and there could not be one without deleting the only one there is.

### The bug an enemy revealed

`CombatMeter` counted *every* `Damage` event. With nothing able to hit the player that was
correct by accident; with enemies it would have reported damage taken as damage dealt. The
meter now has an owner (set when the arena names the player entity) and counts:

* `total_damage` — damage whose **source** is the owner.
* `total_damage_taken` — damage whose **target** is the owner. New, and what the HUD's death
  and damage-taken readouts use.

A meter with no owner behaves exactly as it did before, so every existing test and the
headless simulator are unaffected.

### Tested

`the_meter_counts_what_the_player_dealt_and_not_what_it_took` stands a player in front of a
brute for ten seconds without casting: damage taken is positive, damage dealt is zero.

---

## Milestone 4 — Player damage and death

Nothing new was needed to *hurt* the player — an enemy calls the same `deal_damage`. What was
needed was what happens next:

* **Death** is `Health` reaching zero, announced by the existing `CombatEvent::Death`.
* **Casting is refused**: `try_cast` already returned `AbilityError::CasterDead`, which was
  unreachable before this phase and is now the ordinary path.
* **A running cast is dropped**: `tick_cast` already ended casts on death.
* **Steering stops**: `integrate_bodies` replaces a dead player's intent with `NONE`. The
  body still falls and still slides to a stop under the same controller — a corpse is a body,
  not a frozen object.
* **Feedback** reuses everything: the death sound (`SoundId::Death`) and the hit flash come
  from the same events they always did, and the HUD prints `YOU DIED` plus the reset hint
  from `snapshot.player.alive`.
* **Reset**: `Simulation::reset_encounter` restores the player and every enemy, and
  BACKSPACE — the key that already reset the dummies — calls it in any scene that has
  enemies. **No respawn system was built**; this is the development control the milestone
  asked for.

### Tested

`a_dead_player_cannot_act_and_a_reset_puts_everything_back`.

### Known limitation

No death animation, no ragdoll, no corpse timer for the player: the character keeps its last
pose. Enemy corpses stay in the world with the dead tint, as dummies do.

---

## Milestone 5 — The state machine

```text
Idle ─▶ Alert ─▶ Chase ⇄ Attack ─▶ Cast ─▶ Recover ─▶ (Chase)
  ▲                                  └── interrupted ──┘
  └──────────────── Returning ◀── no target, or past the leash
any state ─▶ Dead
```

Eight flat states rather than `Combat` with a sub-state: `AiState::is_combat()` is the
grouping the brief asked for, as a predicate, and four flat states are easier to assert on
than a nested one. Each has a single small function (`think_idle`, `think_alert`,
`think_engaged`, `think_cast`, `think_recover`, `think_returning`) and every transition in
the diagram has a test.

**It is deterministic by construction**: no randomness anywhere, timers driven by the fixed
timestep, ability choice by list order.

**It is testable alone**: `think(&AiPercept, step) -> AiDecision` needs no world. A test
supplies "a target 10 m away" and asserts the enemy chases.

Deliberately *not* built: a behaviour tree, a blackboard, a planner, a scripting hook.

---

## Milestone 6 — Threat

`ThreatTable`: up to eight rows of `(EntityId, f32)`, held per enemy.

* **Damage generates threat equal to the damage dealt**, added in `deal_damage` — the one
  place damage lands. No ability can deal damage without generating threat, and none can
  generate threat without dealing damage.
* **The target is the highest-threat row that is still valid.** Validity — alive, hostile,
  known to the world — is decided by a closure the *simulation* supplies, which is why the
  table itself has no dependency on the world and is tested with a closure.
* **Ties keep the current target**, so a fight does not flip between two equal attackers.
* **The table is bounded**: a ninth attacker replaces the least threatening row, and only if
  it outranks it.
* **A leash reset clears it.** An enemy that went home is not still angry.

**"The enemy attacks the player" is nowhere in the code.** The one place a fallback exists is
`AiPercept::victim()`: the angriest attacker, or — for an enemy pulled by proximity, which
has no threat towards anybody yet — the nearest hostile. Both are entity ids resolved from
factions.

Deliberately not built: threat decay, per-ability threat multipliers, taunt, threat transfer,
tables shared across a pack.

---

## Milestone 7 — Aggro and leash

| Rule | Where | Brute | Adept |
| --- | --- | ---: | ---: |
| Aggro radius | archetype | 14 m | 20 m |
| Alert time | archetype | 0.35 s | 0.5 s |
| Leash radius | archetype | 40 m | 45 m |

* **Pull by proximity**: an idle enemy notices the nearest hostile inside its aggro radius.
* **Pull by damage**: threat pulls it from any distance, which is what makes a ranged pull
  work — asserted by `damage_generates_threat_and_threat_picks_the_target`, which shoots an
  enemy standing 25 m away, outside its aggro radius.
* **Alert** is a real state with a real pause, so a pull reads as a reaction.
* **Leash** is checked once per tick for every state except `Returning` and `Dead`, so an
  enemy displaced past it by *anything* walks home.
* **Returning** walks — it does not teleport — and on arrival forgets its threat, its target
  and its combat timer.

---

## Milestone 8 — Movement

Enemies move with **the character controller the player uses**: the brain's `MoveIntent` is
parked on the runtime, and `integrate_bodies` feeds it to `movement::step_collided` with the
same config, the same collision world and the same query buffers.

That gives acceleration, friction, gravity, ground contact, walls, steps and slopes for free,
and it means an enemy cannot walk through the arena boundary. Bodies with no intent take the
cheaper `movement::step` — which is what dummies have always taken.

**AI never writes a position.** There is no teleport path for an enemy anywhere in the code.

Approach and stop is a dead band: an enemy closes while the gap exceeds
`preferred_range + range_slack` and stops inside it, so it neither walks through its target
nor twitches at the boundary.

Deliberately not built: navmesh, path-finding, obstacle avoidance, separation between
enemies (they can overlap), strafing, retreat. Documented as deferred; the intent-based
interface is what a path-follower would produce, so replacing "walk straight at it" with
"walk along a path" changes one function.

---

## Milestone 9 — The first melee enemy

**BRUTE**: 320 health, 5.2 m/s (the player runs 7, so it is kiteable), 14 m aggro, 40 m
leash, stops at 2.4 m. Abilities in priority order: `CLEAVE`, then `SWING`.

It acquires, approaches, stops, swings, recovers, swings again; it takes damage, generates
threat, dies correctly, and it reuses the dummy visual and the existing impact and hit sounds
through the event stream. **No renderer, VFX or audio code knows it exists.**

---

## Milestone 10 — The attack model

```rust
EnemyAbilityDef {
    name, range, cast_time, cooldown, damage, damage_type,
    shape,          // the hit test AND the telegraph
    aim,            // Target or Caster
    interruptible,
    recovery,       // the gap it leaves afterwards
}
```

| Attack | Range | Cast | Cooldown | Damage | Shape | Interruptible |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| `SWING` | 3 m | — | 1.8 s | 14 | single | no |
| `CLEAVE` | 5 m | 1.4 s | 9 s | 26 | 6 m cone, 60° | yes |
| `GROUND BLAST` | 25 m | 2.0 s | 7 s | 34 | 4 m circle | yes |
| `BOLT` | 25 m | 1.2 s | 2.4 s | 12 | single | yes |

**No timing lives in the state machine.** `think_engaged` reads `cooldown`, `range`,
`cast_time` and `recovery` off the table; the flow is
`AI asks ─▶ AiRequest ─▶ the simulation validates and executes ─▶ damage ─▶ events`, which
is the same relationship the player's commands have with the ability system.

Range is re-checked *at the moment an instant attack lands*, not when it was chosen: a player
who stepped out between the decision and the swing is out of range.

---

## Milestones 11 and 12 — Telegraphs

```text
enemy starts a cast
   │  shape resolved ONCE (a cone's yaw is filled in here)
   ├─▶ CombatEvent::CastStarted    ─▶ cast bar, gathering effect, gathering sound
   ├─▶ CombatEvent::TelegraphStarted { id, anchor, shape, delay }
   │        └─▶ VFX: a ring for a circle, a wedge for a cone, lasting exactly `delay`
   │        └─▶ audio: a rising bed at the anchor - where NOT to be
   │  ... the player has `delay` seconds ...
   ├─▶ CombatEvent::TelegraphEnded { id, resolved: true }  ─▶ explosion, impact sound
   └─▶ the hit test runs against THE SAME `shape` value
```

Three shapes: `Single`, `Circle { radius }`, `Cone { yaw, range, half_angle }`. The circle is
anchored on the target's position *at cast time* and the cone on the caster; **neither
follows the player**, because a telegraph that followed would be impossible to walk out of.

**The VFX did not decide anything and no new effect was authored.** `VfxId::Telegraph` (the
ring built for Meteor) and `VfxId::ConeWave` are reused, and the audio reuses
`MeteorIncoming` and the cast beds. Two new event variants were added to the existing stream;
no second event system exists.

`GROUND BLAST` is Milestone 12's telegraphed attack: 2 s of cast, a 4 m circle, 34 damage —
a fifth of the player's health, so standing in four of them is death and walking out is the
lesson.

### Tested

`a_caster_telegraphs_before_it_hits_and_hits_what_it_drew` asserts every damaged entity was
inside the announced shape; `a_telegraph_is_matched_by_exactly_one_ending` asserts the
bookkeeping.

---

## Milestone 13 — Interruption

```text
Cast ──interrupt──▶ Recover (double the ability's own recovery, at least 1 s)
  │                    ▲
  └── the telegraph comes off the floor, unresolved; no damage is applied
```

`Simulation::interrupt(entity)` stops an interruptible cast, ends its telegraph with
`resolved: false`, and announces `CastEnded { end: CastEnd::Interrupted }` — a new reason on
the *existing* enum, so one cast bar serves both actors.
`Simulation::is_interruptible(entity)` is the query a UI or an ability would ask first.

**No player interrupt ability was added**, because the phase forbids new player abilities and
the FireMage has none. The mechanism is complete and is driven in the demo scene by the
developer key `G`. Adding a real interrupt later is: a row in the FireMage's table, and a
call to `interrupt`.

Deliberately not built: crowd control, immunity, diminishing returns, interrupt locks.

---

## Milestone 14 — Targeting

The AI never inspects the player. It asks the simulation, which answers in entity ids:

| Question | Where |
| --- | --- |
| Who is angriest at me? | `ThreatTable::highest(valid)` |
| Is this target valid? | the closure `perceive` supplies: alive, hostile by faction, in the world |
| Who is nearest? | `TargetQuery::nearest_hostile` — the same query the player's TAB uses |
| How far, edge to edge? | `perceive`, using both bodies' radii |
| What am I fighting? | `AiPercept::victim()` |

Nothing anywhere says "the player". A pet, a summon, a second player or an NPC ally becomes a
target by being a live entity of a hostile faction — no AI change at all.

---

## Milestone 15 — Enemy death

On the kill, inside `deal_damage`:

* the brain is moved to `Dead` and stops thinking, moving, casting and hating;
* a cast in progress is dropped and announced (`CastEnd::CasterDied`), and its telegraph is
  ended unresolved — a cast bar whose caster died has to come down;
* statuses are cleared and reported, as they already were for dummies;
* `CombatEvent::Death` is emitted, which is what the death sound, the meter and the UI read.

**The entity is not despawned.** It stays in the world with the dead tint, exactly as a dead
dummy does, so nothing holding its id — a nameplate, a floating number, an effect — is left
pointing at a recycled slot. A corpse deals no damage and starts no attacks, which
`an_enemy_that_dies_stops_fighting_and_the_encounter_completes` asserts by standing on one
for ten seconds.

---

## Milestone 16 — The encounter controller

```text
NotStarted ──pull──▶ Active ──last death──▶ Completed
     ▲                                          │
     └──────────── Resetting ◀──────────────────┘
```

`Encounter` holds a name, spawn points, the entities they became, and the state.
`Simulation::set_encounter` spawns the members; `sync_encounter` runs once a tick and
activates on the first pull and completes when the last member dies;
`Simulation::reset_encounter` restores everybody, revives the player and resets the meter.

**No encounter logic is inside any enemy.** An enemy knows its own leash and its own home; it
does not know it is the second of five.

Deliberately not built: waves, phases, scripted timelines, adds, objectives, loot.

---

## Milestones 17 and 18 — The encounters

**`SKIRMISH`** (Milestone 17, and what `--scene encounter` loads): one BRUTE 16 m ahead and
one EMBER ADEPT 24 m ahead and to the right, so walking forward pulls the melee enemy first
and the caster second — the two behaviours are legible one at a time rather than at once.

**`PACK`** (Milestone 18): `--enemies <n>` builds a grid of `n`, alternating melee and
caster. `a_pack_all_fights_and_all_of_it_can_die` asserts that a pack of five all engage, all
generate threat, all deal damage and all die, and that simultaneous casts and telegraphs are
handled — the benchmark takes the same scenario to a hundred.

No coordination between enemies was implemented: they each independently notice, chase and
attack. That is Milestone 18's explicit instruction.

---

## Milestone 19 — The combat UI

Everything reuses the existing UI, its snapshot and its theme:

| Element | Where it came from |
| --- | --- |
| Enemy health | the existing target frame and world-space health bars |
| Enemy cast bar | **new** `push_enemy_cast_bar`, the same widget as the player's, drawn under the target frame in a different colour, with `!` when the cast can be interrupted |
| Target indicator | the existing target glow and target frame |
| Telegraph feedback | the VFX and audio directors, from events |
| Player health | the existing player bars |
| Player death | **new** `push_death_notice`, driven by `snapshot.player.alive` |

`UiSnapshot` gained `player.alive`, `player.damage_taken`, `target.casting` and
`target.ai_state` — plain data, read once a frame, exactly as everything else in it.

---

## Milestone 20 — The demo scene

`mygame --scene encounter`, beside combat, collision, character, environment, vfx and audio.

```text
walk forward       be noticed, and watch the pull
1 - 0              the class, unchanged
BACKSPACE          put the fight back
G                  interrupt the target's cast (developer control)
--enemies <n>      a pack of n instead of the scene's own two
```

The scene is built by `arena::encounter_arena`, in `engine-game`, so a headless test builds
the *identical* scene — which is exactly what `tests/encounter.rs` does.

---

## Milestone 21 — Debugging

The periodic report line (`--report <seconds>`) gained an encounter block:

```text
encounter: ACTIVE | 2 enemies | player 431 hp, took 69
  BRUTE       CHASE   320 hp | target 0 at 6.4 m | threat 1 | -
  EMBER ADEPT CAST    210 hp | target 0 at 15.9 m | threat 1 | GROUND BLAST 1.2s
```

State, health, current target, distance to it, threat rows, and the cast with its remaining
time. Developer-only and in the log rather than over the game, exactly as the audio readout
is. `AiState::as_str`, `EnemyKind::name`, `EnemyBrain::threat/cast/home/timer` and
`Simulation::enemies()` are the API behind it, and the UI's `target.ai_state` exposes the
same thing to a future overlay.

Aggro, leash and attack-range circles are **not** drawn: the collision debug view is the
place for world-space debug geometry, and adding a second one was more than the milestone
needed.

---

## Milestone 22 — Determinism

**The AI contains no randomness at all.** Ability choice is list order, timers come from the
fixed timestep, and threat order is insertion order for ties. Two consequences, both tested:

* `the_same_state_and_the_same_percept_produce_the_same_decision` — two brains fed one
  percept for 300 ticks stay bit-identical.
* `two_runs_of_the_same_fight_are_identical` — two full encounters driven by the same script
  for 900 ticks produce identical event streams.

`tests/determinism.rs` (the pre-existing suite) also covers the new events: its event
fingerprint now includes telegraphs, so a fight that drifted would fail there too.

If randomness is ever wanted — a spread of attack timings, a random target among equals — the
engine already has the pattern: an explicit `Rng` seeded from the simulation's seed, as
`FireMage` and the VFX and audio systems each have.

---

## Milestone 23 — Automated tests

**42 new tests**, none of which needs a device.

| Area | Where | Count |
| --- | --- | ---: |
| Archetypes, attacks and shapes | `enemy/mod.rs` | 6 |
| Threat | `enemy/threat.rs` | 7 |
| The state machine | `enemy/brain.rs` | 13 |
| The encounter lifecycle | `encounter.rs` | 4 |
| A real fight, end to end | `tests/encounter.rs` | 12 |

Coverage against the milestone's list: health and death (`Health`'s own suite, unchanged,
plus the death tests here); threat generation, accumulation, selection and invalid targets;
every state transition in the diagram; range in and out; casts starting, completing, being
interrupted and dealing no damage when interrupted; telegraphs appearing, disappearing and
matching their damage area; and the encounter starting, activating, completing and resetting.

Whole workspace after the phase: **1,776 tests, 0 failures.**

---

## Milestone 24 — The AI stress test

`ai_bench`, 1,200 measured frames per scenario, no device. The player is teleported into the
middle of the pack and kept standing by the harness, so the measurement is a fight in
progress rather than the moment before everybody is dead.

| Enemies | AI us | per enemy | simulation us | total us | p99 | fighting | allocs |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.1 | 0.10 | 0.7 | 1.2 | 1.9 | 1 | 7 |
| 10 | 0.9 | 0.09 | 1.9 | 4.1 | 6.8 | 4 | 43 |
| 50 | 8.4 | 0.17 | 8.2 | 22.4 | 37.2 | 33 | 161 |
| 100 | 25.1 | 0.25 | 15.9 | 51.5 | 77.9 | 46 | 236 |

AI is superlinear because perception asks for the nearest hostile, which is a scan over the
world. At a hundred enemies that is 25 us — 0.6 % of the 240 FPS budget — so it is
**measured and left alone**; the fix (a spatial index, or one shared nearest-hostile pass per
tick) is recorded as deferred work rather than taken pre-emptively.

Allocation is about 0.2 per frame at a hundred enemies: threat rows appearing for a new
attacker, and the world's restore path. Every per-tick buffer is reused.

---

## Milestone 25 — The combat stress test

The same scenarios with the whole presentation stack attached — extraction, culling, the VFX
director, the audio director and a mix on the device's own schedule:

| Scenario | AI | sim | extract | cull | VFX | audio | total | p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10, full frame | 0.9 | 1.9 | 0.9 | 0.5 | 6.6 | 0.8 | 11.5 | 29.2 |
| 50, full frame | 8.6 | 8.4 | 4.2 | 2.0 | 28.2 | 1.6 | 53.0 | 103.9 |
| 100, full frame | 25.4 | 16.2 | 7.6 | 3.3 | 39.4 | 2.1 | 94.1 | 167.4 |

Presentation adds **7.4 / 30.6 / 42.6 us** over the bare fight at 10 / 50 / 100 enemies,
nearly all of it the VFX director — telegraphs are effects, and a hundred enemies produce a
lot of them (296 telegraphs over 1,200 frames in the worst case).

**Against the 4,170 us budget of 240 FPS, the worst case uses 2.3 %.** A ten-enemy fight —
which is what the game will actually contain — uses 0.3 %. Compared with the Phase 4 baseline
the frame is unchanged when no enemies exist: the AI stage costs nothing with an empty array.

---

## Milestone 26 — Full regression

```text
cargo fmt --all -- --check                                clean
cargo clippy --workspace --all-targets -- -D warnings     clean
cargo test --workspace                                    1,776 passed, 0 failed
cargo build --workspace                                   clean
cargo build --workspace --release                         clean
```

Verified by running the game: the combat scene (2,436 FPS with four dummies), the character
scene (2,182 FPS), the VFX scene (2,598 FPS) and the new encounter scene (2,835 FPS) all
behave as before. Dummies, abilities, talents, the damage cycle, the meter, targeting, the
UI, settings, audio and every benchmark are unchanged.

The only pre-existing behaviour this phase *deliberately* changed is the combat meter's
accounting (Milestone 3) and what BACKSPACE does in a scene that has enemies (Milestone 4).
Both are documented above.

---

## Milestone 27 — Manual playtest

**Partly complete, and honestly so.** `docs/manual-tests.md` MT-024 was written with the
fifteen steps the milestone lists, and the automated equivalents all pass. What was verified
by running the game unattended: the scene loads, the encounter reports `NOT STARTED` with two
idle enemies at full health, the readout prints their state, target, threat and cast, and the
frame rate is unchanged from the neighbouring scenes.

What still needs a person: whether the telegraphs *read* — whether a player who has never
seen the cone can get out of it in 1.4 s, whether the caster's circle is legible on the arena
floor, whether the brute closing feels threatening rather than inevitable. No test can answer
those, and this phase does not claim they were answered.

---

## Architecture summary

### How would we add a new enemy type without modifying the core AI system?

One row in `ARCHETYPES`:

```rust
EnemyArchetype {
    name: "FLAME HOUND",
    max_health: 180.0, radius: 0.5, height: 1.2,
    speed: 6.8,                      // faster than the player: it cannot be kited
    aggro_radius: 18.0, leash_radius: 50.0,
    preferred_range: 2.0, range_slack: 0.5,
    alert_time: 0.15,                // it reacts fast
    abilities: [Some(EnemyAbility::Swing), None, None, None],
}
```

…plus a variant on `EnemyKind` and a line in `EnemyKind::ALL`. Nothing in `brain.rs`,
`enemy_exec.rs`, the presentation directors or the renderer changes. The state machine has no
`match` on which enemy it is driving.

### How would we add a new enemy ability without modifying the renderer, VFX or audio?

One row in `ENEMY_ABILITIES` and one entry in an archetype's ability list. Presentation
follows automatically, because it is driven by *events* and by the shape the event carries:

* a cast produces `CastStarted` → a gathering effect and a gathering sound;
* an area produces `TelegraphStarted { shape }` → a ring for a circle, a wedge for a cone,
  and a rising sound at the anchor;
* landing produces `TelegraphEnded { resolved: true }` → an explosion and an impact sound;
* damage produces `Damage` → hit flashes, floating numbers, hit sounds.

A genuinely new *shape* (a rectangle, a donut) is one variant on `TelegraphShape` with its
own `contains`, plus the mapping from that variant to an existing effect — still no renderer,
no new asset, no new pipeline.

### How would we later add AoE avoidance, interrupts, coordinated attacks and boss mechanics?

The foundation was shaped for exactly this:

* **AoE avoidance** — the telegraph is already gameplay state with a shape and a deadline. An
  enemy that avoids them queries the live telegraph list and biases its `MoveIntent`; that is
  a new `think_*` branch, not a new architecture. The player-facing half needs nothing.
* **Interrupts** — `interrupt`, `is_interruptible`, `CastEnd::Interrupted` and the
  `interruptible` flag all exist and are tested. What is missing is a player *ability* that
  calls them, which is a row in the FireMage table.
* **Coordination** — a pack-level actor would sit where `Encounter` sits: it can see every
  member's brain and could hand each one a role or a permission (only two may attack at
  once). Brains would consume that as another field on the percept. No brain needs to know
  another brain exists.
* **Boss mechanics** — a boss is an archetype with a longer ability list and an encounter with
  phases. Phases belong on the encounter (it already has a lifecycle); the ability list is
  already a priority list; and a scripted timeline is a component that pushes `AiRequest`s,
  which is the interface the brain already speaks.

---

## Known limitations and deferred work

* **No path-finding.** Enemies walk straight at their target. They will press into a wall
  rather than go round it. The intent-based interface is exactly what a path follower would
  produce, so this is a replacement of one function.
* **No separation.** Enemies can stand inside each other; they collide with the world but not
  with one another.
* **No enemy animation.** Enemies use the dummy model with a red-shifted tint. The skinned
  character path exists (the player uses it) but binding one `CharacterVisual` per enemy is
  its own piece of work.
* **No player interrupt ability** — the mechanism is complete, the button is a developer key.
* **Perception is a scan.** 0.25 us per enemy at a hundred; a spatial index or one shared
  nearest-hostile pass per tick is the fix when something needs it.
* **Threat has no decay, no multipliers and no taunt.**
* **No respawn.** The reset key restores the fight; there is no death-and-return loop.
* **No AI variation.** Two identical brutes act identically, deliberately (Milestone 22).
  Variation would need an explicit per-enemy RNG.
* **Enemies do not fight each other**, do not flee, do not call for help, and do not use
  their abilities positionally (a caster will not kite).
* **The encounter has no waves, phases or objectives.**
* **MT-024 is written and unrun by a person.**

## Future scalability

* Both tables are indexed by discriminant, so a second class of enemy — or a second game's
  worth — is rows.
* Every number an encounter designer would want to change (aggro, leash, ranges, timings,
  damage) is data, so tuning never touches code.
* The percept/decision split means a different *decision* system — a behaviour tree, a
  planner, a scripted boss — can replace `think` for one archetype without touching the
  executor, the events, or anything downstream of them.
* Because AI runs inside the fixed timestep and produces intents rather than positions, a
  future server-authoritative build moves the same code with no change to what it means.
