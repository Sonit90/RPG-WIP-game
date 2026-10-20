# The Drowned Catacombs

The third production dungeon (`drowned_catacombs`, `assets/scenes/drowned_catacombs.json`). A flooded
catacomb that asks different questions from the crypt: **which wing do we take, who do we kick, what do we
dispel, and which way do we run.** Fully data-defined: the scene, the `dungeons.json` row and the
Abbess's `loot.json` table. The monsters are new rows in the Rust tables ("approved primitives").

```text
cargo run --release --bin mygame -- --scene-file assets/scenes/drowned_catacombs.json
cargo test -p engine-game --test drowned_catacombs
cargo test -p engine-game --test export_drowned_catacombs -- --ignored   # regenerate the scene
```

## Layout

One character per 8 m cell in `crates/engine-game/tests/export_drowned_catacombs.rs` (`MAP`); the generator
turns it into walls, piers, paving, props, lights, water, packs and the dungeon block. The output is a plain
scene file: edit it in the editor from then on (regenerating overwrites hand edits).

```text
                 +Z south ... -Z north
   chapel   A  (56 x 48 m)   THE DROWNED ABBESS           z -136 .. -184
   gate     a  boss gate across the antechamber, z = -136
   ossuary  O  (56 x 24)     OSSUARY WATCH, BONE CHAPTER  z -104 .. -128
   approach c  west and east corridors round to the ossuary doors
   wings    W  west: SUNKEN CLOISTER   east: BILGE BATTERY  (24 x 40 each)
   gallery  g  8 m corridors from the sluice to each wing, one lurker walks each
   sluice   H  (56 x 24)     SLUICE KEEPERS, waist-deep channel across the middle
   nave     N  (40 x 24)     FLOODED PICKETS
   hall     E  (24 x 24)     spawn, no pack; start barrier across the corridor at z = -24
```

The wings are the only roads to the ossuary, so **every run chooses a wing.** Completion (84 needed) is
everything but the larger wing, so the other wing is optional. Water is visual only (the scene's water
bodies add no collision and nothing depends on them). Kit: crypt walls/piers/floors/sarcophagi/bone piles,
pillars (full height, or half as cover), rocks, supply crates, the mausoleum behind the Abbess.

## Packs (8 + boss)

| Pack | Where | Members | Asks |
|---|---|---|---|
| FLOODED PICKETS | nave | skeleton warrior, crypt ghoul, tidecaller | kick or kill the first healer |
| SLUICE KEEPERS | sluice | grave brute, 2 cultists | first Waterlogged; dispel |
| WEST GALLERY LURKER | gallery (patrol) | catacomb lurker | time it, or pull it on purpose; turn its cleave |
| SUNKEN CLOISTER | west wing | warrior, 2 cultists, tidecaller | interrupts and dispels |
| EAST GALLERY LURKER | gallery (patrol) | catacomb lurker | as above |
| BILGE BATTERY | east wing | grave brute, 3 bilge archers | movement, break line of sight, kick volleys |
| OSSUARY WATCH | ossuary | 2 warriors, ghoul, tidecaller | melee pack with a healer |
| BONE CHAPTER | ossuary | brute, 2 archers, cultist | mixed |
| THE DROWNED ABBESS | chapel | boss | below |

## New monsters (all reuse existing models with tints)

| Id | Role | Model | Signature |
|---|---|---|---|
| `drowned_tidecaller` | Support | bonecaster, teal | **Tidal Mend**: 2.2 s interruptible heal (280) on the most wounded ally; 700 health |
| `drowned_cultist` | Ranged | acolyte, teal | **Brine Bolt**: interruptible, random player, stacks **Waterlogged** |
| `catacomb_lurker` | Elite | brute, teal | patrols a corridor at 45 % speed; **Tidal Cleave**: 120 degree, 9 m cone, 1.6 s |
| `bilge_archer` | Ranged | archer, teal | **Bilge Volley**: two 3.2 m circles on each of three players, 1.9 s after a kickable cast |
| `drowned_abbess` | Boss | acolyte, large | see below |

**Waterlogged** (Magic, dispellable, 12 s, stacks to 3): 15 % slower and 8 % more damage taken per stack.
Three stacks (45 % slower) cannot out-walk the Undertow.

## The Drowned Abbess (17,000 health)

| Phase | Starts | List | Notes |
|---|---|---|---|
| 1 | 100 % | Tidal Slam, Crozier, Brine Bolt | slam is a circle: everyone out |
| 2 THE HYMN | 66 % | **Drowning Hymn**, Undertow, slam, crozier, bolt | hymn once this phase |
| 3 THE CHOIR | 50 % | Undertow, slam, crozier, bolt | calls **2 drowned tidecallers** at the altar |
| 4 THE DEEP | 33 % | **Drowning Hymn**, Undertow, ... | hymn once more, faster cooldowns |

* **Undertow** (7 s, uninterruptible): drags every player in range toward her at 4.6 m/s (a collided slide)
  while a drowning pool (3.4 m, 38 dps, 9 s) opens every 1.2 s on a seeded ring 3.5-9.5 m from her, then
  slams a 7 m circle (110) at her feet. A player walks out at 7 m/s and wins by a little. Decisions: leave
  early, dispel Waterlogged beforehand, do not stand still to cast, use the chapel pillars as cover.
* **Drowning Hymn** (6 s, interruptible, once per phase; an interrupt does not give it back): if it finishes
  she heals 10 % and every ally attacks 60 % faster for 14 s. Decision: who kicks, and (at 33 %) whether
  the called tidecallers died first.
* **Tidecallers** (50 %): they mend her and are empowered by an unanswered hymn; kill or kick them.

## Data

* `assets/catalog/dungeons.json`: spawn points, start barrier, keys (1-99), reward table, `timer_seconds` 1500
  (estimated: ~17 min of fighting and walking for a competent party plus margin).
* `assets/catalog/loot.json`: boss table `drowned_abbess`, and five new epics (item level 12) for shoulder, legs,
  hands, feet and a second ring.
* New primitives (Rust): `EnemyRider::Afflict`, `EnemyEffect::{Hymn, Undertow}`, `EnemyAbilityDef::once_per_phase`,
  `EnemyPhaseDef::summon`, `StatusKind::Waterlogged`.

## Not verified here

No visual pass beyond the baked minimap; the Undertow's client-side feel under prediction is untested.
