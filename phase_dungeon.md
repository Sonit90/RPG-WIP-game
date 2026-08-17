# Phase 7 — First Dungeon & Encounter Content

The engine already had enemies that fight back. This phase turned that foundation into one
complete, replayable dungeon: **The Ashen Reliquary** — six rooms, eight trash packs, six
monster types and a three-phase boss, playable from the entrance to the boss's death without a
developer touching anything in between.

No loot, no drops, no rewards, no XP, no progression, and **no new ability sounds**. The
dungeon ends when the boss dies, and can be reset and replayed immediately.

```text
cargo run --release --bin mygame -- --scene dungeon
cargo run --release --bin dungeon_bench
cargo test -p engine-game --test dungeon
```

---

## 1. Dungeon overview

| | |
|---|---|
| Name | The Ashen Reliquary |
| Theme | A burnt-out reliquary: an entrance hall, a gatehouse, a bone hall, a gallery of cinders, a vault, and the gate the Warden guards |
| Areas | 6 (one is the entrance hall, which holds no pack) |
| Packs | 9 (8 trash + the boss) |
| Monster types | 6 |
| Boss | 1 — Warden of the Ashen Gate, 3 phases, 5 abilities |
| Enemies in the world | 23, all present from the moment the dungeon is built |
| Player kit | The existing FireMage. No new player systems. |

The dungeon runs along −Z. The player walks forward the whole way: every room is further from
the entrance than the last, so "forward" is never ambiguous and no route doubles back.

---

## 2. Dungeon layout

| Area | Name | Centre (x, z) | Half-extent | Packs |
|---|---|---|---|---|
| 0 | Entrance Hall | (0, −14) | 12 × 12 | — |
| 1 | Gatehouse | (0, −58) | 16 × 16 | 1, 2 |
| 2 | Ossuary Hall | (0, −106) | 16 × 16 | 3, 4 |
| 3 | Cinder Gallery | (0, −154) | 16 × 16 | 5, 6 |
| 4 | Reliquary Vault | (0, −202) | 16 × 16 | 7, 8 |
| 5 | Ashen Gate | (0, −266) | 24 × 24 | Boss |

```text
   ENTRANCE (an arch in a closed wall)
      │
      ▼
  ┌────────────────┐
  │ 0 ENTRANCE HALL│  no pack
  └───────┬────────┘
          │ corridor
  ┌───────┴────────┐
  │ 1 GATEHOUSE    │  Pack 1 · Pack 2
  └───────┬────────┘
          │ corridor
  ┌───────┴────────┐
  │ 2 OSSUARY HALL │  Pack 3 · Pack 4
  └───────┬────────┘
          │ corridor
  ┌───────┴──────────┐
  │ 3 CINDER GALLERY │  Pack 5 · Pack 6
  └───────┬──────────┘
          │ corridor
  ┌───────┴───────────┐
  │ 4 RELIQUARY VAULT │  Pack 7 · Pack 8
  └───────┬───────────┘
          │ corridor
  ┌───────┴────────────┐
  │ 5 ASHEN GATE       │  BOSS
  └────────────────────┘
```

**How it is built.** `engine_environment::dungeon::ashen_reliquary()` is a deterministic
function producing 165 placements from the existing sixteen-asset library — wall segments,
pillars, arches, braziers, the plain and the sky. No new art was authored.

Every room is a whole number of eight-metre wall segments, and a doorway is exactly the two
centre segments left out. That is why the corridor and the door line up with no gap at the
junction: both are 16 m wide because both are two segments. Every wall, pillar and arch
carries its own collision box (131 shapes); the only invisible boundary is the character
controller's outer clamp at ±320 m, far outside the dungeon, which nothing can reach.

**Room size is derived from the content.** A combat room is 32 m across because two packs
stand at opposite corners of it and the widest aggro radius in the roster is 20 m. 32 m puts
them 28 m apart, which is the smallest room in which two packs are two packs. The boss arena
is 48 m across because Immolation Nova reaches 14 m and the answer to it is running further
than that. Both facts are asserted by tests rather than intended.

**No optional routes.** A first dungeon with a branch is a first dungeon whose flow cannot be
verified in one playthrough. Deferred deliberately (§14).

---

## 3. Monster roster

Six types. Every one is a row in the existing `ARCHETYPES` table — **no monster has code of
its own**, and the state machine still contains no `match` on which enemy it is driving.

