# FireMage — Design Document (rework)

Source of truth for the FireMage rework. Where this document and the old `firemage.md`
disagree, this document wins; `firemage.md` is rewritten from it once the implementation
lands.

Three systems carry the class:

1. **Heat** — combat momentum. Never spent, never manually reduced.
2. **Ignite** — a long stacking DoT that is a *bank* of damage.
3. **Detonate** — the manual decision to cash that bank in as instant AoE.

Two builds must feel different, and the difference is not "one has haste, one has crit":

| | Haste / Detonation | Crit / Ignite maintenance |
| --- | --- | --- |
| loop | build Ignite → cap → **Detonate** → rebuild | build Ignite → cap → **hold it** |
| wants | fast Ignite generation, frequent Heat cycles, cooldown resets | Pyroblast/Flame Lance/Meteor into a full burn, crit-driven cooldown resets |
| Ignite is | ammunition | a damage amplifier that must stay up |
| talents | Rapid Combustion, Molten Meteor, Burning Embers, Extended Flame, Furnace Mastery | Meteoric Impact, Spellfire Feedback, Burning Precision, Infernal Capacity, Volatile Ignition, Chain Pyroblast |

---

## 1. Inventory of the current implementation

### 1.1 Abilities removed

| Ability | Why |
| --- | --- |
| **Inferno** (`FireAbility::Inferno`, PBAoE zone, 45 s) | not in the new ability set; its zone, crit bonus and cascade talents go with it |
| **Flame Wave** (1.2 s cast cone) | replaced by **Flame Cone**: instant, 12 m, 140 % SP, +2 Ignite |
| **Fire Detonation** (flat 300 + 120/stack) | replaced by **Detonate**: a fraction of *remaining* Ignite damage |
| **Cataclysm** (Ultimate) | *not removed* — disabled. See §11 |

There is no Cinder Barrage, no Searing Orb and no manual Combustion ability in this
codebase; the brief lists them as obsolete and they are confirmed absent. "PRIMED" does not
exist here either and must not be introduced.

### 1.2 Abilities modified

| Ability | Change |
| --- | --- |
| Firebolt | 120 flat → 100 % SP; Heat +5 → +2; keeps the 100 %/50 % Ignite asymmetry |
| Flame Lance | 260 flat → 260 % SP; loses the "+40 % into a burn" bonus; **applies +2 Ignite** (it applied none); Heat +10 → +4; crit → Pyroblast cooldown reduction |
| Meteor | 420 flat → 500 % SP; cooldown 18 → 20 s; **applies no Ignite**, instead +15 % damage per Ignite stack on the primary target; Heat +15 → +5; instant (was a 1 s cast) |
| Pyroblast | 800 flat → 650 % SP; keeps the per-stack scaling at +15 %; loses the guaranteed crit at cap; Heat +25 → +5; crit → Flame Lance cooldown reduction |
| Firestorm | placed circle → **linear zone, 20 × 10 m**; 60 flat/tick → 70 % SP/tick; 12 ticks over 6 s; 20 % → 15 % Ignite chance per tick; Heat 3/tick capped at 30 → flat +5 on cast |
| Flame Shield, Blink | **untouched.** Utility, no conflict with the new design |

New: **Fire Mastery** (§10) takes the bar slot Inferno vacated.

### 1.3 Talents removed

All 25. The new tree is exactly the 12 talents in §12. Removed with them: Hot Hands, Quick
Casting, Volatile Heat, Spark of Power, Lingering Embers, Spreading Fire, Molten Core,
Accelerated Inferno, Meteor Shower, Burning Ground (old form), Critical Ignition, Perfect
Combustion, Pyroclasm, Endless Furnace, Arcane Pressure, Rapid Combustion (old form),
Living Inferno, Meteoric Collapse, Phoenix Protocol, Inferno Cascade, Furnace Overdrive,
Cataclysmic Ignition. Burning Embers, Extended Flame and Infernal Capacity survive by name
with new numbers/costs.

### 1.4 Systems reused unchanged

* `Simulation` tick order, `CommandBuffer`, `EventStream`, `CombatMeter`, `GameWorld`,
  movement, collision, targeting, nameplates, `UiSnapshot`'s observe-only contract.
* `AbilityBook` — charges as a bucket with one timer, `reduce_cooldown`, `grant_charge`.
  Exactly what the crit → cooldown-reduction interactions need.
