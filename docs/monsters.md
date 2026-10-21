# Monster models, animation and the monster playground

Every monster in the game is drawn as an animated character with its own stand-in model. The
Monster Combat Playground is a test scene for fighting any combination of them on demand.

## The models

`engine-assets/src/procedural/monsters.rs` builds seven models on the shared 21-bone humanoid
rig (`procedural/humanoid.rs`). They are made the same way as the heroes: prisms, generated
textures, no files.

| Model | Drawn for | Silhouette | Attack style |
|---|---|---|---|
| `skeleton_bonecaster` | Skeleton Bonecaster, Empowered Bonecaster, Ember Adept, Cinder Weaver | bare bones, cowl, mantle, a staff with a burning skull | Channel |
| `crypt_archer` | Crypt Archer, Empowered Archer | bones, leather hood, quiver, a bow in the left hand | Bow |
| `skeleton_warrior` | Skeleton Warrior, Empowered Warrior, Ash Revenant | bones, rusted helm, pauldron, mail skirt, sword and shield | Swing |
| `grave_acolyte` | Grave Acolyte, Empowered Acolyte, Bone Ritualist, Crypt Warden | a floor-length robe and tall hood with violet eyes, a censer | Channel |
| `grave_brute` | Grave Brute, Empowered Brute, Brute | a hunched, stitched giant with shackled fists | Slam |
| `crypt_ghoul` | Crypt Ghoul, Empowered Ghoul | crouched and starved, with long claws | Claw |
| `lord_ashgrave` | Lord Ashgrave, Warden of the Ashen Gate | horned crown, ember-lit plate, cape, greatsword | Swing |

A monster's table row names its model in `MonsterLook::model`. Empowered forms share their
base model and keep their glow; the Ashen Reliquary's monsters borrow the crypt's models
through `MonsterLook::hostile`. These are temporary stand-ins. The loader does not yet check
disk for an exported `.glb` the way it does for heroes. Replacing a stand-in means building
a different `ModelAsset` under the same name.

Each model has one level of detail and a few hundred triangles; a test holds each one under
1,500.

## Animation

Monsters use the heroes' cycles (idle, walk, run, jump, fall, land, cast, cast_heavy) with
their own amplitudes. On top of those they get three additions:

- **Posture** (`humanoid::Posture`): offsets laid over every clip, such as a hunch, a crouch,
  arms hanging forward or the head tipped up. The brute's hunch and the ghoul's crouch come
  from this, not from different cycles. A crouch bends the knees just far enough to keep the
  feet under the hips, and lowers the hips by the height that costs.
- **Attack styles**: `Bow` (the bow arm turns forward, the forearm twists the bow upright and
  the draw goes to the cheek; the heavy version aims into the sky), `Slam` (both fists
  overhead, then down), and `Claw` (a lunge and a rake; the heavy version pounces). These sit
  beside the existing `Swing` and `Channel`. Every cast clip marks the moment the blow lands
  (`humanoid::release_times`).
- **Death** (`humanoid::death`): the knees buckle, the body slumps, and it falls back to lie on
  the floor, holding the last frame. It uses the new optional `AnimationSlot::Death`.

### Driving it

`engine-presentation/src/monster_visual.rs` keeps one `CharacterVisual` per enemy. It uses the
same state machine as the player, fed from the enemy:

- body position, facing and speed (walk and run are chosen by speed);
- the brain's current cast, placed at its progress so the blow lands when the cast does;
- `CastStarted` and `AbilityUsed` events, read in one pass per frame;
- whether it is dead.

An instant attack plays its whole clip, wind-up included (`CharacterVisual::with_whole_instants`),
and is not cut short when the monster steps. A hero's instant still plays only its
follow-through. A new controller cuts straight to its first clip instead of fading in from
the T-pose. When a fight is reset, a corpse stands back up into its idle.

Monsters with a model are skinned instances and hold no `RenderWorld` slot. An enemy whose
model name has no model behind it is still drawn as the tinted dummy. Tinting on a model is a
20% wash towards the table colour. Hit flash, target glow and empowered glow still apply;
corpses dim rather than turning grey.

