# The Warrior

A tank whose heavy abilities build a resource, **Valor**, that turns into a lasting buff,
**Battle Fervor**.

```text
War Stomp / Colossus Strike / Shield Charge       (one Valor per successful cast)
        │
        ▼
   Valor  ○ ○ ○ ○ ○            (four pips with Unbroken Momentum)
        │  fifth pip
        ▼
   Valor back to 0, and +1 stack of Battle Fervor
   (30 s, up to 3 stacks: +10 % damage dealt, −3 % damage taken each)
```

Defending no longer feeds a resource. Block, Parry and Dodge still matter — Battle Trance and
Spell Reflect are fed by them — but nothing banks.

---

## What was reused, and what is new

Nothing in this class is a second copy of a system that existed.

| Mechanic | Where it lives |
| --- | --- |
| The twelve abilities | The Warrior's rows, `engine-game/src/warrior/abilities.rs`, assembled into the one table `ability::ABILITIES` (`engine-game/src/ability/`) |
| Cooldowns, charges, the global cooldown | `AbilityBook`, unchanged; the global cooldown is 1.5 s for every class (`firemage::GLOBAL_COOLDOWN`, shortened by haste) |
| Casting, targeting, line of sight, facing | `sim/firemage_exec.rs`, unchanged |
| Damage, healing, death, events, the meter | `Simulation::deal_damage`, `GameWorld::heal`, `CombatEvent` |
| Threat | `enemy::threat::ThreatTable`, fed from damage, in one place |
| Interrupt | `Simulation::interrupt`, the existing rules about what may be stopped |
| Damage over time, stacking buffs and debuffs | `status.rs`, extended with stacks, an interval and a health coefficient |
| Movement | `Simulation::slide_player` through the collision world |
| Stat scaling | `character::stat::StatBlock` and `firemage::Stats` |
| VFX and audio | The existing catalogues, borrowed by shape. **No new assets.** |

New, because there was nothing to reuse:

* `character/defense.rs` — Dodge, Parry, Block and Armour, and the single roll between them.
  Not Warrior-specific: any character with the stats gets it.
* `warrior/valor.rs` — the resource.
* `warrior/mod.rs` — the class's timers, absorb, banked pool and charge.
* `sim/warrior_exec.rs` — the executor, the mirror of `firemage_exec.rs`.
* Stun, silence and an attack-speed scale on `EnemyBrain`, because control is a property of
  the mind being controlled.

### Eleven abilities on a ten-key bar

`ABILITY_SLOTS` is ten and stays ten: renumbering the keyboard for one class would change
what every other hero's `4` does. The eleventh ability is the hero's **Ultimate** — the slot
beside the bar on `R` that already existed for the FireMage's Moving Fire Beam. The
Warrior's is Last Stand, and `FireAbility::is_ultimate` now asks the owning hero rather than
naming one ability.

---

## Stats

Four stats were added to the one stat table, as a `Defensive` group.

| Stat | Kind | What it does |
| --- | --- | --- |
| `Armor` | flat | `armour / (armour + 1000)` off physical damage, clamped to 75 % |
| `Block` | rating | Chance to block: the attack lands for 30 % less |
| `Parry` | rating | Chance to parry: none of it lands |
| `Dodge` | rating | Chance to dodge: none of it lands |

Ratings convert through the one existing exchange rate (`RATING_PER_PERCENT`). Armour is the
only stat in the game that is neither primary nor a rating: it is flat and converts through a
curve, so `Stat::is_rating` answers false for it and the sheet prints it as a number.

The Warrior is the **one** hero whose base stat block differs from the others': 500 armour and
3 % each of Block, Parry and Dodge. A tank whose defences cannot fire without gear cannot
be played in the gear it starts in.

### The roll

One roll per attack, in `DefenseProfile::roll`, in the order **dodge → parry → block → hit**,
against one random number. There is no such thing as a parried block, so there is exactly one
outcome per attack — which is why a duplicated combat event structurally cannot pay a
defensive event (a Battle Trance heal, a Spell Reflect cooldown tick) twice. Dodge, Parry and Block sum to at most 80 % (`AVOIDANCE_CAP`), scaled together so the
three keep their proportions.