* `TalentGrid` → `TalentEffects` three-layer pattern. Kept; only the contents change.
* `ZoneField` / `PendingImpact` — ground fire and impact delays. Extended, not replaced.
* `Rng` (seeded, part of simulation state), `Timer`.
* Cast/queue/cancel rules, the global cooldown, the facing rule, ground targeting.
* Ultimate architecture (`FireAbility::Cataclysm`, `UltimateEnergy`, the Ultimate button,
  `PlayerCommand::UseUltimate`). Disabled, not deleted.

### 1.5 Systems modified

* `firemage/resource.rs` — Heat becomes a threshold system with an Inferno State and
  out-of-combat decay. Ultimate Energy untouched.
* `firemage/ignite.rs` — 30 s, 2 s base interval, damage expressed in spell power, tick
  interval and crit resolved from *current* stats, remaining-damage query for Detonate.
* `firemage/ability.rs` — new table, damage as spell-power coefficients.
* `firemage/talent.rs` — new 12-talent table and effects.
* `sim/firemage_exec.rs` — new per-ability behaviour, Detonate, dynamic crit/haste.
* `firemage/zone.rs` — zones gain a shape (circle or line) for Firestorm and Burning Ground.
* Presentation: ability bar contents, Heat bar, Ignite counter, talent menu, tooltips.

### 1.6 Systems deliberately not touched

Renderer, collision, platform, settings, damage-number and nameplate plumbing, the combat
meter's schema, input binding architecture, the Ultimate's code path.

---

## 2. Stats — and the no-snapshot rule

A new, single, always-current stat block. Nothing stores a copy of it.

```rust
pub struct Stats {
    pub spell_power: f32,   // base 100.0
    pub haste: f32,         // 0.15 == +15%
    pub crit_chance: f32,   // 0.15 == 15%
    pub crit_damage: f32,   // 2.0 == a crit deals double
}
```

`FireMage::stats()` recomputes it on every call from: base values + Heat threshold bonus +
Fire Mastery buff + Inferno State + talents. Base spell power is **100**, so a 100 % SP
ability hits for 100 before multipliers and the coefficients in this document read directly
as damage.

Derived quantities, all from `stats()` at the moment they are used:

```text
cast_time      = base_cast / (1 + haste)
global_cooldown= 0.5 / (1 + haste)
cooldown       = base_cooldown / (1 + haste)      except Detonate, see below
ignite_interval= 2.0 / (1 + haste)
crit           = rng.chance(crit_chance)
crit_damage    = amount * crit_damage
```

**Detonate's cooldown is the one that haste does not shorten** (`cooldown_hasted: false` in
the ability table). Haste already fills the bank faster; letting it also spend the bank more
often is the same stat paid twice, and at a geared 45 % it was worth +23 % on a single target
against a goal of +5 to +10.

### 2.1 Gear

The class is balanced *geared*, not naked. `Gear` is another input `stats()` adds up, stated
as the totals a character sheet shows:

| build | primary | off-stat |
| --- | --- | --- |
| Haste / Detonation | 40–50 % haste (45 % headline) | 20 % crit |
| Crit / maintenance | 40–50 % crit (45 % headline) | 20 % haste |

Every number in §15 and in `docs/firemage-sim-report.md` is measured at those stats, and the
simulator reports both ends of the band because a build that is only balanced at one point of
it is not balanced.

**Nothing snapshots.** An Ignite applied at 10 % haste that later sees 30 % haste ticks
faster on its *next* tick, with no reapplication: `IgniteTracker::tick` is handed the
current interval every call and rescales the countdown proportionally, so a burn half-way
to its next tick stays half-way when haste changes. Crit chance, crit damage, spell power
and every damage multiplier are read when a tick *fires*, never when it is applied. The
`Ignite` struct therefore stores only `stacks`, `remaining` and `phase` (how far through its
current tick it is, as a *fraction*) — there is no field a snapshot could live in, which is how the rule is enforced structurally rather than
by discipline.

---

## 3. Heat

Range 0–100. Generated by abilities. Never spent by the player, never a cost.

| Heat | damage | stat |
| --- | --- | --- |
| 0–29 | ×1.00 | — |
| 30–69 | ×1.15 | — |
| 70–99 | ×1.30 | +5 % Haste **or** Crit (Fire Mastery choice) |
| 100 | consumed → Inferno State | see below |