## The Monster Combat Playground

```
cargo run --release -- --scene playground        # or --scene monsters
```

An open floor with one dummy (for scale), a player with **1,000,000 health**, and two docked
panels. The HUD is laid out in the space between them (`HudLayout::set_reserved`), so the fight
stays in the middle of the screen; `L` hides the panels and gives the space back.

**Left: Monster Library.** Every monster in `EnemyKind::ALL`, grouped by tier (monsters,
empowered, bosses), discovered from the table rather than listed: a new row is in the library
the moment it compiles. Each row is a checkbox; click any number of them. `Ctrl+F` or a click
on the field searches name, id, role and tier. The details card under the list shows the
monster under the pointer (or the last clicked): level, role, health at that level, its
summary, and every ability it can use in any phase with cast time and cooldown, read from its
table row. The footer counts the selection and holds **SPAWN SELECTED**.

**Right: Playground.** Test or Normal HP, Reset player, passive spawns and the spawn level,
then the **current encounter**: each spawned monster with its health, what its brain is doing
(`Casting Bone Arrow`, `Chasing`, `Stunned`, `Dead`) and a click that targets it. **Kill
all** and **RESET ENCOUNTER** sit underneath.

| Control | Action |
|---|---|
| click a row, Up / Down + Right | choose or drop a monster |
| Ctrl+F | search the library (Enter or Escape stops typing; while typing no key is play) |
| Enter, **SPAWN SELECTED** | replace the encounter with every chosen monster, in formation |
| Delete, **RESET ENCOUNTER** | every playground monster goes; the selection stays |
| K, **Kill all** | kill the encounter through the damage path, so each death plays |
| P, **Passive** | new spawns wait until you hit them |
| **Level - / +** | the level new spawns are (health and damage scale like a level placement) |
| **Test HP / Normal HP** | 1,000,000 (`--test-health <n>`) or the character sheet's own maximum |
| **Reset player** | full health and resource, every cooldown ready, no buffs, debuffs or cast |
| click an encounter row | target that monster (the same command a nameplate click sends) |
| H | interrupt your target's cast |
| L | hide or show the panels |
| Backspace | the scene's ordinary reset: monsters back home and whole, you at full health |

### Effects mode

The **Effects** tab at the top of the left panel turns the playground into a buff and debuff
lab. Nothing in it is an effect of its own: it applies, removes and reads the global registry
(see `auras.md`, "The effect registry").

- **Left — Effect Library.** Every row of the registry, grouped BUFFS and DEBUFFS, each with
  its icon and its dispel school in that school's colour (`system` for respawn protection,
  which nothing outside the respawn system may apply). The card shows what it is, whether it
  can be dispelled, its stacks, its tooltip sentence and, line by line, what it changes.
  **TIME** steps through 1 s to 10 min, or click the value and type seconds; **STACKS** stops
  at the effect's ceiling; **APPLY TO …** applies it to the effect target.
- **Right — Effect testing.** The targets: you, the friendly Healing Dummy, the training
  dummy and every living monster of the encounter (a dead or reset monster drops out and the
  target falls back to you). Clicking one also makes it your game target, so the target frame
  and a healer's next spell point there. Below: **STATS** from `Simulation::effect_stats`
  (the sheet with each effect's change in brackets for the player, then movement, attack
  speed, damage dealt, damage taken and reduction, tinted better or worse), then **BUFFS**
  and **DEBUFFS** as separate lists, one line per effect: the same icon widget the plates use
  (blue border when dispellable), name, `×stacks`, time left counting down on the game clock
  (amber under five seconds), and **×** to remove it. Every status is listed, uncapped; past
  twenty a line says the HUD shows twenty.
- **Add test effects** puts every applicable effect on the target at once, staggered from 6 s
  to 5 min, stackables at full stack. **Clear buffs / Clear debuffs** remove by class.
