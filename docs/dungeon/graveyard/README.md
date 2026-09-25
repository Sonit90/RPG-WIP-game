# The Graveyard Crypt

A small, complete dungeon vertical slice: an abandoned graveyard over a crypt, six undead
monsters, two optional routes, a completion requirement, and a Moroes-inspired boss who calls
two **empowered** versions of monsters the party has already fought.

```text
cargo run --release --features editor --bin mygame -- --scene graveyard            # solo
cargo run --release --features editor --bin mygame -- --scene graveyard --party 3  # tuned numbers
cargo run --release --features editor --bin mygame -- --scene graveyard --editor   # open in the editor
cargo test -p engine-game --test graveyard
```

**The level is a file.** Everything about the crypt — walls, graves, lights, every monster
placement, its packs and its dungeon settings — lives in
[`assets/scenes/graveyard_crypt.json`](../../../assets/scenes/graveyard_crypt.json), the same
format the level editor opens and saves. No Rust describes its layout. `--scene graveyard` is
shorthand for `--scene-file <assets>/scenes/graveyard_crypt.json`.

| Document | What it covers |
|---|---|
| this file | layout, routes, completion, balance, reset, data |
| [`monsters.md`](monsters.md) | the six monsters, their abilities, their empowered forms |
| [`boss_lord_ashgrave.md`](boss_lord_ashgrave.md) | the boss, his adds, the encounter |
| [`../../level-editing.md`](../../level-editing.md) §3 | the file format and placing monsters in the editor |

---

## Layout

```text
                                   −Z (forward)
                     ┌──────────────────────────────┐
                     │ HALL OF THE HOLLOW STEWARD   │  the boss — sealed below 100 %
                     └──────────────┬───────────────┘
                                  ═══╪═══  the gate (a violet wall until 100 %)
   ┌──────────────┐     ┌───────────┴──────────┐     ┌──────────────┐
   │ CATACOMB NAVE├─────┤ HOLLOW CLOISTER      ├─────┤ MASS GRAVE    │
   │ (A2) 2 packs │     │ final area, 1 pack   │     │ (B2) 2 packs  │
   └──────┬───────┘     └──────────────────────┘     └──────┬───────┘
          │                                                  │
   ┌──────┴───────┐     ┌──────────────────────┐     ┌──────┴───────┐
   │ OSSUARY CRYPT├─────┤ MOURNERS CROSSING    ├─────┤ WEEPING GRAVES│
   │ (A1) 2 packs │     │ hub, 2 packs         │     │ (B1) 2 packs  │
   └──────────────┘     └──────────┬───────────┘     └──────────────┘
     ROUTE A: the crypt            │                    ROUTE B: the graveyard
                           ┌───────┴───────┐
                           │ CHAPEL YARD   │ 2 packs
                           └───────┬───────┘
                           ┌───────┴───────┐
                           │ LYCHGATE      │ the entrance — no pack
                           └───────────────┘
                                   +Z
```

The hub has no door north: the only ways to the cloister are through a route. Both routes
reconnect in the cloister, whose north door leads to the gate and the boss hall. Rooms are whole
numbers of 8 m wall segments and doorways are two segments left out, the Ashen Reliquary's rule.
Graves (flattened rocks), sarcophagi (long crates), dead trees, broken columns, banners and
braziers are placed with the existing library; the lighting is a cool moon over a bright blue
fill, with warm braziers at every doorway.

Two packs in one room stand further apart than the widest aggro radius in the roster (22 m),
and the player spawns out of every pack's reach — both are tested against the file.

---

## Routes

| Route | Rooms | Packs | Worth | Emphasis |
|---|---|---|---|---|
| **Main** | Chapel Yard, Mourners Crossing, Hollow Cloister | 5 | **51** | one of everything |
| **A — the Crypt** | Ossuary Crypt, Catacomb Nave | 4 | **52** | Bonecasters and Acolytes: interrupts, ground, marks, positioning |
| **B — the Graveyard** | Weeping Graves, Mass Grave | 4 | **49** | Brutes and Ghouls: charges, cleaves, bleeds, quakes, movement |

Neither route is "the hard one": A punishes a party without an interrupt plan and B one that
cannot move and heal through bleeds.

### Packs and completion values

Completion is a **weighted sum**: each monster is worth its table value (`completion` in the
level file overrides it), against `required_completion` = **100**.

| Monster | Worth |
|---|---|
| Skeleton Warrior | 4 |
| Skeleton Bonecaster | 4 |
| Crypt Archer | 4 |
| Grave Acolyte | 5 |
| Grave Brute | 6 |
| Crypt Ghoul | 3 |
| Empowered adds, boss | 0 |