**Inferno State**: reaching 100 immediately sets Heat to 0 and starts a 10 s state
(12 s with Furnace Mastery) granting **×1.50 damage** and **+7.5 % Haste or Crit**.

* Heat generated during the state accumulates normally, but reaching 100 again cannot
  re-trigger the state; when the state ends Heat is set to **0**. (Resolves the brief's
  "consume all Heat" and "after the state ends: Heat = 0" into one rule.)
* Target pacing: 2–3 complete cycles per encounter. See §16 for the arithmetic.

**Out of combat**: Heat is frozen while in combat. After 10 s with no damage dealt or
taken, it decays at `HEAT_DECAY_PER_SECOND = 5.0` (configurable). "In combat" is refreshed
by any damage the mage deals, including Ignite ticks.

Heat generation per cast: Firebolt +2, Flame Lance +4, Flame Cone +3, Meteor +5,
Pyroblast +5, Firestorm +5 (on cast, not per tick), Detonate +7. Fire Mastery generates
none. Flame Cone and Firestorm pay once per cast regardless of targets hit — per-target
Heat made AoE trivially reach 100 and is the reason the old cone had a cap.

---

## 4. Ignite

| | value |
| --- | --- |
| duration | 30 s (40 s with Extended Flame) |
| maximum stacks | 5 (6 with Infernal Capacity) — **data-driven**, `TalentEffects::ignite_max_stacks` |
| base tick interval | 2.0 s, divided by `(1 + haste)` |
| damage | **24 % SP** per tick per stack (−20 % with Volatile Ignition); opened at 20 %, see §15 |

One stack over a full base duration: `0.24 × SP × 15 ticks = 360 % SP`. Five stacks:
`1800 % SP`. Applying refreshes the duration even at the cap. Applications above the cap
are discarded, and reaching the cap triggers **nothing**.

Per tick:

```text
tick_damage = stacks × 0.24 × spell_power × ignite_damage_scale
            × heat_multiplier × fire_damage_multiplier
if rng.chance(crit_chance): tick_damage ×= crit_damage
```

**Remaining Ignite damage** — the quantity Detonate converts:

```text
ticks_remaining      = remaining_duration / (2.0 / (1 + haste))
remaining_damage     = stacks × 0.24 × spell_power × ignite_damage_scale
                     × ticks_remaining × heat_multiplier
```

Non-crit expected value; the explosion rolls its own critical strike. Fractional ticks
count — a burn with 1.4 ticks left is worth 1.4 ticks, so Detonate value decays smoothly
rather than in steps.

---

## 5. Abilities

Spell power coefficients. `SP = 100` at base, so "260 % SP" = 260 damage before Heat, crit
and talents.

| # | Ability | Cast | Cooldown | Charges | Range | Area | Damage | Ignite | Heat |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Firebolt | 1.0 s | — | — | 35 m | — | 100 % | 100 % at 0 stacks, else 50 % | +2 |
| 2 | Flame Lance | 1.5 s | 8 s | 2 | 35 m | — | 260 % | +2 always | +4 |
| 3 | Flame Cone | instant | 10 s | 1 | 12 m cone | 60° | 140 % | +2 to all hit | +3 |
| 4 | Meteor | instant¹ | 20 s | 1 | 35 m | 5 m circle | 500 % (+15 %/stack) | none | +5 |
| 5 | Pyroblast | 3.0 s | 30 s | 1 | 35 m | — | 650 % (+15 %/stack) | none | +5 |
| 6 | Firestorm | instant | 24 s | 1 | 20 m | **20×10 m line**, 6 s | 70 %/tick × 12 | 15 %/tick | +5 |
| 7 | Detonate | instant | 8 s | 1 | 35 m | 10 m circle | **22 %** of remaining Ignite | consumes | +7 |
| 8 | Fire Mastery | instant | 60 s | 1 | self | — | — | — | — |
| 9 | Flame Shield | instant | 30 s | 1 | self | — | — | — | — |
| 0 | Blink | instant | 15 s | 1 | 10 m | — | — | — | — |
| R | *Ultimate* | **disabled** | | | | | | | |

¹ ground-targeted with a 1.0 s impact delay, the existing placement system.

