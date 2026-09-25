# Character animation

How a character model gets the game's animations: a model rigged on Mixamo, the animation
downloads, and one Blender script that turns them into the `.glb` the game loads. This is how
`assets/firemage.glb` is built, and it is the route for every further character — the only
per-character work is rigging the model.

The code the animations drive is described where it lives: `engine_assets::character`
(slots and clip names), `engine_presentation::character` (which slot plays when),
`engine_assets::mirror` (left/right mirroring).

---

## 1. The short version

For a new hero, say the warrior:

1. Export the model from Blender as **FBX** and upload it to [Mixamo](https://www.mixamo.com).
   Auto-rig it.
2. With the character selected, download any animation as **FBX Binary, With Skin**. That
   file is the character: save it as `assets/characters/<hero>.fbx`, beside the mage's
   `assets/characters/firemage.fbx`.
3. The animation downloads (section 4) are already in `assets/animations/` — the same files
   the mage was built from. They do not have to be downloaded again for a new character.
4. Run the script:

   ```bash
   "C:/Program Files/Blender Foundation/Blender 5.2/blender.exe" -b --factory-startup \
     --python tools/blender/mixamo_character.py -- \
     --character assets/characters/warrior.fbx \
     --out assets/warrior.glb --blend assets/warrior.blend
   ```

5. Start the game. The log should say
   `warrior: imported warrior.glb - … 12 clips, … m tall` and list the optional clips it
   found. (The prefix is the hero's model name: `fire_mage`, `warrior`, `druid`.)

That is all. The hero picks the file up by name (section 6); nothing in the code changes.

---

## 2. Preparing the model

* **One skinned mesh, standing, arms out or down.** A T-pose rigs most cleanly. Mixamo's
  auto-rigger places the joints from markers you drop on the chin, wrists, elbows, knees and
  groin.
* **FBX or OBJ.** Mixamo does not accept `.glb`. Blender: *File → Export → FBX*.
* **Scale does not matter.** A model authored one unit tall is read by Mixamo as one
  centimetre; the script rescales to `--height` (default 1.8 m).
* **Facing does not matter** as long as Mixamo shows the character facing you. The script
  turns every rig to face the engine's forward (−Z).
* **Props held in a hand get rigged as fingers.** The mage's staff is the right hand's
  "index finger", 0.64 m long. It animates, but it follows the finger keys, so a cast pose
  that curls the fingers bends the staff. If a prop matters, separate it and attach it in
  code later rather than letting the auto-rigger guess.

---

## 3. What the script does, and why

`tools/blender/mixamo_character.py` runs headless in Blender. For each animation it:

| Step | Why |
| --- | --- |
| Plays the animation on the rig it was downloaded with, and makes the character's bones follow it **in world space**, then bakes the result. | Mixamo's keys are local to each bone and only mean the same thing on the rig they were made for. An animation downloaded while another character was selected crosses the arms if copied key for key. |
| Scales the character to `--height`, turns it 180°, puts its feet on the origin. | Mixamo rigs come in centimetres and face +Z in glTF; the engine is metres and −Z. |
| Stands each animation's lowest toe position on the character's floor. | A rig's rest pose is not standing height, so the hips are no guide to where the ground is. |
| Removes horizontal travel from clips that are not *In Place*, and records the speed they were authored at. | The game moves the character; the clip's own travel would double it. The recorded speed is what playback is scaled against, so the feet keep pace. |
| Caps the hips at standing height in `jump` and `fall`. | The game lifts the character when it jumps; Mixamo's jump lifts it too. |
| Writes each clip from time zero, and a one-frame pose as two keys. | A loop that starts at 1/30 s hitches every cycle. |
| Stores `release` (cast clips) and `speed` (travelling clips) as glTF extras. | The engine reads them: see section 5. |

Blender's glTF exporter is run in **Actions** mode. The NLA-tracks mode drops the extras.

---

## 4. The animation table

The `CLIPS` table at the top of the script. Clip names are what the game binds; file names
are the Mixamo downloads in `assets/animations/` (or wherever `--animations` points). A file
that is not there is skipped. A new animation goes into that folder and gets a row here.

| Clip | File | Frames | Needed | Without it |
| --- | --- | --- | --- | --- |
| `idle` | `Idle.fbx` | all | yes | bind pose |
| `walk` | `Walking.fbx` | all | yes | the run, slowed |
| `run` | `Fast Run.fbx` | all | yes | bind pose |
| `jump` | `Jumping.fbx` | 14–25 | yes | `fall` |
| `fall` | `Jumping.fbx` | 27 (held) | yes | bind pose |
| `land` | `Land.fbx` | 6–12 | yes | `fall` |
| `cast` | `Cast.fbx` | all | yes | bind pose |
| `cast_heavy` | `CastHeavy.fbx` | all | yes | `cast` |
| `walk_back` | `Walking Backwards.fbx` | all | no | the walk, reversed |
| `run_back` | — | — | no | the run, reversed |
| `strafe_left` | `Left Strafe.fbx` | all | no | mirrored `strafe_right`, else the forward cycle |
| `strafe_right` | `Right Strafe.fbx` | all | no | mirrored `strafe_left`, else the forward cycle |
| `death` | `Death.fbx` | all | no | the character stays standing |

**Downloading from Mixamo:** FBX Binary, **Without Skin**, 30 fps. *In Place* is optional —
the script handles either.

**Frame ranges are per file, not per character.** `jump` is the end of the crouch to the top
of the jump in *this* `Jumping.fbx`; a different jump download needs its own numbers. To find
them, look at where the toes leave the ground and where they peak (the game's own jump rises
for 0.32 s). `land` starts at touchdown, because the frames before it hang the feet in the air.

**Mirroring:** delete the `strafe_right` line and the engine mirrors `strafe_left` when the
model loads. Mixamo's own *Right Strafe* is not quite the mirror image — it starts half a
stride later — which does not matter for a loop.

**Diagonals need no clip.** A forward or backward diagonal plays the forward (or backward)
cycle with the drawn body turned 45° into the diagonal and the spine, neck and head turned most
of the way back (`CharacterVisual::stride_yaw`, see [auras.md](../auras.md#the-diagonal-run)).
The twist finds bones by name — `Spine`, `Spine1`, `Spine2`, `Neck`, `Head` with any `rig:`
prefix — so keep those names when retargeting.

---

## 5. Per-character numbers

Most of a character is shared. What is not:

* **`RELEASE`** in the script: seconds into `cast` and `cast_heavy` at which the spell leaves
  the hands (0.65 and 1.0 for the mage's clips). The game places the cast clip at the cast
  bar's progress so this moment lands as the cast completes, then plays the rest. A character
  with different cast downloads needs these re-measured — watch the hands in the `.blend`.
* **`--height`**: how tall it stands. Gameplay scales the model to its own capsule anyway;
  this only keeps the file sane.
* **The character definition** in code (`fire_mage_character`, `warrior_character`,
  `druid_character` in `crates/engine-assets/src/procedural/`): which clip name fills each
  slot, and the walk/run speeds that pick walk or run. `with_directional_clips()` asks for
  the four optional clips by their conventional names.

---

## 6. Where the game looks

| Hero | File | Until it exists |
| --- | --- | --- |
| Fire mage | `assets/firemage.glb` | the procedural stand-in |
| Warrior | `assets/warrior.glb` | the procedural stand-in |
| Druid | `assets/druid.glb` | the procedural stand-in |

A file that is present but fails to load is a startup error, not a silent fall back.
`assets/` is not in git (see `assets/README.md`), so **back up `assets/animations/` and
`assets/characters/`**: they, not the `.glb`, are the source. The `.glb` can always be rebuilt
from them. A lost character file can be recovered by rigging the model on Mixamo again; the
script retargets onto whatever rig it is given.

The mage, rebuilt:

```bash
"C:/Program Files/Blender Foundation/Blender 5.2/blender.exe" -b --factory-startup \
  --python tools/blender/mixamo_character.py -- \
  --character assets/characters/firemage.fbx --out assets/firemage.glb
```

---

## 7. Checking the result

* `cargo run -p engine-assetcheck --bin asset_check -- assets/warrior.glb` — budgets and rig.
* The startup log: triangle count, bones, clips, height, which optional clips were found,
  and whether a strafe was mirrored.
* `cargo run -p engine-app -- --scene character`, then hold the character in one animation:
  `1` idle · `2` walk · `3` run · `4` jump · `5` cast · `6` heavy cast · `7` fall ·
  `8` strafe left · `9` strafe right · `0` release.
* The delivered-model test, `the_delivered_firemage_loads_within_every_runtime_budget` in
  `engine_assets::disk`, plays every clip of the installed `firemage.glb` and checks the
  character keeps its height. It is written for the mage; a second hero wants its own.

---

## 8. Things that went wrong once, and what they looked like

| Symptom | Cause |
| --- | --- |
| Character stands still in a T/A-pose | Clip names do not match the definition. Mixamo names every clip `mixamo.com`. |
| Faces backwards | glTF's forward is +Z, the engine's −Z. The script turns it. |
| 100× too big and lying down, only while animating | The rig's `Armature` node carries the unit conversion and an up-axis turn; the importer now applies it to animation keys as well as the rest pose. |
| Arms crossed, legs fine | Animations downloaded for a different Mixamo character, copied key for key. Retargeting fixes it. |
| Floats a metre up while animating | Aligned on the rest pose's hips instead of the animation's ground. |
| Jumps twice as high | Mixamo's jump lifts the hips; the game lifts the capsule. |
| Feet slide | Clip played at a rate that does not match the movement speed, e.g. a 1.4 m/s backwards walk at 4.9 m/s backpedal. |

---

## 9. Later: one animation library for every character

The route above bakes every animation into every character's file. The alternative, not
built yet: one `assets/animations/humanoid.glb` holding the clips on a reference rig, and the
engine retargeting them onto each character's skeleton at load, matching bones by name (a
small name map for non-Mixamo rigs), with per-character clips overriding the library. An
animation is then added once for every character, and a character file is just mesh and rig.
The math is the same as `engine_assets::mirror` without the reflection; the requirement is
that characters are rigged in the library's rest pose (T-pose).