| Monster | `EnemyKind` | Role | Health | Speed | Document |
|---|---|---|---|---|---|
| Brute | `Melee` | Melee bruiser | 320 | 5.2 | `docs/dungeon/monsters/brute.md` |
| Ember Adept | `Caster` | Ranged caster | 210 | 4.2 | `docs/dungeon/monsters/ember_adept.md` |
| Bone Ritualist | `Ritualist` | Interrupt-critical caster | 260 | 4.0 | `docs/dungeon/monsters/bone_ritualist.md` |
| Crypt Warden | `Warden` | Support / healer | 240 | 4.4 | `docs/dungeon/monsters/crypt_warden.md` |
| Cinder Weaver | `Weaver` | Area controller | 230 | 4.4 | `docs/dungeon/monsters/cinder_weaver.md` |
| Ash Revenant | `Revenant` | Elite specialist, 2 phases | 900 | 5.0 | `docs/dungeon/monsters/ash_revenant.md` |

Boss: `Boss` — Warden of the Ashen Gate, 6,000 health, `docs/dungeon/boss/warden_of_the_ashen_gate.md`.

Fourteen enemy abilities exist in total (four inherited from the previous phase, ten new).
Each is a row in `ENEMY_ABILITIES`; the timing, the shape, the interruptibility and the effect
all live in the table and none of them in the state machine.

---

## 4. Pack list

| # | Room | Name | Composition | Teaches |
|---|---|---|---|---|
| 1 | Gatehouse | Gatehouse Watch | 2 × Brute | Melee threat, cleave cones, positioning |
| 2 | Gatehouse | Gate Crossfire | 1 × Brute, 2 × Ember Adept | Two threat ranges at once |
| 3 | Ossuary | Bone Choir | 1 × Bone Ritualist, 2 × Brute | Interrupt under pressure |
| 4 | Ossuary | Warden's Circle | 1 × Crypt Warden, 1 × Brute, 1 × Ember Adept | Target priority: the healer first |
| 5 | Cinder Gallery | Cinder Loom | 2 × Cinder Weaver | Ground denial; keep moving |
| 6 | Cinder Gallery | Weave and Word | 1 × Cinder Weaver, 1 × Bone Ritualist, 1 × Brute | Move *and* interrupt |
| 7 | Reliquary Vault | Vault Sentinel | 1 × Ash Revenant, 1 × Brute | An enemy that changes at 50 % |
| 8 | Reliquary Vault | Last Rite | 1 × Crypt Warden, 1 × Bone Ritualist, 1 × Cinder Weaver, 1 × Brute | Everything the dungeon has taught |
| 9 | Ashen Gate | Warden of the Ashen Gate | the boss | Three phases of it |

Packs are `PackDef` rows in `engine_game::dungeon::PACKS`: a name, a room index, and each
member's kind and offset from that room's centre. Offsets rather than world positions, so
moving a room in the level moves its pack with it and the two cannot disagree.

---

## 5. Encounter progression

Difficulty is added as *axes of attention*, not as health and damage. The largest pack's total
health is 1,050 against the first pack's 640 — well under 2× — while what the player has to be
doing at once goes from one thing to four.

| Pack | Position | Line of sight | Interrupt | Target priority | Keep moving |
|---|---|---|---|---|---|
| 1 | ● | | | | |
| 2 | ● | ● | | | |
| 3 | ● | | ● | | |
| 4 | ● | | | ● | |
| 5 | ● | | | | ● |
| 6 | ● | | ● | | ● |
| 7 | ● | | | ● | ● |
| 8 | ● | ● | ● | ● | ● |
| Boss | ● | | ● | | ● |

---

## 6. Boss overview

**Warden of the Ashen Gate** — 6,000 health, three phases on health thresholds.

| Phase | Trigger | Adds | Speed | Cooldowns |
|---|---|---|---|---|
| 1 "The Gate" | Start | Warden's Sweep (uninterruptible cone), Ashfall (interruptible circle), Ember Lance, Swing | ×1.00 | ×1.00 |
| 2 "The Ash" | ≤ 66 % health | Cinder Rain: 10 s burning pools | ×1.08 | ×0.85 |
| 3 "The Burning" | ≤ 33 % health | Immolation Nova: a 14 m circle centred on the boss | ×1.15 | ×0.70 |

Every transition cancels the cast in flight, drops its telegraph, staggers the boss for 1.5 s
and emits exactly one `PhaseChanged` event. Transitions are one-way: healing the boss above a
threshold does not rewind the fight, and one hit crossing two thresholds lands in the deeper of
the two and announces only that one.

Full detail, including per-mechanic counterplay and the eight things a reset restores:
`docs/dungeon/boss/warden_of_the_ashen_gate.md`.

---

## 7. Architecture changes