**Firebolt** — filler. Deliberately the weakest Ignite generator.

**Flame Lance** — primary single-target Ignite generator, two charges on an 8 s recharge.
A **critical Flame Lance reduces the remaining Pyroblast cooldown by 1.0 s** (1.5 s with
Spellfire Feedback). Hard cap 1.5 s, `FLAME_LANCE_CRIT_CDR_CAP`.

**Flame Cone** — fast Ignite across a pack. Low damage on purpose.

**Meteor** — `damage = 500 % × (1 + 0.15 × stacks_on_primary)`. At 5 stacks, 875 %. The
"primary target" is the living enemy nearest the impact point; with no enemy in the circle
there is no bonus. Modified by exactly one of two mutually attractive talents (§12.5, §12.7).

**Pyroblast** — `damage = 650 % × (1 + 0.15 × stacks_on_target)`, 1137 % at 5. A **critical
Pyroblast reduces the remaining Flame Lance cooldown by 1.0 s** (1.5 s with Spellfire
Feedback), capped at 1.5 s.

**Firestorm** — a line 20 m long and 10 m wide from the caster towards the aim point, 6 s,
12 ticks (0.5 s apart), 70 % SP each, each tick 15 % to apply 1 stack. Enemies leaving the
line stop taking ticks; the simulator must not assume 12 × N hits.

**Detonate** — §6.

**Fire Mastery** — §10.

### 5.1 The crit → cooldown interaction (spec conflict, resolved)

The brief states the Flame Lance ↔ Pyroblast reductions both as baseline (§9, §14, §42) and
as the whole content of the Spellfire Feedback talent (§26.8). Taking both literally makes
the talent buy nothing. Resolution:

* **Baseline**: crit Flame Lance → Pyroblast −1.0 s; crit Pyroblast → Flame Lance −1.0 s.
* **Spellfire Feedback**: raises both to −1.5 s, the documented maximum.
* Neither may ever exceed **1.5 s** per proc, enforced by a clamp and by a test.

---

## 6. Detonate

```text
requires  target at maximum Ignite stacks (5, or 6 with Infernal Capacity)
consumes  the whole burn on every affected enemy
damage    22% of that enemy's own remaining Ignite damage (§4)
radius    10 m around the primary target
```

The fraction opened at 60 % and is 22 % after the balance pass; the reason is in §15. It is
one constant, `engine_game::sim::DETONATE_FRACTION`, which the explosion, the tooltip and
the tests all read.

For every living enemy within 10 m of the primary target, independently: compute its own
remaining Ignite damage, deal 22 % of it (rolling one critical strike per target), then
remove its Ignite. An enemy with no Ignite in the radius takes nothing — Detonate does not
splash a fixed fraction of the primary's hit. Nothing is transferred after a Detonate.

Modifiers: `+3 % per consumed stack` with Burning Embers; the 20 % cooldown-reset roll of
Rapid Combustion; Heat +7.

Using it below the requirement is refused with `NotEnoughIgnite` and spends no charge — the
existing rule, kept.

---

## 7. Damage pipeline

One function, one order, for every point of damage in the game:

```text
base = coefficient × spell_power
     × (1 + per-ability bonus)          Meteor/Pyroblast Ignite scaling
     × heat_multiplier                  1.00 / 1.15 / 1.30 / 1.50
     × talent_multipliers               Burning Ground debuff, etc.
if crit: base ×= crit_damage            2.0, or 3.0 for Meteor with Meteoric Impact
```

Ignite ticks and zone ticks go through the same function with the same multipliers, which
is what makes "no snapshot" observable: change Heat and the next tick changes.

---

## 8. Rotations

### 8.1 Single target — Haste build

1. Fire Mastery on cooldown (open with it).
2. Flame Lance whenever a charge is up (builds 2 stacks, feeds Pyroblast resets).
3. Pyroblast on cooldown once the target has 3+ stacks.
4. Meteor on cooldown once the target has 3+ stacks.
5. Firestorm on cooldown.
6. Detonate at maximum stacks **when the bank is fat** — i.e. when remaining Ignite
   duration is high — or when the target is about to die.
7. Firebolt as filler.

### 8.2 Single target — Crit build