- **Hero tabs** (Fire Mage, Warrior, Druid) under Reset player swap the hero, so a dispel is
  tested with the Druid's real Nature's Cleanse on the bar: debuff yourself or the Healing
  Dummy, target it, press the cleanse. It costs its mana and its eight seconds of cooldown as
  anywhere else; a bleed stays.
- **Reset player** clears every effect on you and every stat it changed. **Reset
  encounter** takes the monsters with every effect on them, and every effect they had put on
  anybody.

`--effects fortitude,poisoned` (or `all`) opens Effects mode with those on you, for
screenshots.

### How it is built

The playground orchestrates; it does not fight. `engine_game::playground` owns the library,
the selection, the encounter's membership (by entity) and the health mode. Everything else
is the game's own:

- a spawn is `Simulation::spawn_enemy_with`: the same brain, threat, abilities, telegraphs,
  nameplate, cast bar and death as a dungeon pack. Non-passive spawns notice the player at
  once, as a pack does when walked into;
- the formation (`playground::formation`) puts melee in front, ranged behind, rows sized to
  each body's radius with a 1.2 m gap, everybody facing the player; each slot is then put on
  walkable ground the player can walk to (the navigator's own columns and line test), moving
  outwards in rings when a wall, a pit or cover is in the way;
- a reset is `Simulation::dismiss_enemies`: each monster through `despawn_enemy` (cast ended
  with an event, telegraphs, hazards and marks down, target dropped), then every status it
  applied to anybody removed and a player cast aimed at it cancelled. Alive, dead, casting,
  stunned or mid-charge makes no difference. The meter restarts. Nothing outside the
  encounter is touched;
- test health is `Simulation::set_player_health_override`; reset player is
  `Simulation::reset_player_state`;
- a boss's summoned adds are adopted into the encounter from `AddSummoned`, so a reset takes
  them too; a member something else removed (an editor reload) is forgotten the same frame.

The panels (`engine_presentation::ui::monster_playground`) are pure interface: they answer
input with a `PlaygroundAction` the application carries out. Names, roles and ability lines
are formatted once when the panels are built; a frame draws them without allocating.

For screenshots and automated checks:

```
mygame --scene playground --spawn skeleton_warrior,grave_brute,crypt_steward [--spawn-hostile]
mygame --scene playground --spawn all
```

`--spawn` chooses monsters by table id and spawns them through the playground, passive unless
`--spawn-hostile` is given.

## The Drowned Catacombs roster

Five more archetypes, drawn with existing models and a teal tint (see
[`dungeon/DROWNED_CATACOMBS.md`](dungeon/DROWNED_CATACOMBS.md)):

| Id | Drawn as | Role |
|---|---|---|
| `drowned_tidecaller` | `skeleton_bonecaster` | Support: interruptible Tidal Mend on the most wounded ally |
| `drowned_cultist` | `grave_acolyte` | Ranged: Brine Bolt leaves the dispellable Waterlogged |
| `catacomb_lurker` | `grave_brute` | Elite: patrols; Tidal Cleave (wide cone) |
| `bilge_archer` | `crypt_archer` | Ranged: Bilge Volley (delayed circles on three players) |
| `drowned_abbess` | `grave_acolyte` | Boss: Undertow, Drowning Hymn, healers at 50 % |

## The Bone Foundry roster

Five more archetypes, drawn with existing models and forge colours (see
[`dungeon/BONE_FOUNDRY.md`](dungeon/BONE_FOUNDRY.md)):

| Id | Drawn as | Role |
|---|---|---|
| `slag_ironclad` | `grave_brute` | Melee shielder: interruptible Slag Bulwark wards its pack (Warded, 40 % less damage) |
| `cinder_bomber` | `crypt_ghoul` | Melee suicide: Cinder Blast, a drawn fuse and circle, then it dies |
| `slag_thrower` | `skeleton_bonecaster` | Ranged: Slag Lob (delayed circles on two players that leave burning slag) |
| `foundry_foreman` | `grave_acolyte` | Elite: interruptible Stoke the Furnace (Stoked, 30 % more damage), Chain Lash |
| `foundry_overseer` | `grave_brute` | Boss: Slag Brand (spread), Molten Ladle (soak), Conveyor Surge (lane), Overheat |

## The Sanctum of Ash roster

Seven more archetypes, drawn with existing models in ash and ember colours (see
[`dungeon/SANCTUM_OF_ASH.md`](dungeon/SANCTUM_OF_ASH.md)):

| Id | Drawn as | Role |
|---|---|---|
| `mirror_penitent` | `skeleton_warrior` | Melee: interruptible Mirror Ward (Mirrored, half of damage taken sent back at the attacker, capped), Penitent Blade |
| `ash_cantor` | `grave_acolyte` | Support: Litany of Ash (stacking dispellable Ashen Curse on two players), Ashen Mend |
| `ash_stalker` | `crypt_ghoul` | Melee: Ambush Leap at the player with the least health left, bleeding Slash |
| `pyre_immolator` | `grave_brute` | Melee: Pyre Slam; its death leaves Ember Collapse, a blast and a fire that spreads |
| `ember_pylon` | `grave_acolyte` | Called by the boss, never placed: rooted, Ash Tithe pulses the sanctum, holds the boss immune while it stands |
| `censer_warden` | `grave_brute` | Elite (mid-dungeon guardian, own loot table): Smoke Veil, Litany of Ash, Censer Swing, Censer Blaze at 50 % |
| `ashen_pontiff` | `grave_brute` | Boss: Branding Strike (stacking Branded), Rite of Cinders (line of sight), pylons at 50 %, Last Rite |

## Cost

Measured in the Graveyard Crypt, release build, hidden 1920×1080, 6,000 frames, with all 37
monsters animated:

| | Before (tinted dummies) | After |
|---|---|---|
| CPU mean | ~0.29 ms | 0.367 ms |
| GPU mean | ~0.24 ms | 0.252 ms |
| Skinned draws | 1 character | 173 in view |

"Before" is the measurement recorded in `docs/dungeon-map.md`. Animating every monster costs
about 0.03 ms of extraction. The rest is one draw per submesh with a palette upload per
character in each pass. Uploading palettes once per frame instead of once per shadow cascade
is the next saving if a bigger dungeon needs it.

## Tests

- `engine-assets` `procedural::monsters` (6 tests): every model validates like an import and
  has 9 clips; every definition resolves, including Death; feet on the floor; no two models
  share a silhouette; crouch geometry; unknown names return `None`.
- `engine-presentation` `monster_visual` (3 tests): a standing monster is posed from its first
  frame (no T-pose); the boss's blade points the way it was authored; every monster dies onto
  the floor and gets back up after a reset.
- `engine-game` `playground` (3 tests) and `tests/monster_playground.rs` (21 tests):
  selection (one, several, deselect, clear); formations never overlap, stand in front and
  face the player for 1-20 bodies; spawning one, several and the whole library with unique
  entities and no overlap; a slot inside a wall moves to reachable ground; passive and
  hostile spawns; level scaling; the monsters land real hits; reset removes every
  playground monster (alive or dead) and nothing else; reset mid-fight leaves no status,
  cast, hazard, combat or damage behind; stale members are forgotten; select, spawn, reset
  repeated twelve times leaks no entity; test and normal health; reset player; a dead
  player is brought back.
- `engine-presentation` `ui::monster_playground` (9 tests): every registered monster is
  listed under its tier; clicks toggle rows, spawn, target and reset; the player controls;
  search typing filters and clears; the keys; the panels fit 3 resolutions × 2 scales and
  leave the middle to the fight; hiding returns the pointer and the HUD space; the drawn
  text includes the details card's real abilities; thousands separators.
- `engine-presentation` `ui::hud_layout`: a reserved edge moves the HUD into what is left,
  keeps a drag round-tripping, and gives the space back.
- `engine-game` sim: the health override survives refreshes and a Warrior swap, and can be
  released.
- `engine-app` args: `--scene monsters` and its aliases (`playground`), `--spawn`,
  `--spawn-hostile`, `--test-health`.
