# FireMage

The first playable class: ten abilities, a channelled Ultimate, a burn that banks damage, an
explosion the player decides when to spend, and twelve talents that decide which of those two
things the class is for.

Design source of truth: `docs/firemage-design.md`. Simulator output: `docs/firemage-sim-report.md`.

## The three rules the class is built on

**Reaching maximum Ignite does nothing.** No automatic combustion, no hidden timer, no
"primed" state. The payoff belongs to the player, who spends it with Detonate when it is
worth spending, and the ability refuses until the burn is full.

**Nothing hardcodes five.** The maximum is `TalentEffects::ignite_max_stacks`, read where it
is needed. Infernal Capacity moves the cap, the Detonate requirement, the stack pips, the
`n / max` readout and the tooltip together, because they all ask the same field.

**Nothing snapshots a stat.** `FireMage::stats()` is derived on every call from Heat's band,
Fire Mastery and the talents. A burn stores stacks, seconds left and how far through its tick
it is *as a fraction* — there is nowhere for a cached haste or crit chance to live. A burn
applied without haste ticks faster the instant the mage has some, mid-burn, with no
reapplication.

## Architecture

```text
engine-game/src/firemage/
    ability   the table: cast, cooldown, charges, range, area, heat, spell-power coefficients
    ignite    stacks, refresh, consumption, remaining worth. Reports ticks; deals no damage
    resource  Heat with its three bands and the Inferno State; Ultimate Energy
    stats     the derived stat block, and the rule that nothing stores one
    talent    data -> TalentGrid (what is bought) -> TalentEffects (what it means)
    zone      ground and line fire, and the impacts still falling towards it
    rng       seeded, part of simulation state, so a fight replays exactly

engine-game/src/sim/firemage_exec.rs
    validation, target collection, crit rolls, damage, Ignite, Detonate, Heat, cooldown resets

engine-sim/
    the combat simulator: scenarios, a rotation AI, talent combination search, the report

engine-presentation/src/
    snapshot.rs           what the UI is allowed to know, rebuilt every frame
    game_ui.rs            ability bar, Heat bands, Ignite readout, tooltips
    ui/targeting.rs       one ground-targeting system, now with a line for Firestorm
    ui/talent_menu.rs     the twelve talents, by tier
```

Every module under `firemage/` is state and rules with no side effects. An Ignite tracker
reports that a burn ticked and *how many stacks it had*; the simulation decides what that is
worth from the stats of that moment. That is what keeps one function responsible for every
point of damage in the game, and it is what makes the no-snapshot rule structural.

## Abilities

Damage is a coefficient of spell power. Base spell power is 100, so 260 % reads as 260.

| # | ability | cast | cooldown | charges | range | area | damage | ignite | heat |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Firebolt | 1.0 s | – | 1 | 35 m | – | 100 % | 1 stack: always if cold, else 50 % | +2 |
| 2 | Flame Lance | 1.5 s | 8 s | 2 | 35 m | – | 260 % | +2 | +4 |
| 3 | Flame Cone | instant | 10 s | 1 | 12 m cone | 60° | 140 % | +2 to all | +3 |
| 4 | Meteor | instant | 20 s | 1 | 35 m | 5 m | 500 % +15 %/stack | – | +5 |
| 5 | Pyroblast | 3.0 s | 30 s | 1 | 35 m | – | 650 % +15 %/stack | – | +5 |
| 6 | Firestorm | instant | 24 s | 1 | 20 m line | 20×10 m, 6 s | 70 %/tick ×12 | 15 %/tick | +5 |
| 7 | Detonate | instant | 8 s | 1 | 35 m | 10 m | 22 % of remaining Ignite | consumes | +7 |
| 8 | Fire Mastery | instant | 60 s | 1 | self | – | – | – | – |
| 9 | Flame Shield | instant | 30 s | 1 | self | – | – | – | – |
| 0 | Blink | instant | 15 s | 1 | 10 m | – | – | – | – |
| R | **Moving Fire Beam** | 6.0 s channel | 90 s | 1 | 22 m beam | 2.6 m wide | 95 % per 0.25 s, ×24 | 1 stack a second | – |