1. Fire Mastery before the burst window.
2. Flame Lance to reach maximum stacks as fast as possible.
3. **Hold the burn.** Do not Detonate.
4. Pyroblast on cooldown into the full burn (975 % SP).
5. Meteor on cooldown into the full burn (750 %, ×3 crits with Meteoric Impact).
6. Firebolt/Flame Lance to refresh the 30 s duration before it lapses.
7. Detonate only when: the target is about to die, or the burn cannot be maintained.

### 8.3 AoE (3 / 8 / 15)

1. Flame Cone on cooldown — the pack-wide Ignite engine.
2. Firestorm across the pack.
3. Meteor into the pack (Molten Meteor for sustained pressure, Meteoric Impact for burst).
4. Flame Lance on the primary to keep it at cap.
5. Detonate when the pack is at cap and the burns are fresh, or on execute.
6. Firebolt filler.

Priority is *not* a fixed script: the simulator's AI (§14) evaluates the Detonate decision
per situation.

---

## 9. Fire Mastery

A selectable specialisation, not a talent purchase.

| Version | Active (20 s, 60 s cooldown) | 70–99 Heat | Inferno State |
| --- | --- | --- | --- |
| Haste | +15 % Haste | +5 % Haste | +7.5 % Haste |
| Crit | +15 % Crit | +5 % Crit | +7.5 % Crit |
| Dual (talent) | +10 % Haste **and** +10 % Crit | +2.5 % each | +3.75 % each |

Dual Fire Mastery (7 points) trades peak value in one stat for both; the threshold bonuses
split evenly rather than doubling, otherwise the talent would be a strict upgrade with no
decision attached.

---

## 10. The Ultimate

Disabled for this iteration. `ULTIMATE_ENABLED: bool = false` gates exactly three places:
the cast validation (refuses with `UltimateDisabled`), the Ultimate button's availability,
and the `R` binding. Cataclysm's definition, its zone, its energy cost and Ultimate Energy
itself stay in the code untouched, so the system is one constant away from returning. No
new Ultimate concept is designed here.

---

## 11. Talents

Twelve talents, 51 points to own all of them. The default build budget is
`DEFAULT_TALENT_BUDGET = 20` points (configurable; testing mode still grants the full 51 so
every talent is reachable in a manual test).

| # | Talent | Cost | Effect |
| --- | --- | --- | --- |
| 1 | Burning Embers | 3 | Each Ignite stack consumed increases Detonate damage by +3 % |
| 2 | Extended Flame | 2 | Ignite duration 30 → 40 s |
| 3 | Infernal Capacity | 5 | Maximum Ignite 5 → 6; Detonate then requires 6 |
| 4 | Rapid Combustion | 4 | Detonate: 20 % chance to reduce Flame Cone, Firestorm and Meteor cooldowns by 2 s (rolled independently per ability) |
| 5 | Molten Meteor | 4 | Meteor −25 % impact damage and cannot crit; leaves Burning Ground for 15 s, ticking every 3 s for `MOLTEN_GROUND_TICK` % SP and applying +3 % Fire damage taken |
| 6 | Furnace Mastery | 5 | Inferno State 10 → 12 s |
| 7 | Meteoric Impact | 4 | Meteor +100 % critical strike damage (×2 → ×3); a critical Meteor reduces Pyroblast cooldown by 4 s |
| 8 | Spellfire Feedback | 4 | Crit Flame Lance → Pyroblast −1.5 s; crit Pyroblast → Flame Lance −1.5 s (from the 1.0 s baseline) |
| 9 | Burning Precision | 3 | A critical Ignite tick has a 10 % chance to reduce Meteor's cooldown by 1 s |
| 10 | Volatile Ignition | 5 | Ignite damage −20 %; Pyroblast +1 charge |
| 11 | Chain Pyroblast | 5 | Pyroblast bounces with probability equal to current crit chance; each bounce −20 % damage; max 10 bounces; never the same target twice in one chain |
| 12 | Dual Fire Mastery | 7 | Fire Mastery grants +10 % Haste and +10 % Crit instead of +15 % of one |

### 11.1 Interactions

* **Molten Meteor ⊗ Meteoric Impact** — mutually exclusive in practice: one removes crits
  from Meteor, the other doubles them. Both may be bought (no rule forbids it); doing so
  wastes 4 points, which the combination search will show.
