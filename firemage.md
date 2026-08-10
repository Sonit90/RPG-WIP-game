# FireMage

The first playable class: ten abilities, a separate Ultimate, two resources, a stacking burn
that never explodes on its own, and a twenty-five talent grid that changes what all of it
means.

## The rule the class is built on

**Reaching maximum Ignite does nothing.** No automatic combustion, no hidden timer. The
payoff belongs to the player, who spends it with Fire Detonation when it is worth spending,
and the ability refuses until the burn is full. Every ability that applies Ignite - Firebolt,
Flame Wave, Meteor, Firestorm, Inferno, Cataclysm - stops at the cap and waits.

**Nothing hardcodes five.** The maximum is `TalentEffects::ignite_max_stacks`, read where it
is needed. Infernal Capacity moves the cap, the detonation requirement, the stack pips and the
tooltip text together, because they all ask the same field.

## Architecture

```text
engine-game/src/firemage/
    ability   the table: cast, cooldown, charges, range, radius, heat, energy, damage
    ignite    stacks, refresh, consumption. Reports ticks; deals no damage
    resource  Heat with its Overheat, Ultimate Energy with its trickle
    talent    data -> TalentGrid (what is bought) -> TalentEffects (what it means)
    zone      ground fire, and the impacts still falling towards it
    rng       seeded, part of simulation state, so a fight replays exactly

engine-game/src/sim/firemage_exec.rs
    validation, target collection, crit rolls, damage, Ignite, Heat/Energy, Blink

engine-presentation/src/
    snapshot.rs           what the UI is allowed to know, rebuilt every frame
    game_ui.rs            ability bar, resource bars, Ultimate, tooltips, Ignite pips
    ui/targeting.rs       one ground-targeting system for three abilities
    ui/talent_menu.rs     the talent grid, four tiers of nodes
```

Every module under `firemage/` is state and rules with no side effects. An Ignite tracker
reports that a burn ticked; the simulation turns that into damage, events and deaths. That is
what keeps one function responsible for every point of damage in the game, and it is what
makes all of it testable without a world.

## Abilities

| # | ability | cast | cooldown | charges | range | radius | heat | energy |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Firebolt | 1.0 s | - | 1 | 35 m | - | +5 | +1 |
| 2 | Flame Lance | 1.5 s | 8 s | 2 | 35 m | - | +10 | +2 |
| 3 | Flame Wave | 1.2 s | 10 s | 1 | 12 m cone | - | +3/target, max 15 | +2 |
| 4 | Meteor | 1.0 s | 18 s | 1 | 35 m | 5 m | +15 | +5 |
| 5 | Firestorm | 0.8 s | 24 s | 1 | 35 m | 6 m | +3/tick, max 30 | +8 |
| 6 | Pyroblast | 3.0 s | 30 s | 1 | 35 m | - | +25 | +10 |
| 7 | Inferno | instant | 45 s | 1 | self | 8 m | +20 | +10 |
| 8 | Fire Detonation | instant | 8 s | 1 | 35 m | 10 m | +15 | +3 |
| 9 | Flame Shield | instant | 30 s | 1 | self | - | - | - |
| 0 | Blink | instant | 15 s | 1 | 10 m | - | - | - |
| R | **Cataclysm** | 1.0 s | - | 1 | 35 m | 10 m | - | costs 100 |

Damage numbers are in the same table: Firebolt 120, Flame Lance 260 (+40% into a burn),
Flame Wave 180, Meteor 420, Firestorm 60/tick, Pyroblast 800 (+10% per stack, guaranteed crit
at the cap), Inferno 140/tick, Fire Detonation 300 + 120 per stack consumed, Cataclysm 1200
on impact and 150/s afterwards. Base crit 15%, crit multiplier 2.0.

### Ignite

8 seconds, 5 stacks by default. Applying refreshes the duration even at the cap - a target
held at maximum must not fall off mid-rotation. 12 damage per stack per second, so a stack is
worth something before it is spent.

Firebolt's asymmetry is deliberate: **100% to apply on an unburnt target, 50% on a burning
one**, and never past the maximum.

### Fire Detonation