A critical Flame Lance takes 1.0 s off Pyroblast and a critical Pyroblast takes 1.0 s off
Flame Lance — 1.5 s with Spellfire Feedback, and **never more than 1.5 s**, which is a
constant with a test rather than a convention.

### Ignite

30 seconds, 5 stacks, a 2-second interval divided by haste, 24 % spell power per stack per
tick. Applying refreshes the duration even at the cap. Five stacks over a full duration is
1800 % spell power: that is the bank.

Firebolt's asymmetry is deliberate: **100 % to apply on an unburnt target, 50 % on a burning
one**, and never past the maximum.

### Detonate

Requires the target at the current maximum. For every living enemy within ten metres, on its
*own* burn: 22 % of what that burn had left, then the burn is removed. An enemy with no burn
takes nothing — the explosion *is* the burn, not a splash of the primary's hit. Refused with
`NEEDS MAXIMUM IGNITE` below the cap, and a refusal spends no charge.

Twenty-two per cent rather than the sixty the brief opened with: at sixty, cashing a burn
every eight seconds beat letting it tick by three to one, and the maintenance build was
strictly worse at its own mechanic. The whole reasoning is in `docs/firemage-design.md` §15.

## Heat

0–100, generated by casting, never spent by the player.

```text
0-29    x1.00
30-69   x1.15
70-99   x1.30  and +5% haste or crit
100     spent: the Inferno State, x1.50 and +7.5%, for 10 s (12 with Furnace Mastery)
```

Haste shortens casts, the global cooldown, Ignite's interval and every cooldown **except
Detonate's**: haste already refills the bank faster, and letting it spend the bank more often
too was the same stat paid twice.

Reaching a hundred empties the bar and starts the state; whatever gathers during the state
goes with it when it ends. Heat does not decay in combat, and drains at 5/s after ten seconds
without dealing damage.

## Fire Mastery

A specialisation, not a purchase: **Haste** or **Crit**. It decides what the 60-second active
grants for 20 seconds (+15 %), and which stat the 70+ band and the Inferno State hand over.
Dual Fire Mastery trades the peak for both at once.

## The Ultimate: Moving Fire Beam

A channelled, steerable, server-authoritative beam. Every magnitude is a named constant in
`firemage/beam.rs` (to be moved to data later); the rules live in that file and the executor in
`sim/beam_exec.rs`.

```text
cost       a full Ultimate Energy bar, all of it, on the global cooldown     cooldown 90 s (not hasted)
channel    6.0 s, 24 damage ticks of 95 % spell power (0.25 s apart)
beam       from the mage, 22 m long, 2.6 m wide; Fire damage, normal crits, 1 Ignite stack a second
steering   the cursor's ground point is the target direction; the beam turns at most 75 deg/s
           (a half turn is 2.4 s). Born pointing at the aim point of the cast.
movement   40 % speed while channelling; no jump; Blink is refused
broken by  stun, silence, death, casting anything else, pressing R again, a hero swap or reset
```

**Tuning.** 24 x 0.95 = 22.8 against Pyroblast's 6.5 is 3.5 Pyroblasts on one target; every
enemy inside the rectangle takes it, so on a pack it is the class's largest burst. Ultimate
Energy was retuned to match: every ability's `energy` column was halved and the passive trickle
is a quarter of a point a second, so a rotation refills the bar in about ninety seconds, level
with the cooldown (`the_bar_refills_in_about_a_minute_and_a_half`). See
`docs/balance/sim-report.md`: against the Warrior and Druid together the Mage's default build
moves from 0.82-0.86 to 0.88-0.91 single target and from 0.69-0.75 to 0.75-0.82 on eight
targets in the 60 s windows (which include one opening beam each).

**Server authority.** The aim is an intent: `PlayerCommand::ChannelAim`, carried by the
`combat_act` reducer as `NetAction::ChannelAim` and rate limited by the existing combat limit
(the client sends at most ten a second, only when the point moved half a metre). The server
simulation turns the beam and deals all the damage. A client under a remote authority predicts
the beam's yaw for drawing only (and for its own movement slow), snaps it to the server's when
they disagree by more than 10 degrees, and deals no damage. Others see it through three
events: `BeamChannelStarted { entity, yaw, start_time, duration }`, `BeamYaw { entity, yaw,
turn_target_yaw, time }` (at most ten a second, only when the target moved; observers turn the
beam themselves in between) and `BeamChannelEnded { entity, reason }`.

