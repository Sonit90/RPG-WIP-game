# The Bone Foundry

The fourth production dungeon (`bone_foundry`, `assets/scenes/bone_foundry.json`). An industrial
necromantic foundry where the dead are smelted into soldiers. It asks four questions the earlier
dungeons do not: **who do I kick (a ward, a rally), what do I kill before it arrives (a fused
bomber), where do I stand (a shared soak), and which way do I step (a lane that carries).** Fully
data-defined: the scene, the `dungeons.json` row and the Overseer's `loot.json` table. The monsters
are new rows in the Rust tables ("approved primitives").

```text
cargo run --release --bin mygame -- --scene-file assets/scenes/bone_foundry.json
cargo test -p engine-game --test bone_foundry
cargo test -p engine-game --test export_bone_foundry -- --ignored   # regenerate the scene
```

## Layout

One character per 8 m cell in `crates/engine-game/tests/export_bone_foundry.rs` (`MAP`), turned into
walls, piers, paving, props, lights and packs by the generator. The output is a plain scene file: edit
it in the editor from then on (regenerating overwrites hand edits). The shape is a **ring round a
central furnace** with a yard off each arm, not a spine of rooms. The level is flat: the navigator has
no ramp support worth building a multi-level dungeon on, and the furnace island does the job of
"somewhere you cannot go".

```text
                 +Z south ... -Z north
   floor     B  (56 x 48 m)   THE FOUNDRY OVERSEER         z -184 .. -232
   gate      a  boss gate across the antechamber, z = -184
   hall      H  (40 x 24)     FURNACE CHOIR                z -152 .. -176
   north arm R  16 m wide     FOREMAN ON ROUNDS (patrol along z = -136)
   west arm  R  CRUCIBLE GANG       east arm  R  LADLE CREW
   west yard W  BONE-SORTING YARD   east yard X  SLAG WORKS      (optional, see below)
   south arm R  16 m wide     SLAG HAULER (patrol along z = -72)
   forecourt F  (40 x 24)     SCRAP FORECOURT
   hall      E  spawn, no pack; start barrier across the corridor at z = -24
```

The ring is the only road north, so every run **chooses an arm** (or clears both) and walks the
patrols. The two yards hang off the arms by one-cell doors. Completion (113 of 136) is everything but
the larger yard: either yard is optional, skipping both is not enough (the same rule as dungeon 3's
wings). Kit: crypt walls/piers/floors, pillars (full height, or half as cover), rocks, supply crates,
bone piles, soft iron fences as conveyor rails, braziers and low orange lights as the slag channels
(visual only: no new terrain, no damage from the glow), the mausoleum as the great furnace.

## Packs (8 + boss)

| Pack | Where | Members | Asks |
|---|---|---|---|
| SCRAP FORECOURT | forecourt | 2 warriors, 2 cinder bombers | teaches the fuse: kill or leave the circle |
| SLAG HAULER | south arm (patrol) | slag ironclad | time it, or pull it; kick the ward, turn the sweep |
| CRUCIBLE GANG | west arm | ironclad, warrior, 2 slag throwers | kick the ward before the lobs stack |
| LADLE CREW | east arm | grave brute, bomber, 2 slag throwers | movement under lobs and a fuse |
| BONE-SORTING YARD | west yard | foreman, 2 ghouls, bonecaster | the rally on a fast pack |
| SLAG WORKS | east yard | ironclad, 3 bombers, 2 throwers | spread, kill the bombers, kick the ward |
| FOREMAN ON ROUNDS | north arm (patrol) | foundry foreman | time his walk; interrupt the rally |
| FURNACE CHOIR | casting hall | ironclad, warrior, foreman, 2 throwers | everything at once, before the boss |
| THE FOUNDRY OVERSEER | foundry floor | boss | below |

## New monsters (all reuse existing models with tints)

