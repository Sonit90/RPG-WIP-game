# assets

Art the game loads at startup. **Nothing in this directory is in git** except this file: a
model is tens of megabytes of binary that git cannot pack and cannot ever remove from history
once committed, so exports live beside the repository rather than inside it.

A checkout with an empty `assets/` builds and runs — every model has a procedural stand-in, and
`engine_assets::disk::find_model` treats a missing file as "use the procedural one". A file that
*is* here and fails to load is a startup error, so a mis-exported file never looks like one
nobody has delivered yet.

## Where the game looks, in order

1. `$MYGAME_ASSETS`, for pointing a build at a content directory somewhere else entirely
2. `assets/` beside the executable — what a packaged build ships
3. `./assets` in the working directory — what `cargo run` finds
4. this directory, so a model is found however the binary was launched

## What goes here

| File | What it replaces |
|---|---|
| `firemage.glb` | The fire mage, `procedural::fire_mage` |
| `warrior.glb` | The warrior, `procedural::warrior` |
| `druid.glb` | The druid, `procedural::druid` |
| `animations/*.fbx` | Nothing the game reads: the Mixamo animation downloads every character's `.glb` is built from |
| `characters/<hero>.fbx` | Nothing the game reads: each hero rigged on Mixamo, the other input to its `.glb` |
| `scenes/*.json` | A level's layout function. Load one with `--scene-file` |
| `*.glb` layouts | The same, authored in Blender. Load one with `--layout` |

## Authoring a level

Two routes, one destination — an `EnvironmentScene`, which is what the renderer places and
what gameplay builds its collision from.

**In the game.** Build with `cargo run -p engine-app --features editor`, press `F6`, and place
objects from the palette. `CTRL+S` writes `assets/scenes/<scene name>.json`. Play it with
`--scene-file assets/scenes/<name>.json`. The editor is behind a Cargo feature, so a player's
build has none of it.

**In Blender.** Build the level out of stand-ins and name each object after the asset it
stands for — `pillar`, `pillar.001`, `Collection:wall_segment.042` all resolve to the library
asset, case-insensitively, with Blender's duplicate suffix cut at the dot. Export one `.glb`
and load it with `--layout assets/<name>.glb`.

Only **names and transforms** are read out of a layout export — never its geometry. That is
what keeps one upload per distinct asset however many times it is placed, a level of detail
per object, and a cull per object. A node whose name matches nothing is skipped and counted in
the startup log; `spawn_player`, `spawn_featured_dummy` and `spawn_group` move the scene's
spawn points instead of placing anything.

Collision comes from the library's own table (`engine_environment::object::default_collision`),
so a wall placed either way blocks without the author saying so. A scene file can override it
per object; a layout export cannot, which is the one thing the file format does that Blender
does not.

## Exporting a model this importer accepts

* **`.glb`, or a `.gltf` with everything embedded.** Sidecar `.bin` and image files are refused
  with a message saying so — there is no asset resolver inside the importer.
* **PNG or JPEG textures.**
* **At most four influences per vertex.** `JOINTS_1` is refused rather than truncated, because
  dropping the fifth influence unnormalizes the weights and deflates the mesh.
* **No cubic spline interpolation** — refused rather than resampled, which would change the
  timing the animator authored.
* Morph targets, cameras, lights and extensions are ignored; the meshes still load.
* Several skins over one hierarchy are fine — they are merged into one skeleton.
* Budgets: 100k triangles per LOD, 16 materials, 32 submeshes per LOD, 128 bones. Textures over
  2048 are halved until they fit rather than refused.

**Animation clips are matched by name.** A character binds the clips `idle`, `walk`, `run`,
`jump`, `fall`, `land`, `cast` and `cast_heavy`, and optionally `walk_back`, `run_back`,
`strafe_left` and `strafe_right`; a required name the export does not carry falls back to the
bind pose, and the startup log says how many were found. An export with no usable clips
renders as a character standing still, which is a delivery state, not a bug.

**Building a character from Mixamo:** `docs/character-animation.md`. One script,
`tools/blender/mixamo_character.py`, turns a rigged model and the animation downloads into
the `.glb` above.