Requires the target at the current maximum. Consumes the whole burn, deals its scaled damage,
and splashes 60% of it to everything within 10 m, applying a stack to each. Refused with
`NotEnoughIgnite` below the cap, and a refusal spends no charge.

## Resources

**Heat**, 0-100. Nothing spends it; it fills as the class plays. At 100 it Overheats: +15%
fire damage while it drains at 12/s. 80 is the "running hot" threshold the bar marks and two
talents read.

**Ultimate Energy**, 0-100. +1 every two seconds, plus what abilities generate. At 100
Cataclysm can be cast, and casting it empties the bar.

## Casting

A cast is a commitment: the charge is spent when it starts.

**Global cooldown: 0.5 s**, started by any ability and shortened by haste. It is deliberately
shorter than most cast times - a 1 s Firebolt outlasts it, so back-to-back casting is limited
by the cast and never by the timer, while two instants cannot go off in the same frame. A
press during a cast still queues: the queue starts when the cast lands, by which time the
global cooldown that cast began has long expired.

- **Pressing another ability queues it.** The running cast is not disturbed; the queued one
  starts the moment it lands, and is re-validated then because its target may have died.
- **One slot, not a list.** Mashing four keys means "then that one".
- **Only moving and Escape cancel**, and cancelling takes the queue with it.
- **A cancelled cast is refunded.** The charge comes back, the Ultimate's energy comes back,
  and so does the global cooldown: a cast that never landed costs the time it wasted and
  nothing else. A completed one keeps all three, because something happened.
- **An ability with no charge, or held by the global cooldown, cannot be selected at all.**
  Its key and its button do nothing rather than opening a preview for a cast that would be
  refused on the click.
- **Nothing is spent until the cast lands.** Requirements - charges, the global cooldown,
  energy, target, range, facing, the Ignite maximum - are all checked before it starts, with
  the reason; the charge and the Ultimate's energy buy what actually happened.
- **Spells are thrown forwards.** A *targeted* ability needs its target inside the frontal
  half-circle, ninety degrees either side of where the character faces, and refuses with
  YOU ARE FACING THE WRONG DIRECTION otherwise. An *aimed* ability - a placed circle or a
  cone - has no wrong direction: choosing where it goes turns the character to face it.
- **A refusal is shown for five seconds** and then stops being reported. The simulation owns
  that, so the interface cannot keep a message alive longer than the rule that produced it.

## Talents

25 talents in 4 tiers, bought in any order - the tiers group power, they are not a
prerequisite chain. Three layers: `TALENTS` (a table a designer edits), `TalentGrid` (what is
bought), `TalentEffects` (the numbers gameplay reads). Combat code reads a field, never
`if has_talent(..)`, so a talent that changes an existing number is one line in
`TalentEffects::from_grid` and no change where the number is used.

`TESTING_MODE = true` grants exactly `total_talent_cost()` points - **derived from the table,
never written down**, so a new talent raises the testing budget automatically. With it off the
class starts at zero and earns them.

## Interface

The UI observes; it never computes. `UiSnapshot` is rebuilt from the simulation each frame and
every widget reads it, so a button cannot claim an ability is ready when the simulation would
refuse it, and a tooltip is generated from the ability's own row.

- **Ability bar**: ten slots with keys, names, charge counters (only where there is more than
  one), radial cooldowns and five state colours - ready, cooling, no resource, invalid target,
  and *empowered* for Fire Detonation against a full burn.
- **The global cooldown is drawn on every slot**, in a lighter wash than a real cooldown,
  because starting one ability locks all of them. A slot already showing its own cooldown
  draws only that: the two washes would darken the overlap twice.
- **The ability you are in the middle of is framed in white**, doubled outward, so it wins
  against ten other frames. That is the one being cast *or* the one whose area is being
  placed: aiming a Meteor and casting a Pyroblast are the same answer to "what am I doing",
  and without it the only sign a placement is open is the circle on the ground, which is
  where the player is not looking.
- **A cooldown prints the seconds left** - whole ones until the last, then tenths, rounded up
  so it never reads zero while the ability still refuses. The global cooldown prints nothing:
  half a second in tenths on eleven buttons at once is noise.