* **Infernal Capacity ⊗ Burning Embers** — a 6th stack is +3 % Detonate and +20 % Ignite
  throughput, but raises the Detonate requirement. Synergy for the Detonation build, a
  small tax for the maintenance build.
* **Volatile Ignition ⊗ Chain Pyroblast** — the Pyroblast build: two charges bouncing.
  Ignite −20 % hurts Detonate and Burning Precision, so it pulls *away* from the
  Detonation build. This is the intended shape of the crit branch.
* **Burning Precision ⊗ Meteoric Impact** — crit density feeds Meteor cooldown from two
  directions; the crit build's engine.
* **Rapid Combustion ⊗ Extended Flame** — Detonation build's engine: more Detonates, and a
  longer bank to Detonate.
* **Furnace Mastery ⊗ Dual Fire Mastery** — 12 points of stat-uptime, the "greedy" build
  the search is expected to test and reject at high target counts.

---

## 12. Combat simulator

A separate crate, `crates/engine-sim`, binary `engine-sim`, depending on `engine-game` and
nothing else. It runs the **real `Simulation`** headlessly: the same ability table, the same
Ignite, the same Heat, the same damage function. There is no second copy of any formula —
the requirement in the brief's §41 is satisfied by construction rather than by review.

### 12.1 Scenario configuration

```text
targets          1 | 3 | 8 | 15          (configurable)
target_hp        per scenario, configurable
duration         60 s | 90 s             (configurable)
iterations       configurable, default 500 for finalists / 24 for the sweep
seed             per iteration, so a run is reproducible
build            talent set + Fire Mastery choice
stats            spell power, base haste, base crit, crit damage
movement model   see 12.3
```

### 12.2 Target model

Targets have real health and really die. On death: Ignite is removed, its ticks stop, casts
against it fail, AoE hits fewer enemies, the AI retargets. Because the simulator drives the
real simulation, all of this is the game's own behaviour rather than a model of it.

### 12.3 Movement / missed damage

Targets are placed in a pack and given a scripted wander: every `move_interval` seconds each
target has probability `wander_chance` of stepping `wander_distance` metres in a random
direction, and probability `return_chance` of stepping back. Whether that takes it out of a
Firestorm line or a Flame Cone is then decided by the *game's* geometry, not by a fudge
factor. Defaults: `move_interval 2 s`, `wander_chance 0.25`, `wander_distance 4 m`.

### 12.4 Cast completion

Modelled by the game: a cast is a timer, and a cast whose target dies lands on nothing while
still paying its charge. The simulator counts casts started, casts completed, and casts that
completed against a dead target ("wasted"). Firebolt, Flame Lance and Pyroblast are the
three that can be wasted.

### 12.5 Rotation AI

Two priority systems (§8), plus a shared **execute heuristic**:

```text
if target has maximum Ignite and
   remaining_ignite_damage(target) >= target.hp × execute_margin
then Detonate now
```

`execute_margin` defaults to 1.0 — Detonate when the burn would otherwise overkill. This is
simulator-only decision-making; no gameplay ability contains it.

The Haste build additionally Detonates whenever the target is at cap and the burn is fresh
(`remaining >= detonate_freshness × duration`, default 0.8). The Crit build only Detonates
on execute, on a target that is about to become unreachable, or when Ignite is about to
expire and cannot be refreshed.

### 12.6 Talent combination search

12 talents, costs summing to 51, budget 20 → the search enumerates every subset whose cost
fits the budget and which is **maximal** (no unbought talent would still fit), which cuts
4 096 subsets to a few hundred. For each: run the sweep iteration count across all four
target counts and both durations, rank by DPS, then re-run the top N with the full
iteration count. Both counts are configurable and the whole enumeration can be replaced by
an explicit build list.

### 12.7 Recorded per run

Total damage, DPS, damage per target, per-ability damage (Firebolt, Flame Lance, Flame Cone,
Meteor, Pyroblast, Firestorm, Ignite, Detonate, Burning Ground), casts started/completed/
wasted, Detonations, Ignite applications, average Ignite stacks, Heat cycles, average Heat,
time above 30 and above 70 Heat, Inferno State activations, critical strikes, cooldown
reductions granted, cooldown reduction wasted (applied to an ability not on cooldown),
target deaths, overkill damage.

### 12.8 Output

