# Heroes

Three playable characters — FireMage, Warrior and Druid (in human form) — one ability table,
one executor, and a swap in the pause menu that changes what you are holding without
touching the world you are standing in.

## What a hero is

A **slice of the one ability table**, and nothing else.

```text
Hero ─┬─ abilities()  the rows of ABILITIES whose `hero` column names it, in bar order
      ├─ bar()        those rows on the digit keys; empty in the slots it does not fill
      ├─ model_name() a name the visual layer looks up. `engine-game` never sees a triangle
      └─ ultimate()   the ability on `R`: Cataclysm (disabled), Last Stand, or none
```

`engine-game/src/hero.rs`. Every row of `ABILITIES` carries `hero` and `damage_type`; no two
heroes share a row, which is what lets `FireAbility::bar_slot()` still answer from the ability
alone and what keeps every button, tooltip and meter unaware that a second class exists.

A second table would have been a second set of cooldown rules, a second global cooldown, a
second tooltip generator and a second thing for the damage meter to learn.

## The abilities

| hero | 1 | 2 | 3 |
| --- | --- | --- | --- |
| FireMage | *ten, unchanged* — see `firemage.md` | | |
| Warrior | *eleven* — see `warrior.md` | | |
| Druid | MOONFIRE — 30 m, 1.0 s cast, 160 % | STARFALL — 8 m circle, 6 s, 60 %/tick | REGROWTH — self heal, 400 %, 12 s |

Nothing in the executor branches on who is casting. The Warrior and the Druid generate no
Heat and apply no Ignite because their rows say zero, and a test asserts that against a live
fight rather than against the table. Three mechanisms were generalized rather than copied:

* **Movement** — Blink is `dash(distance)`; the distance is the row's range. The Warrior's
  Shield Charge is deliberately *not* the same thing — it is a run through the collision
  world, steered a step at a time. See `warrior.md`.
* **Placed zones** — Starfall is the `Zone` Firestorm and Molten Meteor's ground already are,
  with a circle instead of a line.
* **Healing** — Regrowth is the first heal a *player* can cast. It goes through
  `GameWorld::heal` and the `Healing` event exactly as an enemy's does.

## The models

`engine-assets/src/procedural/`. Both new characters are built from the same tapered prisms
the FireMage is, for the reason that module gives: a technical baseline with no licence
attached, cheap enough to be free, real enough to build skinning, animation, materials and
LOD against.

```text
humanoid.rs   the shared rig: 21 bones, T-pose, feet at y = 0, facing -Z
              SkinBuilder, and eight clips generated from a `Style`
warrior.rs    geometry + textures + materials + a Style. Nothing else.
druid.rs      likewise.
firemage.rs   keeps its own copy — see below.
```

A `Style` may change **amplitudes and rest poses, never structure**: every character walks
with the same knee rule, the same two bobs per cycle and the same counter-swinging arms,
because those are facts about walking rather than facts about a class.

Each model is arranged around what survives to thirty metres:

* **Warrior** — pauldrons wider than the hips, a flat-topped helm, a blade held out from the
  body. All three are kept at LOD1; the arm plate, gauntlets, belt and separate knee are not.
  A test asserts the shoulders are wider than the hips, because a tuning pass that narrowed
  the pauldrons would take the class's readability with it.
* **Druid** — antlers where the mage has a hood and the Warrior a helm, a narrow silhouette,
  and a green glow at the head of the staff, which is the model's only emissive surface.

`firemage.rs` keeps its own rig rather than moving onto `humanoid.rs`. It was written first,
its clips are authored around a staff held across the body, and its tests assert on the
geometry it produces; rebuilding it would be a change to the one character that already works,
for no gain the player can see. When artists' exports replace the three, the duplication goes
with them.

## Swapping

`ESC → SWAP HERO`. One ticked row per hero with a line saying what it is, built from
`Hero::ALL`, so a fourth playable character is a row in that array.

```text
MenuAction::HeroSelected
        │
        ▼
Simulation::set_hero        the abilities, and a clean class state
        ├── HeroVisuals::visual     the model, already uploaded at startup
        ├── ActionBarSet::rebind    bar one; bars two to eight are the player's own
        ├── SlotMenu::set_hero      the spell picker offers this hero's spells
        └── MenuSystem::set_hero    the tick, reported back from the game
```

**The world is untouched.** Position, health, target and the fight all survive: a swap is a
change of what you are holding, not a reload, and treating it as one would make it useless as
the thing it is for — trying three classes against the same dummies.

**What the old hero had open does not survive.** The cast in progress, the cooldowns, the burn
it banked and the fire it left on the ground all go. A mage's three-second Pyroblast landing
after its caster became a Warrior is the one outcome that is certainly wrong.

All three models are uploaded at startup rather than on the swap: a `create_model` in the
middle of a frame the player is looking at would be a hitch, and three prism characters are
cheaper than one.

## Known limitations

* **No hero is persisted.** Every session starts as the FireMage. Bar one's *arrangement* is
  saved, so the bar comes back as whatever hero last wrote it, and the swap rewrites it.
* **The Druid has no resource.** The FireMage's Heat and Ignite sit at rest while it is worn;
  mana is not designed. The Warrior has Deflection — see `warrior.md`.
* **They borrow the mage's sounds and effects rather than having their own.** The stand-ins are chosen by
  shape — a swing is an impact, a placed circle is a placed area — because silence would read
  worse than a sound of the right length.
* **No talents outside the FireMage.** The talent grid is the FireMage's.
* **No animal forms.** In human form the Druid is the same rig as the Warrior, which is the
  property `humanoid.rs` exists to demonstrate; a bear would be a different skeleton, a
  different set of cycles and a second character in everything but the name.