**Physical only.** Magic is answered by Spell Reflect, by armour-independent flat reductions
and by the absorb, which is why the class wants both kinds of mitigation.

### The incoming pipeline

`Simulation::deal_damage` → `defend_player`, once, in this order:

```text
Spell Reflect  →  roll  →  block share  →  armour (physical)  →  flat reduction
               →  absorb (Puncture Blood)  →  Adrenaline banks its share  →  health
```

---

## Valor

| | |
| --- | --- |
| Range | `0..=5` whole stacks (`tuning::VALOR_MAX`) |
| Generation | **+1** when War Stomp, Colossus Strike or Shield Charge succeeds |
| Nothing else pays | Block, Parry, Dodge, every other ability: no Valor |
| Decay | **None.** There is no clock; what is banked stays banked |
| At 5 | The bar empties on the same call and one stack of Battle Fervor is applied |

"Succeeds" means: War Stomp and Colossus Strike land and hit at least one enemy; Shield Charge
arrives and its impact hits something. **One gain per cast**, however many enemies it hit. A
smash into empty air costs its cooldown and pays nothing. Which abilities pay is a column of
the ability table (`WarriorRow::valor`), so the tooltip's `Generates 1 Valor` line and the
executor read one number. A `ValorGained` event is emitted per gain (`filled` on the fifth).

A new run starts with 0 Valor and no Fervor.

## Battle Fervor

A normal status (`StatusKind::BattleFervor`), applied to the Warrior only.

| | |
| --- | --- |
| Duration | 30 s (`FERVOR_DURATION`); gaining a stack refreshes the whole status |
| Stacks | Up to 3 (`FERVOR_MAX_STACKS`); at 3 a further fill still refreshes the clock |
| Per stack | +10 % damage dealt (`FERVOR_DAMAGE_PER_STACK`) |
| Per stack | −3 % damage taken, every damage type (`FERVOR_DAMAGE_TAKEN_PER_STACK`) |
| Per stack | +X % maximum health (`FERVOR_MAX_HEALTH_PER_STACK`), **0 by default**; set it to `0.01` for +1 % |

The numbers live in `StatusDef`'s modifiers, so they go through the same evaluators every
effect uses (`StatusSet::damage_dealt_multiplier`, `damage_taken_multiplier`, and the new
`max_health_multiplier`). The maximum-health share is folded into the Warrior's health
multiplier and keeps current health (`Health::set_max_keeping_current`). It ends by expiring,
or with the Warrior's death like any other buff, and it is synced and shown like any status:
the player's buff bar prints its stacks and remaining time.

## The abilities

Cooldowns, durations, ranges, radii and coefficients are rows of `ABILITIES`. Magnitudes are
named constants in `warrior::tuning`.

| # | Key | Ability | CD | What it does | Valor |
| --- | --- | --- | --- | --- | --- |
| 1 | `1` | **Recuperate** | — | 5 m swing, stacks a self-heal to 5 | — |
| 2 | `2` | **War Stomp** | 4 s | 120° cleave at the target, −2 % attack speed a stack to 8 | +1 |
| 3 | `3` | **Colossus Strike** | 8 s | 8 m smash, +1 % max HP, 8 s bleed | +1 |
| 4 | `4` | **Shield Charge** | 15 s | runs to the target, area hit, 0.5 s stun | +1 on arrival |
| 5 | `5` | **Shield Bash** | 15 s | damage and interrupt | — |
| 6 | `6` | **Shield Block** | 15 s | 6 s of extra Block, no critical strikes taken | — |
| 7 | `7` | **Puncture Blood** | 30 s | −30 % current HP, +30 % max HP absorb for 10 s | — |
| 8 | `8` | **Adrenaline** | 30 s | banks 30 % of damage for 6 s, heals it over 6 s | — |
| 9 | `9` | **Battle Trance** | 45 s | 10 s: every defensive event heals 1 % max HP | — |
| 10 | `0` | **Spell Reflect** | 45 s | 6 s: enemy spells go back to their caster | — |
| 11 | `R` | **Last Stand** | 90 s | +max HP, a heal for the same share, −10 % damage taken | — |

There is no empowered form of any ability.