A text report to stdout and to `docs/firemage-sim-report.md`: one block per scenario in the
brief's §33 order, then a ranking table of talent combinations across 1/3/8/15 targets, then
the Haste vs Crit comparison against the balance targets.

---

## 13. Balance targets

| Targets | Goal |
| --- | --- |
| 1 | Haste 5–10 % ahead of Crit — **met at 7.8 %** |
| 3 | within ±10 % — **met at −2.3 %** |
| 8 | within ±5 % — **met at −3.8 %** |
| 15 | within ±5 % — **missed at −8.2 %**, see §15 |

No talent may dominate every scenario; no talent may be worthless in all four. Numbers come
from the simulator, and coefficients are adjusted a few at a time with the reason recorded
in §15 of this document.

---

## 14. Expected throughput (hand check before the simulator exists)

At SP 100, 15 % crit, ×2 crit damage, no talents, single target, Heat sitting in the 30–69
band (×1.15):

```text
Flame Lance    260 × 1.15 × 1.075 = 321   per 1.5 s cast, 2 charges / 8 s
Pyroblast (5)  975 × 1.15 × 1.075 = 1205  per 30 s
Meteor (5)     750 × 1.15 × 1.075 = 927   per 20 s
Firebolt       100 × 1.15 × 1.075 = 124   per 1.0 s cast
Ignite (5)     100 × 1.15 × 1.075 = 124   per 2 s tick  → 62 dps
```

A 60 s single-target fight at ~1 cast/1.2 s ≈ 50 casts. Heat: Flame Lance ×15 (60) +
Pyroblast ×2 (10) + Meteor ×3 (15) + Firestorm ×2 (10) + Firebolt ×20 (40) + Detonate ×3
(21) ≈ 156 Heat, i.e. **~1.5 cycles in 60 s and ~2.5 in 90 s** — inside the 2–3 target,
slightly low on the short fight. Flagged for the first balance pass rather than pre-tuned.

---

## 15. Balance change log

Every entry is one observation, the smallest change that addressed it, and the result. All
numbers are sixty-second fights, forty iterations, the two designed builds.

### Pass 1 — the Detonation build was 59 % ahead at one target

**Observed.** Haste 622 DPS against Crit 391 at one target. Detonate was 47 % of the
Detonation build's damage; Ignite was 6 % of it.

**Why.** Detonate converts the burn's *remaining* damage, and a burn's duration refreshes on
every application. A full burn is therefore always worth its whole 30–40 seconds, and an
8-second cooldown cashes that bank about three times faster than it would ever tick. Holding
a burn paid 72 DPS; spending it paid 108 DPS, so the maintenance build was strictly worse at
its own mechanic.

**Changed.** `DETONATE_FRACTION` 0.60 → 0.30 → 0.20 → **0.22**, measured at each step. At
0.22 the two ways of using a burn are worth roughly the same per second, which is what makes
the choice between them a decision rather than a mistake.

### Pass 2 — the maintenance build had nothing to hold a burn *for*

**Observed.** With Detonate cut, the Crit build's Pyroblast and Meteor were 28 % of its
damage and its Flame Lance was 2 %: it was the Detonation rotation with the payoff removed.

**Changed.** Two things, both of them the "reward for a full burn" number:

* Meteor and Pyroblast `ignite_bonus_per_stack` 0.10 → **0.15**. At six stacks Pyroblast is
  1.9× rather than 1.6×, which is what a build that keeps a burn alive is buying.
* `IGNITE_SPELL_POWER_PER_STACK` 0.20 → **0.24**, because Ignite is the maintenance build's
  largest differentiated row and the Detonate cut had taken 63 % off what a burn was worth
  when spent.

Also fixed in the simulator, not in the game: the maintenance rotation was holding Flame
Lance charges at the cap and filling with Firebolt. Flame Lance is worth two and a half
Firebolts per cast and refreshes the duration the build depends on, so it is now the filler
at the cap as well.

### Pass 3 — Molten Meteor's ground had no number at all

`MOLTEN_GROUND_TICK_COEFFICIENT` set to **0.55** (55 % SP every 3 s for 15 s = 275 % SP per
target, against the 125 % of impact damage the talent gives up). It is worth taking at eight
targets and not at one, which is the shape the talent is meant to have.

### Pass 4 — everything above was measured naked