Nothing in this phase added a second AI. Everything is either a new **row** in an existing
table or a new **generic capability** any enemy can use.

| Change | Where | Why it is generic |
|---|---|---|
| `EnemyEffect` on an ability | `engine-game/src/enemy/mod.rs` | `Damage` \| `Heal { amount }` \| `Hazard { duration, damage_per_second }`. Any archetype can pick any of the three. |
| `EnemyTarget` on an ability | same | `Victim` \| `WoundedAlly`. What makes a healer possible with no `HealerAI`: the ability is simply not *available* when nothing is wounded, and the enemy falls through its priority list. |
| `EnemyPhaseDef` on an archetype | same | A phase is a threshold, an ability list and two scalars. The boss and the Ash Revenant share the struct; neither has a line of code. |
| `MAX_ENEMY_ABILITIES` 4 → 6 | same | A boss's priority list is a longer list, not a branch. |
| `EnemyHazardField` | `engine-game/src/enemy/hazard.rs` | Lingering ground damage. Announced with the existing `AreaEffectStarted`/`Ended` events, so the VFX director already draws it with no change. |
| `AiPercept.health_fraction`, `.wounded_ally` | `enemy/brain.rs` | What phases and heals are read from. The brain still cannot see the world. |
| `AiDecision.phase` | same | The transition comes out of the same call the ability request does — there is no second question to forget to ask. |
| `Simulation::encounters` (many, was one) | `sim/mod.rs` | A dungeon is a list of encounters. `encounter()` still answers the single-encounter question and returns the first. |
| `Simulation::apply_encounter_resets`, `restore_enemy` | same | Resetting one pack does not touch another. The whole-world reset runs the same per-enemy restore, so the two cannot drift. |
| Per-enemy movement speed | `sim/mod.rs` | Enemies previously walked at the *player's* `max_speed`; `archetype.speed` was a number nothing read. Movement now builds a per-enemy config from the brain's current phase speed. |
| `Simulation::slay` | same | A development control that kills through the ordinary damage path, so a test arranging "this is dead" gets the death event, the cast bar, the telegraph and the hazards going out. |
| `CombatEvent::PhaseChanged` | `event.rs` | Presentation learns about phases the way it learns about everything else. |
| `engine_game::dungeon` | new module | Areas, packs, progression and reset. Owns no AI and no combat. |
| `engine_environment::dungeon` | new module | The level, as data. Depends on `engine-core` alone, like the rest of that crate. |
| `Scene::Dungeon` | `engine-app` | `--scene dungeon`. Builds its collision from the same deterministic scene function the renderer draws. |
| `TargetView.phase`, `.boss` | `engine-presentation` | The target frame prints a phase marker for anything with more than one phase, and in the enemy-cast colour for a boss. |

### The one deliberate coupling

`EnemyEffect::Hazard` announces itself with `ZoneShape::Circle`, a type that lives in
`firemage::zone`. That is a *shape*, not a FireMage concept, and reusing it is what lets the
existing VFX director draw enemy hazards with no change at all. The hazard **runtime** is
separate from the FireMage's zone field, because that one carries spell-power coefficients and
fire vulnerability, and an enemy table naming a `ZoneKind` would be gameplay's monster data
depending on the player's class.

---

## 8. Implementation details

### Milestone 1 — Dungeon design document

This file, §§1–6, written before any production code.

### Milestone 2 — Dungeon environment

`crates/engine-environment/src/dungeon.rs`. 165 placements, 131 of them blocking. Rooms are
built by `push_room`, corridors by `push_corridor`, and both use `span_centers`, which covers a
span exactly because every span is a whole number of segments. The boss arena's four inner
pillars are placed on a 13 m radius *inside* the nova's 14 m, with the reason written next to
the number.

Tests: determinism by signature, every room a whole number of segments, rooms in order and not
overlapping, the player inside the first room, walls and pillars actually built, the boss arena
large enough for its own nova, and `span_centers` covering a span exactly.

### Milestone 3 — Monster roster documentation

Six files under `docs/dungeon/monsters/`, each covering exactly one monster in the format the
brief specified, all written **before** the archetypes were added to the table.

### Milestones 4 & 5 — Monster implementation and abilities

Six rows in `ARCHETYPES` and ten new rows in `ENEMY_ABILITIES`. The brain gained phase
awareness and per-ability target selection; it gained no knowledge of any specific monster.

The ability path is unchanged and unchangeable by content:

```text
EnemyBrain::think  ──▶ AiRequest ──▶ enemy_exec ──▶ Simulation::deal_damage / heal / light_hazard
                                                        │
                                                        ▼
                                                   CombatEvent ──▶ VFX / UI / audio
```