- **Cooldowns** are a `RadialFill` command the renderer tessellates into the existing batch.
  The dark wedge's leading edge sweeps **clockwise from twelve o'clock**.
- **Ultimate button**: larger, separated from the bar, glowing at 100 Energy.
- **Resource bars**: Heat with its Overheat threshold marked, Ultimate Energy with a ready
  state.
- **Ignite pips**: one per possible stack, so the count of pips *is* the current maximum.
- **Ground targeting**: one system for Meteor, Firestorm and Cataclysm, in the shape of the
  genre's. Activating an ability enters a targeting state rather than casting; a world-space
  indicator follows the cursor across the ground every frame; left click confirms, right click
  or Escape cancels. The indicator is two flat meshes 3 cm up - a shaded fill for the
  footprint and a brighter ring for its edge, the ring breathing slowly - scaled to the
  ability's own radius, red when the placement is out of range and refused before the click.
  Enemies the shape covers are ringed on screen.

  The indicator is a **fixed size**: it shows the area the ability covers, so anything that
  animated its scale would be lying about it.

  The **shape** comes from the ability, not from the targeting code: `TargetShape` covers
  `Circle`, `Cone`, `Rectangle` and `Line`, with containment tested for all four. Circles and
  cones have meshes; a circle is placed under the cursor and a **cone is anchored on the
  caster and aimed** - the cursor picks a direction, so a cone is never out of range, and the
  wedge shown is the wedge that will be caught. Rectangle and line are defined and tested,
  and get a mesh when an ability asks for one.
- **Talent menu**: the button in the top-right corner, all 25 nodes by tier with costs,
  descriptions and state.

### Escape

One press backs out of exactly one thing, most local first:

```text
talent window -> targeting circle -> cast in progress -> pause menu
```

## Testing

- `firemage/*`: the data layers, unit tested with no world - stacks, cap, refresh, Heat's
  boil-over edge, energy, charge buckets, talent effects, zone ticks, seeded rolls.
- `sim/firemage_exec.rs`: the mechanics against a real arena - Firebolt's guaranteed ignite,
  the cap never leaking, nothing detonating by itself, the detonation requirement and its
  splash, Flame Lance's bonus, Pyroblast's guaranteed crit, Meteor's delay, zones burning and
  expiring, Blink's distance, resources, the Ultimate's cost, cast queueing and cancelling.
- `snapshot.rs`, `game_ui.rs`, `ui/*`: what the interface shows, including that the Ultimate
  is not on the bar and that Fire Detonation only lights up against a full burn.

## Known limitations

- **Damage numbers are unvalidated by play.** They are tuned against 200,000 HP dummies and are
  one table edit away from anything else.
- **The talent purchase click is covered by unit tests but not by a live click-through.**
- **Ignite has no relationship to the generic `StatusSet`.** It has its own tracker, which is
  why nameplates read it directly.
- **Zones do not read the collision world.** A Firestorm on a ledge burns a circle on the flat,
  not the geometry.
- **Only three abilities have icons.** Firebolt, Meteor and Flame Wave draw art from
  `engine-renderer/assets/icons`, rasterized into the UI atlas at build time at every size the
  interface can draw them. The other eight still show four letters of the name, and the
  layout is the same either way — an ability gains art by adding an SVG, a `UiIcon` variant
  and a line in `icon_for`.
- **The interface scales.** `INTERFACE SIZE` in the settings screen runs from 0.75 to 2.0 and
  is saved to `settings.cfg`. Every HUD, menu and talent-screen measurement is a logical pixel
  multiplied by it; text snaps to whole glyph scales, because the font is a bitmap.
- **The area indicator's fill is opaque, not translucent.** The world mesh pass has blending
  disabled, so alpha would be ignored; the fill is a dark shade the ground reads through
  tonally and the bright ring carries the shape. A real translucent fill needs a blended
  pipeline, instance sorting and a second draw - worth doing, and deliberately not smuggled
  into this change.
- **Cone, rectangle and line targeting have no mesh yet.** The shapes exist and are tested;
  nothing asks to place one.