| Pack | Route | Members | Worth |
|---|---|---|---|
| Gate Wardens | Main | 2 × Warrior | 8 |
| Bone Choir | Main | Bonecaster, Warrior | 8 |
| Crossing Sentries | Main | Brute, Archer | 10 |
| Mourners | Main | Acolyte, 2 × Ghoul | 11 |
| Cloister Guard | Main | Brute, Warrior, Bonecaster | 14 |
| Ossuary Watch | A | 2 × Bonecaster, Warrior | 12 |
| Candle Vigil | A | Acolyte, Bonecaster, Warrior | 13 |
| Bone Chapter | A | 2 × Acolyte, Warrior | 14 |
| Crypt Choir | A | Bonecaster, Acolyte, Warrior | 13 |
| Gravediggers | B | Brute, 2 × Ghoul | 12 |
| Restless Dead | B | 2 × Warrior, Ghoul | 11 |
| Charnel Pit | B | 2 × Brute | 12 |
| Feeding Frenzy | B | 2 × Ghoul, Warrior, Archer | 14 |
| Hollow Steward | Boss | Lord Ashgrave | 0 |

* Main alone: **51 %** — the boss stays sealed.
* Main + A: **103 → 100 %**. Main + B: **exactly 100 %**.
* Skipping any pack of the route you chose leaves you short; clearing a pack of the other route
  makes it up. Both routes at once: 152, clamped at 100 %.

### The gate

Below 100 % the boss's pack is **sealed**: a collision box stands across the hall's doorway
(drawn as a violet wall of light), and the boss notices nobody and takes no damage. The kill
that reaches 100 % removes the gate, announces `BossUnlocked`, and the panel says so.

### The progress panel

Top right, in the game's HUD, drawn only in a dungeon:

```text
GRAVEYARD CRYPT                      GRAVEYARD CRYPT
COMPLETION                           COMPLETION
████████████████░░░░ 82%             ████████████████████ 100%
REQUIRED: 100%                       THE PATH TO THE CRYPT LORD IS OPEN
```

During the boss it counts the adds still standing; at the end it shows the run's time.

---

## Balance: a party of three

Tuned for **a Warrior tank, a damage dealer and a Druid healer**, all at the heroes' base 500
health. The game has one player today, so `--party N` scales monster health and damage
(party 1: ×0.45 health, ×0.6 damage; 2: ×0.7/×0.8; 3: ×1/×1; 4–5: more health). **Mechanics,
telegraphs and timings never scale** — a solo run meets the same fight with smaller numbers.

Assumptions the tests hold the numbers to (`engine-game/tests/graveyard.rs`, *balance*):

| Assumption | Value | Rule |
|---|---|---|
| Party health | 500 each; tank ⅓ physical mitigation from base armour | — |
| Healer throughput | 45 HPS sustained | a pack's **unavoidable** damage on the tank ≤ 1.25 × that |
| Party single-target DPS | 180 | every trash pack dies in **10–40 s** |
| Avoidable hit | — | **≤ 25 %** of a player (survivable once), **≥ 8 %** (worth avoiding); pools count 2 s of standing |
| Warning | — | every avoidable (ground) ability gives **≥ 0.9 s** (cast + delay) |
| Charges | — | cast on a player (cast bar, no floor telegraph), follow them, hit them and **slow 50 % for 1 s** |
| Movement | player 7 m/s | every crypt monster, every phase, slower |
| Boss fight | boss + two largest adds | **1.5–3.5 min** at 180 DPS |

The deaths the design wants to be explainable: *I stood in the Rain of Arrows. I didn't
interrupt the Grave Drain. I stood in front of the Skeleton Warrior. The Ghoul's bleed stacked
too high.* Every one of those is a telegraph, a cast bar, a cone or a stack count on screen.

---

## Reset

**Dying does not undo progress.** When the player comes back (the automatic respawn, or
`BACKSPACE` while dead), every monster **still standing** goes home, whole and forgetful; every
monster the party **killed stays dead**; completion keeps what was credited, and an opened gate
stays open. A boss fight in progress is abandoned: his adds are despawned and he is made whole,
to be pulled again with a fresh pair.

`BACKSPACE` pressed while **alive** is the development reset: the whole run back — every monster
restored, completion 0, the gate shut, the boss sealed. Leaving the editor with `F6` rebuilds
the monsters from the edited level.

---

## Data, and adding monster #7 or dungeon #2

| What | Where |
|---|---|
| Monster stats, abilities, cooldowns, casts, shapes, targeting, riders, completion worth, empowered form, look | `engine-game/src/enemy/mod.rs`: `ENEMY_ABILITIES` and `ARCHETYPES` rows |
| Placement, packs, per-placement level/aggro/worth | the level file's `monsters` |
| Requirement, boss pack, gate, add spawns, add count, add pool, duplicates | the level file's `dungeon` |
| Party scaling | `engine_game::dungeon::DungeonScaling` |

**Monster #7:** add an `EnemyKind` variant, a row in `ARCHETYPES` (with an `id`), and rows in
`ENEMY_ABILITIES` for any new attacks. It appears in the editor's Monsters folder, can be placed,
saved and spawned, and can join a boss's `add_pool` — no other code changes.

**Dungeon #2:** a new level file with `monsters` and a `dungeon` section, made in the editor. It
is loaded with `--scene-file` and becomes a dungeon run through the same path.