Every telegraphed attack still carries **one** `TelegraphShape`: the value the telegraph event
carries is the value the hit test runs. A heal resolves on the ally the cast was started
against rather than on whoever the enemy is fighting, which is the one place the recipient and
the victim differ.

### Milestone 6 & 7 — Packs and pack behaviour

`PACKS` in `engine-game/src/dungeon.rs`. Every pack is populated when the dungeon is built and
stands idle at its spawn points, so entering a room is a *pull* rather than a spawn.

Pack behaviour needed no new code: threat, targeting, leashing and simultaneous combat are the
previous phase's, and the tests confirm three monsters fight at once, all target the player,
and a pack leashes home together and forgets everything.

### Milestone 8 — Encounter difficulty

Tuned by composition, not by inflation — see §5. The one stat change made for difficulty was
giving enemies their own movement speeds, which had never been wired up: before this phase
every enemy walked at 7 m/s, the player's speed, which made kiting impossible and every
"move out of it" mechanic unanswerable.

### Milestone 9 — Dungeon flow

`--scene dungeon`. The player walks from the entrance to the boss with no developer
intervention; the flow is a consequence of where the rooms are, not of anything sequencing
them, and that is what `walking_forward_pulls_the_packs_in_order` asserts.

### Milestones 10–13 — The boss

Designed in `docs/dungeon/boss/warden_of_the_ashen_gate.md` before implementation, then
implemented as one archetype row plus two `EnemyPhaseDef` rows. Phase 3 was added because it
changes what the fight is about — the first two phases are answered standing near the boss and
the third cannot be.

### Milestone 14 — Boss counterplay

Every mechanic satisfies the chain: telegraph on the floor → readable danger → time to react →
a specific action → avoided. Nothing is invisible, nothing is unavoidable, nothing is random,
and nothing requires reading the interface — the phase marker on the target frame is a
convenience, not the mechanic.

| Mechanic | Warning | Time | Action |
|---|---|---|---|
| Warden's Sweep | 11 m cone on the floor | 1.7 s | step sideways |
| Ashfall | 6 m ring under the player | 2.2 s | interrupt or leave |
| Ember Lance | cast bar | 1.0 s | accept or interrupt |
| Cinder Rain | 4.5 m ring, then a drawn pool | 2.0 s + 10 s | leave, then stay out |
| Immolation Nova | 14 m ring around the boss | 3.2 s | run past the pillars |
| Phase change | boss stops, cast disappears | 1.5 s | reposition |

### Milestones 15 & 16 — Boss and dungeon reset

`Simulation::restore_enemy` is the single per-enemy restore: brain reset (state, phase, target,
threat, timers, cooldowns, cast), telegraph ended with an event, hazards extinguished with
events, health restored, body put back at its home. `apply_encounter_resets` runs it for one
pack; `reset_encounter` runs it for every enemy in the world, revives the player, clears every
hazard and finishes every encounter's reset. `Dungeon::reset` calls the latter, so a field
added to an enemy later is covered without the dungeon changing.

Resets are *requests* carried out at the top of the next tick, before any brain thinks, so
nothing spends a tick acting on a fight that is being taken down.

### Milestone 17 — Dungeon UI

Nothing new was added to the HUD framework. What the dungeon needed was already there — player
health, target health, the enemy cast bar, nameplates, telegraphs on the floor — and the one
gap was that a fight which changes has to say so. The target frame now prints a phase marker
(`P1`/`P2`/`P3`) for anything with more than one phase, in the enemy-cast colour for a boss.
No loot UI, no reward UI, no progression UI.

### Milestone 18 — Encounter debugging

The developer overlay (F5, `debug-overlay` feature, absent from a `--no-default-features`
release build) now reports:

* one line per pack: name, state, alive/total, resets;
* live hazard count alongside the enemy count;
* per-enemy state, health, **phase name**, target, distance, threat entries and current cast;
* enemies that are merely idle are omitted once there are more than eight, so a dungeon's
  twenty-three do not bury the two that are doing something.

Existing debug drawing (F3 collision, telegraph shapes, aggro/leash rings) is unchanged and
covers the dungeon because it was never arena-specific.

---

## 9. Testing results

```text
cargo test --workspace       1,948 passed, 0 failed
```

New tests: **28** in `crates/engine-game/tests/dungeon.rs`, plus 7 in
`engine-game/src/dungeon.rs`, 7 in `engine-environment/src/dungeon.rs` and 5 in
`engine-game/src/enemy/hazard.rs`.

### Monsters