| Id | Role | Model | Signature |
|---|---|---|---|
| `slag_ironclad` | Melee (shielder) | grave brute, iron grey | **Slag Bulwark**: 1.8 s interruptible cast; if it finishes, every ally within 14 m (itself too) takes 40 % less damage for 8 s. **Shield Sweep**: 120 degree, 6.5 m cone |
| `cinder_bomber` | Melee (suicide) | ghoul, ember | **Cinder Blast**: a 1.5 s fuse drawn as a 4.5 m circle for its whole length, 120 damage, then the bomber dies. 700 health |
| `slag_thrower` | Ranged | bonecaster, orange | **Slag Lob**: interruptible; a 3.2 m circle under each of two players, 1.7 s after the cast, that leaves slag burning 38 dps for 10 s |
| `foundry_foreman` | Elite | acolyte, ember | **Stoke the Furnace**: 2.2 s interruptible; allies within 16 m deal 30 % more damage for 10 s. **Chain Lash**: a 14 m line |
| `foundry_overseer` | Boss | grave brute, glowing | see below |

**Warded** (Buff, 8 s, 40 % less damage taken) and **Stoked** (Buff, 10 s, 30 % more damage dealt) are
statuses on the monsters, shown on the nameplate. They are not dispellable: the answers are the kick, a
burst on the foreman or the ironclad first, or patience.

## The Foundry Overseer (19,000 health)

Between the Abbess (17,000) and a harder dungeon's boss; he is never faster than a player.

| Phase | Starts | List | Notes |
|---|---|---|---|
| 1 | 100 % | Foundry Slam, Slag Brand, Hammer | slam is a 7.5 m circle: everyone out |
| 2 THE LADLE | 70 % | **Molten Ladle**, slam, brand, hammer | the soak joins the spread |
| 3 THE CONVEYOR | 50 % | **Conveyor Surge**, ladle, slam, brand, hammer | calls **3 cinder bombers** |
| 4 OVERHEAT | 25 % | **Overheat**, surge, ladle, brand, slam, hammer | calls **2 slag throwers**; soft enrage |

* **Slag Brand** (the spread): three marks that follow their players, 4.5 m circles, 4 s. Whoever a
  detonation catches besides its victim is marked in turn, so a clump takes the brand twice over.
  Decision: spread, and do not run through each other.
* **Molten Ladle** (the soak): a 5.5 m circle on one player's ground, drawn for 5.5 s. Its 330
  damage is **shared by whoever stands in it**; alone it is three times a share, and if nobody is in it
  every player takes 125 % of the whole, so even a solo player soaks it. Decision: everybody walks to the mark together, so the brand
  and the ladle pull the party in opposite directions in the same phase.
* **Conveyor Surge** (the movement): a 10 m wide, 34 m lane aimed at a random player for a 5 s
  uninterruptible cast. Everyone inside is carried away from him at 3.4 m/s (a collided slide, so a wall
  stops them) while slag pools open at the lane's far end, then the lane hits whoever is still on it.
  A player walks out sideways in a second; a player who rides it ends in the slag. Decision: leave the
  lane early, and do not be in it when the bombers arrive.
* **Bombers** (50 %): three fused runners at the party; each is a circle to leave or a body to kill.
* **Overheat** (25 %): once, for the rest of the fight, everything he does comes 35 % faster; two more
  throwers arrive. The party has to burst him, which is what a soft enrage is.

## Data

* `assets/catalog/dungeons.json`: spawn points, start barrier, keys (1-99), reward table,
  `timer_seconds` 1800 (estimated: about 300 m of route plus a yard detour is a couple of minutes of
  walking; eight packs at about a minute and a quarter each, patrol timing, and about four minutes on
  the Overseer is roughly 22 minutes for a competent party, with margin).
* `assets/catalog/loot.json`: boss table `foundry_overseer`, and five new epics (item level 13) for the
  slots the earlier tables left bare: back, wrist, an armoured head and chest, a necklace.
* New primitives (Rust): `EnemyEffect::{Bestow, Detonate, Soak, Conveyor}`, `StatusKind::{Warded, Stoked}`
  (appended last), and delayed hazards (a delayed zone whose ability leaves a pool now lights it when it
  lands).

## Not verified here

No visual pass beyond the baked minimap (`--bake-map --hidden`); the conveyor's feel under client
prediction, like the Undertow's, is untested online; timer and Overseer tuning are estimates from the
simulation, not from a played run.