### Health scaling: one formula, two abilities

Shield Block's Block chance and Last Stand's maximum health are both:

```text
value = base + scale × (scaling_health / 500 − 1)
```

`scaling_health` is the **sheet's** pool: Stamina and gear, without the class's own
multipliers. Scaling from the live maximum would make Shield Block drift every time an attack
was blocked, and would make Last Stand a bonus derived from a number it had just increased.

| | base | scale | no gear | at 1.6× health |
| --- | --- | --- | --- | --- |
| Shield Block | 6 % | 30 % | +6 % | **+24 %** |
| Last Stand | 6 % | 30 % | +6 % | **+24 %** |

The design's "fully geared +24 %" is a *consequence* of two tunable constants and the geared
health anchor (`GEARED_HEALTH_MULTIPLE`), not a second hard-coded number.

### Colossus Strike's bleed does not snapshot

`Status` carries **coefficients**, never a number of points:

* `damage_per_tick` — the flat half, decided when it is applied because it is a share of
  attack power.
* `source_health_per_tick` — a *fraction* of the caster's maximum health, resolved by the
  simulation on the tick the effect happens.

There is structurally nowhere in the status for a stale maximum health to live. A Last Stand
cast after the bleed lands makes the rest of that bleed hit harder; when it expires, the ticks
that follow are smaller again.

The bleed is tuned to about **75 %** of the ability's total damage: four ticks over eight
seconds at `0.75 ×` the impact each.

### The bleed lands on three enemies at most

The smash itself still cleaves everything within 8 m. The **bleed** only takes hold on
`COLOSSUS_BLEED_MAX_TARGETS` (3) of them: the selected target first, then the nearest to
the Warrior (ties broken by the order of the hit list, so the choice never depends on a
sort). Without the cap the bleed, three quarters of the ability's damage, was paid once per
enemy in reach: Colossus Strike was 72 % of the Warrior's damage in an eight-target fight
and the class dealt 1744 DPS at top gear against the Mage's 1823, with the Druid's 620 on
top. Against a single target or three the cap changes nothing.

| Eight targets, `engine-sim --iterations 20` | none | medium | top |
|---|---:|---:|---:|
| Warrior DPS before / after | 1097 / 761 | 1402 / 976 | 1744 / 1232 |
| Mage Default / (Warrior + Druid) before / after | 0.81 / 1.05 | 0.80 / 1.04 | 0.77 / 0.98 |

Single-target numbers are unchanged (208 / 264 / 329). The cap is the only lever pulled; it
is one constant in `warrior::tuning`.

### Battle Trance's rate limit

Three healing activations per second, counted in whole seconds measured from the moment the
buff started — not a sliding window, because a window would make the answer depend on the
order two attacks landed within one tick. There is **no cap on the total**: ten seconds of
three a second is thirty activations, and nothing stops it earlier.

### Puncture Blood reads two different numbers

The cost is a share of **current** health; the shield is a share of **maximum** health.

```text
10,000 max, 8,000 current  →  lose 2,400, gain 3,000 absorb
```

The cost is applied straight to the health rather than through the incoming pipeline: it is
the Warrior cutting themselves, not an attack, and routing it through the pipeline would let
their own armour reduce it, their own shield absorb it and their own Block roll it away.

### Adrenaline

Banks a share of what **actually lands** — after avoidance, armour, block and the absorb — so
a mitigated hit banks what it really cost. When the buff ends the pool becomes a `Recovering`
status: the whole of it, healed evenly over six seconds.

### Spell Reflect

Reflectable, from the classification the enemy table already carries: an enemy ability **with
a cast bar**, whose damage type is **not physical**, whose effect is plain damage. A swing is
not a spell; burning ground is not something to hand back; a heal is not reflected. Friendly
sources never reach the check — the caller has already established the attacker is hostile.

Reflected damage is credited to Spell Reflect, so the meter shows the button the player
pressed. While the ability is on cooldown, every successful defensive event takes 0.1 s off it
— once per attack, because there is one outcome per attack.

### Shield Charge is a run, not a Blink