| Test | What it proves |
|---|---|
| `every_monster_in_the_roster_is_distinct_and_reachable` | every archetype can reach its own attacks, leashes further than it aggros, and is slower than the player **in every phase** |
| `a_brute_closes_and_a_caster_holds_its_distance` | preferred range is respected per archetype |
| `the_ritualists_nova_is_interruptible_and_an_interrupt_costs_it_the_cast` | the interrupt path, and that an interrupted cast deals nothing |
| `the_ritualists_nova_hits_what_is_inside_its_ring_and_nothing_outside_it` | the drawn shape is the hit test |
| `the_warden_heals_the_ally_that_is_worst_off_and_nothing_when_nobody_is_hurt` | ally selection, and that a healer with nothing to heal does not cast |
| `a_mend_that_is_interrupted_restores_nothing` | heals go through the same interrupt path |
| `a_weaver_leaves_burning_ground_that_expires_on_its_own` | hazard duration and its end event |
| `burning_ground_damages_the_player_standing_in_it_and_never_its_own_side` | faction filtering on hazard ticks |
| `a_weavers_pools_go_out_when_it_dies` | no orphaned hazards |
| `the_revenant_changes_phase_once_at_half_health_and_never_goes_back` | one-way transitions, one event, a genuinely different rotation |

### Packs

`a_pack_activates_when_one_member_is_pulled_and_completes_when_all_are_dead`,
`every_member_of_a_pack_fights_at_the_same_time`,
`a_pack_leashes_home_together_and_forgets_everything`,
`resetting_one_pack_leaves_the_others_exactly_where_they_were`, and — in the module tests —
`every_pack_stands_inside_the_room_it_belongs_to` and
`two_packs_in_one_room_cannot_pull_each_other`.

### Boss

`the_boss_has_three_phases_and_each_one_adds_a_mechanic`,
`the_boss_crosses_both_thresholds_in_order_and_announces_each_once`,
`one_hit_that_crosses_two_thresholds_lands_in_the_deeper_phase`,
`a_phase_transition_cancels_the_cast_and_takes_its_telegraph_down`,
`the_bosses_uninterruptible_mechanics_cannot_be_stopped_and_its_one_window_can`,
`the_nova_hits_at_its_radius_and_misses_just_outside_it` (13.9 m in, 14.1 m out, and the cast
long enough to outrun at 7 m/s), `the_boss_can_be_killed_and_its_death_completes_its_encounter_once`,
`a_boss_reset_puts_back_every_piece_of_it`.

### Telegraphs

Shape equality between what is drawn and what is hit is asserted per ability, and the hazard
tests assert cadence (`a_hazard_ticks_at_a_fixed_cadence_and_stops_at_its_duration`), catch-up
without lost ticks (`one_long_step_does_not_swallow_ticks`) and edge-of-body containment.

### Dungeon

`the_dungeon_starts_asleep_with_every_pack_standing_where_it_belongs`,
`nothing_notices_the_player_until_they_walk_forward`,
`walking_forward_pulls_the_packs_in_order`,
`a_dungeon_reset_leaves_no_stale_state_anywhere`,
`a_wipe_during_a_cast_and_a_telegraph_still_resets_cleanly`,
`the_dungeon_is_deterministic_over_a_long_run`.

Every test is deterministic: no clock, no random number, fixed timestep.

---

## 10. Performance results

`cargo run --release --bin dungeon_bench -- --frames 600`, no device, 300 warm-up frames.
Target: 240 FPS is 4,167 µs a frame.

| Scenario | AI µs | Sim µs | Extract µs | Cull µs | VFX µs | Audio µs | Total µs | p99 µs | Allocs |
|---|---|---|---|---|---|---|---|---|---|
| 1 pack | 2.5 | 6.3 | 2.0 | 0.6 | 2.7 | 0.4 | 14.5 | 41.5 | 2,405 |
| 3 packs | 2.6 | 6.3 | 1.6 | 0.6 | 5.4 | 0.7 | 17.1 | 30.9 | 2,402 |
| 5 packs | 2.6 | 7.2 | 1.7 | 0.6 | 7.2 | 0.8 | 20.0 | 50.7 | 2,406 |
| full dungeon | 2.9 | 7.6 | 1.9 | 0.6 | 8.7 | 0.9 | 22.6 | 67.9 | 2,403 |
| boss encounter | 2.7 | 7.2 | 1.6 | 0.3 | 3.5 | 0.6 | 15.8 | 47.4 | 2,405 |
| boss, worst case | 2.9 | 7.4 | 1.7 | 0.3 | 4.3 | 0.6 | 17.2 | 63.7 | 2,413 |

