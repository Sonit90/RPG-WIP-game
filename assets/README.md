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
| `firemage.glb` | The player character, `procedural::fire_mage` |

## Exporting a model this importer accepts

* **`.glb`, or a `.gltf` with everything embedded.** Sidecar `.bin` and image files are refused
  with a message saying so — there is no asset resolver inside the importer.
* **PNG textures only.** A JPEG-textured export is refused.
* **At most four influences per vertex.** `JOINTS_1` is refused rather than truncated, because
  dropping the fifth influence unnormalizes the weights and deflates the mesh.
* **No cubic spline interpolation** — refused rather than resampled, which would change the
  timing the animator authored.
* Morph targets, cameras, lights and extensions are ignored; the meshes still load.
* Several skins over one hierarchy are fine — they are merged into one skeleton.
* Budgets: 100k triangles per LOD, 16 materials, 32 submeshes per LOD, 128 bones. Textures over
  2048 are halved until they fit rather than refused.

**Animation clips are matched by name.** A character binds the clips `idle`, `walk`, `run`,
`jump`, `fall`, `land`, `cast` and `cast_heavy`; a name the export does not carry falls back to
the bind pose, and the startup log says how many were found. An export with no usable clips
renders as a character standing still, which is a delivery state, not a bug.
