# The Sanctum of Ash

The fifth production dungeon (`sanctum_of_ash`, `assets/scenes/sanctum_of_ash.json`) and the hardest
of the five: a burnt-out temple of the ash cult, the Ashen Reliquary's big sibling. Its questions are
**what do I stop hitting (a mirror), who do I cleanse (a curse), who is the weakest (a hunter's
leap), where do I kill it (a death that spreads), what do I kill first (pylons that hold the boss
immune) and what stands between me and the Pontiff (a pillar).** Fully data-defined: the scene, the
`dungeons.json` row and two `loot.json` tables; the monsters are new rows in the Rust tables.

```text
cargo run --release --bin mygame -- --scene-file assets/scenes/sanctum_of_ash.json
cargo test -p engine-game --test sanctum_of_ash
cargo test -p engine-game --test export_sanctum_of_ash -- --ignored   # regenerate the scene
```

## Layout

One character per 8 m cell in `crates/engine-game/tests/export_sanctum_of_ash.rs` (`MAP`). The shape
is a **hub with three shrines**: the nave leads to the cinder rotunda; a shrine hangs off its west
door, one off its east door, and the road north runs *through* the third. Flat (the navigator has no
ramp support worth building on).

```text
   sanctum floor  B  72 x 56 m  THE ASHEN PONTIFF       z -200 .. -256, gate at z = -200
   antechamber    a, corridor c
   gallery        G  40 x 24    GALLERY OF CENSERS: the Censer Warden and two cantors
   shrine         S  SHRINE OF THE HUNT (the road north)
   rotunda        H  56 x 40    CINDER WATCH (an immolator patrols z = -92)
        west door W SHRINE OF MIRRORS            east door X SHRINE OF EMBERS
   nave           N  ASHEN NAVE   -   entrance hall E, start barrier at z = -24
```

**Completion rule: nothing is optional.** All six packs before the gate are 100 % (129 points); skipping
any shrine, the watch or the guardian leaves the gate shut (tested pack by pack). Six great pillars
(3 m across, solid to sight) stand on the sanctum floor.

## Packs

| Pack | Members | Asks |
|---|---|---|
| ASHEN NAVE | penitent, 2 warriors, cantor | kick the ward, cleanse the curse |
| CINDER WATCH (patrol) | immolator | where it dies |
| SHRINE OF MIRRORS | 2 penitents, cantor, 2 bonecasters | reflect plus casters |
| SHRINE OF EMBERS | 2 immolators, cantor, brute, archer | fires on fires |
| SHRINE OF THE HUNT | 3 stalkers, immolator, 2 archers | keep the weakest alive |
| GALLERY OF CENSERS | Censer Warden, 2 cantors | smoke, curses, the flare |
| THE ASHEN PONTIFF | boss | below |

## New monsters

| Id | Signature |
|---|---|
| `mirror_penitent` (3,600) | **Mirror Ward**: 1.6 s interruptible cast; if it finishes, allies within 6 m (itself too) are Mirrored for 6 s: half of every hit they take is sent back at the attacker, capped at 12 % of the attacker's maximum health, and never reflected in turn |
| `ash_cantor` (1,700) | **Litany of Ash**: interruptible, two random players get a stack of the **Ashen Curse** (Curse school, dispellable: 6 % more damage taken and 3 % slower per stack, to 5); **Ashen Mend** heals its worst-off ally |
| `ash_stalker` (1,900) | **Ambush Leap**: instant leap at the player with the least health left (`EnemyTarget::Weakest`), 3.5 m landing circle and a slow; **Stalker Slash** leaves a stacking bleed |
| `pyre_immolator` (3,000) | **Pyre Slam** circle; **Ember Collapse** on death (`on_death`): a 3.5 m blast and a fire that spreads to 8 m over 14 s. A development `slay` lights nothing |
| `ember_pylon` (1,300) | boss-called, rooted. **Ash Tithe**: 3 s interruptible pulse that fills the sanctum, every 8 s per pylon; stone stops it. While any pylon stands the boss takes no damage |
| `censer_warden` (11,000, elite, own table) | Smoke Veil (two delayed circles that burn), Censer Swing, Litany of Ash; at 50 % **Censer Blaze**, an 8.5 m circle |

## The Ashen Pontiff (22,000 health)

Above the Overseer's 19,000; never faster than a player (6.0 m/s).

| Phase | Starts | Notes |
|---|---|---|
| 1 | 100 % | **Branding Strike**, Pyre Sweep, mace |
| 2 THE RITE | 75 % | **Rite of Cinders** joins |
| 3 THE PYRES | 50 % | calls **3 ember pylons**; he is immune until they all fall |
| 4 THE LAST RITE | 25 % | **Last Rite** (once, permanent, +40 % speed of everything) and **2 immolators** |

* **Branding Strike** (the stack, the tank swap): a 5.5 m cone drawn for 1.4 s; each hit leaves
  **Branded** for 20 s (12 % more damage taken per stack, to 8; Magic, dispellable). Answers: kick
  the cast, step out of the cone (the only answer alone), dispel the brand, or hand him to a second
  tank (stacks are per victim). Fits 1 to 5 players.
* **Rite of Cinders** (line of sight): 220 damage in a 90 m circle drawn for 4.5 s, uninterruptible.
  The telegraph covers the room; blasts do not pass stone (`blast_reaches`), so the answer, alone or
  in a party, is to stand behind a great pillar. Tested against the shipped scene.
* **Pylons** (intermission with a clock): the pulses add up (30 each, every 8 s, per pylon), so the
  party kills the pylons (1,300 health each, easy), kicks the tithe or shelters from it.
* **Last Rite** is the hard enrage. The immolators' fires turn the floor into a clock too.

## Data

* `assets/catalog/dungeons.json`: heroic, keys 1-99, `timer_seconds` 2400 (estimate: about 540 m of
  route with the two shrine detours is two minutes of walking; six packs at about a minute and a half
  each, the Warden about four minutes, the Pontiff about eight, plus regroups).
* `assets/catalog/loot.json`: tables `ashen_pontiff` and `censer_warden` (small, with two epics of its
  own), six epics: four armoured pieces at item level 14 for shoulder, legs, hands and feet (none
  existed), and a wrist and a necklace at 13 from the Warden.
* New primitives (Rust): `StatusKind::{Mirrored, Ashen, Branded}` (appended last, count 39),
  `EffectModifier::Reflect`, `EnemyTarget::Weakest`, `EnemyArchetype::{on_death, shields_boss}`, and a
  mid-dungeon monster with its own boss table paying one roll per player where it falls
  (`drop_kill_loot`; the catalog accepts elites as table keys). The Warden is an elite rather than a
  boss so the community-map validator still sees one boss pack.

## Not verified here

No visual pass beyond the baked minimap (`--bake-map --hidden`); timers, damage and the Pontiff's
tuning are estimates from the simulation, not a played run; the pylon immunity has no dedicated
client indicator beyond the boss's health not moving; online feel is untested.