| Scenario | Entities | Hazards (peak) | Effects (peak) | Voices (peak) | Mix µs |
|---|---|---|---|---|---|
| 1 pack | 24 | 0 | 10 | 14 | 16.9 |
| 3 packs | 24 | 0 | 15 | 26 | 37.8 |
| 5 packs | 24 | 4 | 20 | 26 | 39.8 |
| full dungeon | 24 | 3 | 21 | 27 | 47.0 |
| boss encounter | 24 | 0 | 10 | 15 | 23.2 |
| boss, worst case | 24 | 1 | 14 | 15 | 27.6 |

**Against the budget:** the worst p99 is the full dungeon at 67.9 µs — **1.6 % of the 4,167 µs
that 240 FPS allows.** The AI cost is essentially flat from one pack to nine, because every
brain runs every tick whether or not it has been pulled and 23 brains is not a number that
matters. What grows is the VFX director, and it grows with what is on fire rather than with
what exists.

The "worst case" scenario holds the boss in phase 3 by threshold while every trash pack in the
dungeon is also pulled and alive — a state the game cannot produce.

**Rendered, on the target hardware** (RX 6700 XT, 1920×1080, release, `--frames 3000`):

| Scene | Average FPS | CPU mean | GPU mean | Render entities |
|---|---|---|---|---|
| Dungeon | 1,608 | 0.62 ms | 0.065 ms | 185 |
| Combat arena (baseline) | 1,493 | 0.67 ms | 0.066 ms | — |

The dungeon renders slightly *cheaper* than the existing arena: 165 placements of 16 uploaded
models against the arena's scatter. Both scenes show the same CPU p99 outliers (5–7 ms) and the
same max (~17 ms) on an uncapped loop, so those are pre-existing and not something this phase
introduced.

**Allocations:** ~2,400 over 600 measured frames, or four per frame, identical across
scenarios — so it does not scale with the dungeon and is not hazard, pack or phase related.
Left as-is; see §13.

---

## 11. Full playtest results

`walking_forward_pulls_the_packs_in_order` is the playthrough, run as a test: the player is
driven forward from the entrance with movement commands only — **no teleporting between
encounters** — and every one of the nine packs is observed activating, in order, ending with
the boss. It is asserted that the order the packs wake in is exactly their index order.

Verified by that run and the ones around it:

* no broken transitions — every room is reachable on foot through its corridor;
* no stuck enemies — the leash test brings a whole pack home and returns it to Idle;
* no unexpected aggro — `nothing_notices_the_player_until_they_walk_forward` runs ten seconds
  of standing still in the entrance hall with every enemy staying Idle, and
  `two_packs_in_one_room_cannot_pull_each_other` proves two packs sharing a room stand further
  apart than either can notice;
* no combat state leaks — the reset tests check state, phase, cast, threat, position and
  health for every member of every pack;
* no stale VFX — every hazard removal emits `AreaEffectEnded`, which is what the VFX director
  keys its removal on, and every telegraph taken down by a death, an interrupt, a phase change
  or a reset emits `TelegraphEnded`;
* no stale telegraphs or UI — the target frame reads live snapshot state, and the phase marker
  disappears with the target.

The dungeon was also launched for real (`--scene dungeon --frames 3000`): it builds, uploads
16 models, places 185 render entities, runs and shuts down clean.

**What was not done:** a human did not sit down and clear the dungeon by hand. Everything
above is either an automated behavioural test or a real run of the executable. A hand
playthrough remains the honest way to judge *pacing*, and it is the first thing to do with
this build.

---

## 12. Reset / wipe testing

| Scenario | Result |
|---|---|
| Player dies to trash | Enemies lose their target, cancel casts, leash home; reset revives and restores |
| Player dies to the boss | Same, and the boss's phase and hazards go with it |
| One enemy dies | Its cast bar comes down, its telegraph ends unresolved, its hazards go out |
| Several die at once | Each announces its own death; the encounter completes exactly once |
| Phase transition then wipe | `a_wipe_during_a_cast_and_a_telegraph_still_resets_cleanly` — cast gone, telegraph ended, phase back to 1 |
| Active telegraph during death | `TelegraphEnded { resolved: false }` is emitted from the death path |
| Active cast during reset | `restore_enemy` ends the telegraph and clears the cast before the brain is reset |
| Reset one pack | Only that pack is restored; the rest keep their damage |
| Reset the dungeon | Every pack, the boss, the player, every hazard, every encounter state |

