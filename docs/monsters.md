# Monster models, animation and the monster lab

Every monster in the game is drawn as an animated character with its own stand-in model. The
monster lab is a test scene for looking at any of them on demand.

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

## The monster lab

```
cargo run --release -- --scene monsters
```

This loads a flat floor with one dummy (for scale) and a player with **1,000,000 health**.
`Simulation::set_player_health_override` holds that value through gear refreshes, hero swaps
and the Warrior's multipliers. The panel on the right lists every monster in the game:

| Control | Action |
|---|---|
| click a row, Up / Down | choose a monster |
| Enter, **Spawn** | spawn it in front of you (bosses spawn farther away) |
| **Spawn 3** | three, fanned out |
| P, **Passive** | new spawns ignore you until you hit them, for looking at the idle |
| K, **Kill all** | kill every monster through the normal damage path, so each death plays |
| Delete, **Clear** | remove every monster |
| H | interrupt your target's cast |
| L | hide or show the panel |
| Backspace | reset: monsters back home and whole, you at full health |

The status line shows how many monsters are alive, dead and animated, and which animation the
most recent spawn is playing.

For screenshots and automated checks:

```
mygame --scene monsters --spawn skeleton_warrior,grave_brute,lord_ashgrave [--spawn-hostile]
mygame --scene monsters --spawn all
```

`--spawn` lines monsters up, by table id, in front of the player. They are passive unless
`--spawn-hostile` is given.

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
- `engine-presentation` `ui::monster_lab` (8 tests): every enemy kind is on the roster once;
  keyboard stepping skips headings and wraps; clicks select and spawn; keys spawn, kill,
  clear and toggle passive; the layout fits at 3 resolutions × 2 scales and owns the pointer;
  hiding releases the pointer; the text uses the interface font; thousands separators.
- `engine-game` sim: the health override survives refreshes and a Warrior swap, and can be
  released.
- `engine-app` args: `--scene monsters` and its aliases, `--spawn`, `--spawn-hostile`.