**Presentation.** `engine-presentation/src/beam.rs` tracks every beam; the VFX director draws a
dense additive particle stream along it from the hands (`VfxId::FireBeam`), the telegraph
indicators lay a ground rectangle under it (`IndicatorKind::Beam`), the cast bar drains for the
channel and the Ultimate button wears the active frame. Audio: `BeamStart`, `BeamLoop` (held
while the channel runs) and `BeamEnd`.

## Talents

Twelve, costing 51 points in total against a 20-point build budget, so a build is four or five
of them. Three layers, unchanged from before: `TALENTS` (a table a designer edits),
`TalentGrid` (what is bought), `TalentEffects` (the numbers gameplay reads). Combat code reads
a field, never `if has_talent(..)`.

The simulator enumerates the same table. There is exactly one definition of a talent in the
project, and the talent menu, the tooltips, the gameplay and the balance search all read it.

## Gear

The class is balanced at the stats it is played at, not at base: 40–50 % in the build's
primary stat and 20 % in the off-stat. `Gear` is a field on `FireMage` that `stats()` adds up
like everything else, so changing it mid-fight reaches a burn that is already running.

## The combat simulator

`cargo run -p engine-sim --release -- --search --out docs/firemage-sim-report.md`

It drives the **real** `Simulation` headlessly — same abilities, same Ignite, same Heat, same
damage function — so a number it prints is a number the game produces. Targets have health and
die, they wander in and out of the line and the cone, casts against a corpse are wasted, and
the rotation AI decides when to detonate rather than following a script.

## Interface

The UI observes; it never computes. `UiSnapshot` is rebuilt from the simulation each frame.

- **Ability bar**: ten slots, radial cooldowns, charge counts, six states — ready, cooling,
  no resource, invalid target, *empowered* (Detonate against a full burn) and *disabled*.
- **Heat bar**: one colour per band with both thresholds marked, and during the Inferno State
  it shows the state's remaining seconds instead of a value that is sitting at zero.
- **Ignite**: one pip per possible stack *and* `IGNITE 3 / 5` in words, becoming `MAX IGNITE`
  at the cap. The maximum is the configuration's, never a literal five.
- **Tooltips** are generated from the ability's own row and the *current* stats: the cast time
  it prints is the hasted one, the damage is spell power times the coefficient, and Detonate's
  percentage comes from the constant the explosion uses.
- **Ground targeting**: one system, three shapes and three meshes. Meteor places a circle,
  Flame Cone aims a wedge, and Firestorm aims a **linear zone** — a 20 × 10 m strip anchored
  on the caster, rotated to the cursor, drawn with the same fill-and-edge pair as the others.

## Testing

- `firemage/*`: the data layers with no world — stacks, cap, refresh, the Heat bands and the
  Inferno State, out-of-combat decay, the interval rescaling that proves nothing snapshots,
  charge buckets, every talent on and off, the 1.5 s feedback cap.
- `tests/firemage.rs`: the mechanics against a real arena — Detonate's arithmetic against the
  burn's remaining worth, its ten-metre radius on each enemy's own burn, haste and crit
  reaching a running burn, casts whose target dies, Molten Meteor's ground and its debuff, and
  the Ultimate (see below).
- `engine-sim`: that a fight produces damage and counters, that the two rotations really
  differ, that targets die, that movement costs damage, and that a seed replays.

## Known limitations

- **The beam ignores geometry.** Like Firestorm's line it burns a rectangle on the flat and is
  not stopped by walls.
- **Nothing can stun or silence a player yet.** `Simulation::break_channel` is the hook the day
  something does; it is tested, but no enemy ability calls it.
- **Any other cast breaks the channel**, defensive cooldowns included, as specified.
- **Movement**: holding the look button pins the character's facing to the camera, so
  strafing and backpedalling keep the target in front. Released, the character faces where it
  is walking, as before.
- **Infernal Capacity is in almost every top build** — a dominating talent whose cost and
  effect are both pinned by the brief.
- **Firestorm's line ignores geometry.** It burns a rectangle on the flat.
- **Only three abilities have icons**; the rest show four letters of the name.
- **Ignite has no relationship to the generic `StatusSet`.** It has its own tracker.