No stale combat state survives a reset. The player can retry immediately: the encounter goes
back to `NotStarted`, which means it can be pulled again on the next tick.

---

## 13. Known limitations

1. **No hand playthrough.** Pacing, readability and "is it fun" have not been judged by a
   human. Every claim in this document is either a test or a measurement.
2. **Threat is damage-only.** There is no taunt, no threat modifier and no aggro table
   readout. Multi-target threat behaves correctly but is not *interesting* yet.
3. **Packs do not coordinate.** Deliberately, per the brief. Monsters in a pack act
   independently; there is no focus fire, no flanking and no pulling-in of neighbours.
4. **The Crypt Warden can heal itself.** Correct per its documentation, and it makes a
   Warden fought alone slightly tedious. It is only ever fought in a pack.
5. **Hazards are circles only.** `EnemyEffect::Hazard` takes its footprint from the ability's
   `TelegraphShape`, and only `Circle` produces a sensible pool. A cone-shaped hazard would
   need the shape carried into the hazard field.
6. **The boss has no enrage.** A player who never dies can fight it indefinitely. An enrage
   timer is a reward-adjacent pressure mechanic and was left out of a phase with no rewards.
7. **Four allocations a frame** in the steady state, unchanged across every scenario, so not
   dungeon-specific. Not chased, because the measurement says it costs nothing at this scale.
8. **Uncapped-loop CPU outliers** (p99 ~6 ms, max ~17 ms) are present in the dungeon and in
   the pre-existing combat arena equally. Pre-existing; not investigated here.
9. **No dungeon-completion state.** When the boss dies the dungeon is simply over; nothing
   announces it beyond the encounter reaching `Completed`. That is a reward-shaped feature and
   was deliberately not built.
10. **The entrance hall is empty.** It exists so that arriving is not the same event as being
    noticed. It is not decorated beyond its walls, pillars and braziers.

---

## 14. Deferred work

* Optional routes and branching layouts.
* A second dungeon — the point of the layout being a function is that this is a new function.
* Coordinated pack AI, patrol paths and pulling neighbours.
* Taunt, threat modifiers and a threat readout.
* Non-circular hazards, hazards that move, and hazards that grow.
* Boss adds, summoning, and any mechanic that spawns entities mid-fight.
* An enrage timer and any other soft failure condition.
* Dedicated ability sounds for the ten new enemy attacks — **explicitly out of scope for this
  phase**, and the sound director's table deliberately maps every new ability onto one of the
  three gathers that already exist.
* A dedicated boss frame as its own HUD element, with a phase name rather than a marker.
* Difficulty modes.

---

## 15. Future dungeon / content considerations

* **Content is data; the systems are not.** Everything added here except two small generic
  capabilities was a table row. The next dungeon should cost the same shape of work.
* **The tables are getting long.** `ENEMY_ABILITIES` is 14 rows and `ARCHETYPES` is 7. Around
  50 and 30 respectively, a loader becomes worth its parser, its schema and its error path —
  and the migration is one module, because nothing outside `enemy/mod.rs` knows how the tables
  were produced.
* **Encounter scripting is the next real capability.** Everything in this dungeon is reactive:
  health thresholds and priority lists. A timeline — "at 30 s, do this" — is the first thing a
  second boss will want, and it belongs above the encounter, not inside the brain.
* **Rewards belong outside the encounter.** See the question below.
* **The arena is the unit of level design.** A room that is a whole number of wall segments,
  with doorways that are segments left out, made the corridor/door alignment free. Keep that
  constraint.

---

## Content graph

```text
                         DUNGEON
                            │
             ┌──────────────┴──────────────┐
             ↓                             ↓
          TRASH PACKS                  BOSS ENCOUNTER
             │                             │
      ┌──────┼──────┐              ┌──────┼──────┐
      ↓      ↓      ↓              ↓      ↓      ↓
    Melee  Caster  Support       Phase 1 Phase 2 Phase 3
      │      │      │              │      │      │
      └──────┼──────┘              └──────┼──────┘
             ↓                            ↓
          COMBAT                       MECHANICS
             │                            │
             └────────────┬───────────────┘
                          ↓
                     GAMEPLAY EVENTS
                          ↓
                  Animation / VFX / UI
```

---

## The four questions

### How can a new monster be added without modifying the dungeon system?

A monster is a row in `ARCHETYPES` and, if it needs attacks nothing else has, rows in
`ENEMY_ABILITIES`. Neither the dungeon module, the encounter, the state machine, the executor,
the renderer nor the UI contains a `match` on which monster it is handling — the brain reads
the archetype it was given, and `engine_game::dungeon::PACKS` names an `EnemyKind` in a list.