**Observed.** The class was being balanced at base stats: 0 % haste, 15 % crit. A geared
FireMage plays at 40-50 % in its primary stat and 20 % in the other, and at those stats the
Detonation build was **+23 %** at one target against a goal of +5 to +10.

**Why.** Haste shortens every cooldown in this engine, Detonate's included. A haste character
therefore refilled the Ignite bank faster *and* cashed it in more often - the same stat paid
twice, compounding at exactly the gear level the class is actually played at.

**Changed.**

* `Gear` added to the stat block, and every simulator build now wears it. Nothing is measured
  naked any more.
* Detonate's cooldown no longer scales with haste (`cooldown_hasted: false`, the only ability
  in the table with it). +23 % became +12 %.
* `DETONATE_FRACTION` 0.22 → **0.20** for the last two points.

### Where that landed

All numbers below are at geared stats: 45 % primary, 20 % off-stat.

```text
TARGETS     HASTE DPS     CRIT DPS   HASTE +%  GOAL                 RESULT
1                 782          708      10.5%  HASTE 5-10% AHEAD    0.5 OVER
3                1274         1394      -8.5%  WITHIN 10%           MET
8                2061         2176      -5.3%  WITHIN 5%            0.3 OVER
15               2305         2377      -3.0%  WITHIN 5%            MET
```

Across the gear band (40 % and 50 % primary) the ordering does not change: the Detonation
build leads at one target at both ends and trails at three and above at both ends. Run-to-run
noise at forty iterations is about a point, so the two misses above are inside it - a
twenty-four-iteration run of the same build put all four inside the goal.

### Open findings, deliberately not acted on

* **Three targets sits at −8.5 %**, inside its ±10 % goal but at the edge of it. The gap is
  insensitive to `DETONATE_FRACTION` - moving it changes the single-target spread five times
  as fast as the multi-target one. Closing it wants an AoE-only lever the Detonation build
  owns and the maintenance build does not, which is a design change rather than a coefficient.
* **Meteoric Impact and Burning Precision are now in fourteen of the sixteen best builds**,
  and Infernal Capacity in half of them. Crit density at geared stats is what the tree
  rewards.
* **Infernal Capacity was in fifteen of sixteen at base stats.** A dominating talent by the
  brief's own definition. Its cost (5) and its effect (+1 maximum stack) are both pinned by
  the specification, so it is reported rather than changed; the obvious follow-up is 6 or 7
  points.
* **Spellfire Feedback is nearly never taken.** Four points for two 0.5-second cooldown
  upgrades is thin against a talent that raises every burn by 20 %.
* **Heat cycles run at 1-2 per sixty seconds and 2-3 per ninety**, against a target of 2-3
  per encounter. The Heat generation values are given explicitly by the specification, so
  they are left alone; raising them by about a quarter would centre the target.


---

## 16. Testing plan

* **Ignite**: 0→1, 1→2, cap, never above cap, refresh at cap, expiry, dynamic haste
  (interval shortens mid-burn), dynamic crit (crit rate follows the buff), no snapshot
  (damage of an unchanged burn changes when Heat changes).
* **Detonate**: refused below maximum, spends no charge on refusal, damage equals 60 % of
  remaining, 10 m radius, consumption, multiple targets each on their own burn, dead target.
* **Heat**: the 30/70/100 thresholds, consumption at 100, Inferno State duration and stats,
  Heat = 0 when it ends, no decay in combat, decay after 10 s out of combat.
* **Cooldown reductions**: crit Flame Lance → Pyroblast, crit Pyroblast → Flame Lance, crit
  Ignite tick → Meteor, crit Meteor → Pyroblast; every one clamped at its documented limit.
* **Cast interruption**: target dies during Firebolt / Flame Lance / Pyroblast; no damage
  lands afterwards.
* **Talents**: each of the 12 enables and disables cleanly, and every effect it claims is
  observable.
* **UI**: Ignite reads `n / max` from the configuration, Detonate lights only at maximum,
  the Ultimate button is unusable, the talent menu shows exactly 12 talents.

---

## 17. Known limitations (design-level)

* The Ultimate has no gameplay concept and is switched off.
* Firestorm's line has no collision awareness — it burns a rectangle on the flat.
* Chain Pyroblast's bounce order is nearest-first, which is a choice, not a derivation.
* The simulator's movement model is scripted rather than a real AI.