`Blink` and the old `Charge` used to be the same `dash`, which slid the body and cleared the
interpolation history. Shield Charge is a `ChargeMotion` the tick loop steers: the body takes
a real collided step towards the target at 22 m/s, follows a target that moves, stops at a
wall, and gives up after three seconds. Its area hit, its stun and its threat all happen when
it arrives.

### War Stomp

Targeted, not aimed: the player picks an enemy and the game takes a 120-degree cone
(`TARGET_CLEAVE_HALF_ANGLE`) around the line to it. `TargetMode::TargetCone` is what makes
that a property of the row rather than of the executor.

Stacks are a `Sundered` status on the enemy. Every tick the simulation reads the stacks back
out and pushes them into the enemy's mind as an attack-speed scale, and sets the Warrior's
armour bonus from whether **anything** currently carries a full eight. Recomputed rather than
counted as it is applied, so an expired stack and a dead enemy both take the armour with them
on the tick it becomes true.

---

## Talents

Eight talents, bought with talent points (tiers 1-4 cost 3 / 4 / 4 / 5, as the Druid's).
The rows and every number live in `assets/catalog/talents.json`;
`engine-game/src/talent/warrior.rs` is the behaviour: `WarriorTalentEffects`, one field per
number the executor reads, neutral when nothing is bought and swapped per player like the
Druid's. No call site asks whether a talent is owned; the descriptions on the talent menu
are templates filled from the same numbers.

| Talent (tier, cost) | Effect |
|---|---|
| Unbroken Momentum (1, 3) | Battle Fervor needs 4 Valor, not 5 (the Valor bar is four pips long). |
| Shockwave (1, 3) | War Stomp's reach +40 % (both how far a target may be and the cone), damage +30 % and one extra Sundered stack; its cooldown +1 s. |
| Executioner (2, 4) | Colossus Strike deals +50 % to a target under 35 % health, on the impact and on every bleed tick (read when the tick lands); its cooldown +1 s. |
| Bloodied Valor (2, 4) | The last tick of a Colossus Strike bleed that ran its whole course pays +1 Valor. Once per tick however many bleeds end together; a bleed cut short (target dead, dispelled) pays nothing. |
| Fervent Guard (3, 4) | Battle Fervor per stack is +6 % damage dealt and -6 % damage taken, instead of +10 % / -3 %. |
| Relentless Charge (3, 4) | Shield Charge holds two charges; its impact damage -10 %. |
| Bulwark Rhythm (4, 5) | Every Battle Fervor stack gained (each fill of the bar, including a refresh at three stacks, and Unyielding's) takes 5 s off Shield Block's remaining cooldown. |
| Unyielding (4, 5) | Last Stand immediately grants 2 stacks of Battle Fervor; its cooldown +30 s. |

Where the numbers land:

* **Valor and cooldowns.** The Valor bar's length lives on `Valor` itself. Extra charges and
  extra cooldown seconds (Relentless Charge, Executioner, Unyielding) are an adjustment held
  by the `AbilityBook` beside the FireMage's talent effects, so every cooldown reader
  (the bar, the simulator) sees them; the Warrior's executor sets them whenever the grid
  changes.
* **Fervent Guard** (its aura tooltip prints the talent's numbers) does not touch the status registry's Battle Fervor row (which states the
  untalented 10 % / 3 %). `WarriorTalentEffects` gives the ratio of the two sets of numbers
  for the stacks held, and the two places damage multipliers are read (the damage pipeline
  and the effect readout) apply it for the Warrior only.
* **Shockwave's reach** is `Simulation::ability_range`, read by the cast check, the cone and
  the interface's in-range test.

`crates/engine-game/tests/warrior_talents.rs` plays each scene with and without the talent.

---

## Threat

No new system. Threat is still damage, added in the one place damage lands, into the one
`ThreatTable`. What is new is a column: `WarriorRow::threat_multiplier`, one for everything
that is not deliberately a threat tool.

| Ability | ×threat |
| --- | --- |
| Shield Charge | 4 |
| Colossus Strike | 3 |
| War Stomp | 3 |
| Shield Bash | 3 |
| everything else | 1 |

---

## The three builds

**Health.** Stamina improves Shield Block's Block chance, Last Stand's bonus and its heal,
Battle Trance's healing, Colossus Strike's impact *and* its bleed on every tick, Recuperate,
and Puncture Blood's absorb. Not everything scales from health: Shield Charge, Shield Bash and
War Stomp scale from the main stat, and Adrenaline scales from what is hitting you.

**Valor.** Cooldown reduction and haste mean more casts of the three Valor abilities, so a
Battle Fervor stack sooner. Block, Parry and Dodge buy *frequency* of Battle Trance heals and
Spell Reflect cooldown ticks; they do not make any single payout larger, which is what keeps
the builds from being one.

**Hybrid.** Good health, good avoidance, steady generation. Deliberately not better than
either specialist: the health build's payouts are larger and the avoidance build's are more
frequent, and the hybrid gets the middle of both.

---

## The interface

The player frame swaps its resource rows by hero: Heat and Energy for the FireMage, Valor
for the Warrior.

* **The Valor pips**: five segments, lit left to right, labelled `n/5`.
* **Battle Fervor** is an ordinary buff on the buff bar, with its stack count and remaining
  time (`Aura::stacks`, `write_remaining`) and the `battle_fervor` icon.
* **What is running**: `BLOCK`, `TRANCE`, `STAND`, `REFLECT`, `CHARGE`, `STOMPED`, the
  Recovery Pool while Adrenaline banks, and the absorb while it stands.
* **Tooltips** of Valor abilities have a *Resource* section reading `Generates 1 Valor`. The
  Warrior has no *Empowered Effect* section. Both are generated: see [Ability text](#ability-text).
* **Taunt** is `F`. The key issues Taunt's own command (`Hero::command_for`): it used to open the
  Ultimate's placement circle, and the click after it cast Last Stand. Taunt reaches **30 m**,
  needs line of sight, and turns the Warrior to the target rather than refusing for facing
  (`AbilityDef::turns_to_target`). Out of range and out of sight are refused with the usual
  message and cost nothing. `tests/taunt.rs` covers all of it.
* **Nameplates** count stacking debuffs, so War Stomp's eighth stack is readable from across
  the room.
* The **character sheet** has a `DEFENSIVE` section: Armour flat, the other three as
  percentages.

## Ability text

Descriptions are templates in the rows, filled from the table and the tuning constants by
`ability::text` — `{duration}`, `{range}`, `{heal_per_stack}`, … — so a balance pass moves the
sentence with the number. Each class answers its own tokens (`warrior::abilities::token`).
House style: second person, sentence case, `sec` / `m` / `%`, resource and stat names
capitalised. `AbilityDef::resource_uses()` derives Cost / Empower / Generate from the columns, so the
tooltip and the executor cannot disagree. A unit test holds every token to resolving and every sentence to sentence case.

> Recuperate: *Strike your target and gain a stack of Recuperate, restoring 0.2% of your maximum
> Health every second for 10 sec. Stacks up to 5 times.*

## Feedback

Every cue is an existing asset chosen by shape, which is the same rule the Warrior's and the
Druid's ability sounds already followed. Blocks are a heavy impact; parries and dodges are the
fizzle that already means "that did not happen"; an absorb is the Flame Shield shell; a
reflected spell is an explosion where it turned round. The fifth Valor, which becomes Battle
Fervor, plays the cooldown-activation cue and a gold burst; the four before it are silent.

---

## Known limitations and deliberate omissions

* **Gear and progression.** Talents exist (above), but nothing grants
  Armour, Block, Parry or Dodge yet except the Warrior's own base block, so a "geared" Warrior
  is currently only reachable through a scripted stat grant.
* **Adrenaline's cooldown is an assumption.** The design did not state one; 30 s is in the
  table and is tunable like every other cooldown.
* **Last Stand's duration is an assumption.** The design did not state one; 15 s is in the
  table.
* **Nothing rolls a critical strike against the player**, so Shield Block's critical immunity
  is a guard on a future mechanic rather than a live one. It is carried on the defender rather
  than as a roll the attacker skips, so turning enemy critical strikes on needs no change here.
* **The Warrior has no auto-attack.** Recuperate is the filler, as designed.
* **Enemies have no defences.** `DefenseProfile::from_stats` works for anybody; nothing in the
  roster has the stats.