Adding a seventh monster and putting it in a pack is two table edits. The only thing that must
be written first is its document under `docs/dungeon/monsters/`.

### How can a new boss mechanic be added without modifying the renderer?

Because a mechanic is announced, never drawn. Gameplay emits `TelegraphStarted` carrying the
**gameplay** `TelegraphShape`, `AreaEffectStarted` carrying the gameplay footprint, `CastStarted`
carrying the duration, and `PhaseChanged` carrying the phase. The VFX director turns each of
those into an effect by reading the shape off the event; it has no table of boss mechanics.

Cinder Rain is the proof: it was added as a table row with `EnemyEffect::Hazard`, and the
existing director drew its ring, its pool and the pool's removal without a line of renderer or
VFX code changing.

The rule that makes this safe is that there is **one** shape value: the telegraph event carries
the same `TelegraphShape` the hit test runs, so a mechanic cannot be drawn differently from the
way it resolves.

### How can the same monster be reused in a different dungeon with different pack composition?

A `PackDef` is a name, a room index and a list of `(EnemyKind, x, z)` offsets. Nothing about a
monster records which pack it belongs to, and nothing about a pack is stored on the monster —
membership lives on the `Encounter`, which is built from the pack definition at load time.

A second dungeon is a second scene function and a second `PACKS` table. Every monster in this
roster can appear in it, in any composition, at any position, with no change to the roster.
`Brute` already appears in six packs of five different compositions.

### How can the boss later receive loot/rewards without coupling rewards to the encounter implementation?

Because the encounter already announces everything a reward system would need, to nobody in
particular. `CombatEvent::Death { entity, killer, tick }` is emitted for the boss exactly as it
is for a Brute, and `Encounter::observe_alive` moves the encounter to `Completed` exactly once
and logs it.

A reward system is a new observer of the event stream — the same position the damage meter, the
VFX director and the audio director already occupy. It would read `Death` for a boss entity, or
watch the encounter reach `Completed`, and do whatever it does. Nothing in `enemy`, `encounter`,
`dungeon` or `sim` would learn that loot exists, which is exactly the property the phase's "do
not implement rewards" rule was protecting.

---

## Milestones

- [x] M1 — Dungeon design document
- [x] M2 — Dungeon environment
- [x] M3 — Monster roster documentation
- [x] M4 — Monster implementation
- [x] M5 — Monster abilities
- [x] M6 — First monster packs
- [x] M7 — Pack behaviour
- [x] M8 — Encounter difficulty
- [x] M9 — Dungeon flow
- [x] M10 — Final boss design
- [x] M11 — Boss phase 1
- [x] M12 — Boss phase 2
- [x] M13 — Boss phase 3
- [x] M14 — Boss counterplay
- [x] M15 — Boss reset
- [x] M16 — Dungeon reset
- [x] M17 — Dungeon UI
- [x] M18 — Encounter debugging
- [x] M19 — Automated tests
- [x] M20 — Monster documentation validation
- [x] M21 — Boss documentation validation
- [x] M22 — Full dungeon playtest (automated; no hand playthrough — see §11)
- [x] M23 — Failure / wipe testing
- [x] M24 — Performance test
- [x] M25 — Final regression
- [x] M26 — Final playable dungeon

### M20 / M21 — where documentation and implementation disagreed

Three disagreements were found and resolved by changing **both**, with the implementation
taken as correct in each case:

1. **The dungeon gained an entrance hall and a sixth area.** The design put the first pack in
   the first room with the player. Implementation showed two packs cannot share a 32 m room
   *and* be separated by more than the 20 m widest aggro radius while the player also starts
   there. The layout gained a pack-free entrance hall; `docs/dungeon/README.md` and §2 were
   rewritten.
2. **The boss arena is a 48 m square, not a 44 m circle.** Round rooms are not expressible
   with eight-metre wall segments without scaled geometry, and a scaled segment's mesh and
   collision box are two numbers that drift. The arena became a square hall 24 m from centre
   to wall — still comfortably outside the nova's 14 m — and the boss document's Arena section
   was rewritten.
3. **Corridors are 16 m wide, not 8 m.** A doorway is two wall segments left out, and a
   corridor narrower than its doorway leaves a gap at the junction the player can walk out of.
   Both were made 16 m, and the README says so.

Everything else — every stat, cooldown, cast time, radius, damage number, threshold and scalar
in the seven monster and boss documents — matches the tables as implemented, and the numbers
that *are* mechanics (the 14 m nova, the 66 % and 33 % thresholds, interruptibility) are
asserted by tests.
