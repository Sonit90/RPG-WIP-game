# Phase 5 — Character & Training Dummy Visual Foundation

Replacing the placeholder cubes with a character/model foundation that scales to many
playable characters, without the gameplay simulation learning what a mesh is.

Scope: models, skeletons, animation, materials/textures, LOD architecture, a character test
scene. Explicitly **not** in this phase: VFX, audio, environment art, a new UI system.

---

## Milestone status

| # | Milestone | Status |
|---|-----------|--------|
| 1 | Audit existing architecture | ✅ complete |
| 2 | Define asset structure | ✅ complete |
| 3 | Model format and import pipeline | ✅ complete |
| 4 | Character visual abstraction | ✅ complete |
| 5 | FireMage model | ✅ complete |
| 6 | Training dummy models | ✅ complete |
| 7 | Skeleton foundation | ✅ complete |
| 8 | Skeletal animation | ✅ complete |
| 9 | Basic FireMage animations | ✅ complete |
| 10 | Animation state integration | ✅ complete |
| 11 | Casting animation foundation | ✅ complete |
| 12 | Character orientation | ✅ complete |
| 13 | Materials and textures | ✅ complete |
| 14 | Performance-conscious model rendering | ✅ complete |
| 15 | LOD foundation | ✅ complete |
| 16 | Character visual test scene | ✅ complete |
| 17 | Asset validation | ✅ complete |
| 18 | Automated tests | ✅ complete |
| 19 | Manual visual verification | ✅ complete |
| 20 | Performance audit | ✅ complete |
| 21 | Full regression | ✅ complete |

---

## Milestone 1 — Architecture audit

### 1.1 The crate graph as it stands

```text
engine-core        math, transform, jobs, time, metrics, config      (no windows, no D3D12)
engine-collision   capsule controller, collision world               (engine-core only)
engine-game        the simulation: entities, combat, abilities       (engine-core only)
engine-settings    the settings file                                 (no engine deps)
engine-render-data RenderWorld, handles, Camera, Lighting, UiDrawList (engine-core only)
engine-renderer    D3D12: device, meshes, materials, pipelines       (engine-render-data)
engine-presentation extraction, camera, HUD, UI                      (game + render-data)
engine-app         the executable: window, loop, scenes              (everything)
engine-showcase    dev-only renderer test bed                        (renderer)
engine-bench       benchmarks
engine-sim         headless combat simulator
```

The separation this phase must preserve is already the load-bearing rule of the workspace:
**`engine-game` depends on `engine-core` and nothing else.** It has no render-data
dependency, let alone a renderer one. Every visual decision is therefore already outside
gameplay; the job of this phase is to keep it that way while the visual side gets much
larger.

### 1.2 How a gameplay entity becomes a renderable entity

`engine-presentation/src/extract.rs`. `RenderExtractor::extract` walks the simulation's
entity slots once per rendered frame and maintains a parallel `RenderWorld`:

```text
Simulation (engine-game)          RenderExtractor              RenderWorld (engine-render-data)
------------------------          ---------------              --------------------------------
position, facing, height   ---->  Transform             ---->  transforms[slot]
kind, alive                ---->  VisualBindings lookup ---->  meshes[slot], materials[slot]
radius, height             ---->  Sphere                ---->  bounds[slot]
health, statuses, cooldowns   X   never crosses
```

Facts that matter:

* Extraction is a **copy**, not a synchronization. Nothing in `RenderWorld` is read back by
  gameplay.
* Render entities are **not** respawned per frame. A gameplay slot keeps its `RenderEntity`
  for as long as the gameplay entity lives; a generation change despawns and respawns.
* `VisualBindings` is a flat struct of `(mesh, bounds, material)` triples supplied by
  `engine-app` — `player_mesh`, `dummy_mesh`, `ground_mesh`, `marker_mesh`, plus
  `dummy_material` / `dead_material`. **This is the hardcoding site this phase must
  replace**: it is a fixed set of fields, one per known thing, and adding a second playable
  character today means adding two more fields to a struct that presentation, the app and
  every scene all mention.
* Per-entity visual state that is *not* a material — hit flash, selection glow, corpse
  fade — already exists as `InstanceStyle { tint, emissive }`, written per frame with no
  slot churn. Character visuals should reuse it rather than inventing a second channel.

### 1.3 How meshes are currently loaded

There is no loader. `MeshData` (`engine-renderer/src/mesh.rs`) is built in code by
constructors — `cube`, `plane`, `disc`, `ring`, `sector`, `strip`, `uv_sphere` — and
`Renderer::create_mesh` uploads it:

```rust
// engine-app/src/app.rs, upload_visuals()
let unit_cube = MeshData::cube(1.0);
let player_mesh = renderer.create_mesh(&unit_cube)?;
let dummy_mesh  = renderer.create_mesh(&unit_cube)?;
```

**The player and every training dummy are literally the same 1 m cube**, distinguished only
by material colour and by the non-uniform scale extraction applies from the gameplay
capsule's radius and height.

Mesh internals:

* `MeshVertex` is `repr(C)`, 32 bytes: `position: Vec3`, `normal: Vec3`, `uv: Vec2`. There
  are no joint indices, no weights, no tangents, no vertex colour.
* Indices are `u32` throughout.
* `MeshData::new` validates: non-empty, index count a multiple of three, every index inside
  the vertex array. It computes a bounding `Sphere` at construction.
* `GpuMesh::upload` puts vertices and indices on a **default** heap through an upload-heap
  staging buffer and blocks until the copy completes. Fine at scene load, never per frame.
* `Renderer::create_mesh` returns `MeshHandle::new(index, 1)` — a `Vec` push. Meshes are
  never freed. Generation is always 1.

### 1.4 How materials and textures are currently loaded

* `MaterialDesc` is five values: `base_color: Vec4`, `roughness`, `metallic`,
  `emissive: Vec3`. It is validated (finite, both scalars in `[0,1]`) and converted to a
  48-byte `MaterialConstants` padded into a 256-byte device-local constant buffer written
  once at creation.
* A material is bound as a **root CBV**, not a descriptor table. `material.rs` says so
  explicitly, and says what changes when a texture arrives: *"When a material gains a
  texture, this becomes a table into the persistent descriptor region that Milestone 4 built
  for exactly that."* The plan for textures was written into the engine before this phase.
* **There is no texture path for meshes at all.** The only sampled texture in the engine is
  the UI atlas: `Renderer::create_ui_state` builds a `GpuTexture`, queues its texels through
  `StagingUploader::queue_texture`, allocates one persistent descriptor from a
  `GpuDescriptorHeap`, creates an SRV, and the UI pipeline binds it as a root descriptor
  table at `ROOT_UI_ATLAS_TABLE`. Every piece a mesh texture needs already exists and is
  proven; it is only wired to the UI root signature.
* Textures currently in the binary are generated at **build time** by
  `engine-renderer/build.rs`, which rasterizes the ability-icon SVGs into RGBA texel arrays
  at two sizes and emits them as `&'static [u8]`. That is a real, working
  source-asset → processed-asset → embedded-runtime-asset pipeline, and it is the precedent
  for how character textures should be produced.

### 1.5 The rendering path

```text
RenderWorld ──► build_render_list ──► RenderList ──► draw_render_list ──► D3D12
               (frustum cull, either            (group by material+mesh)
                serial or across the
                JobSystem; CPU only,
                touches no D3D12)
```

* Culling is sphere-vs-frustum over the SoA arrays, optionally parallel.
* The list is grouped so entries sharing a mesh *and* a material are contiguous.
* Two draw modes, both live: one `DrawIndexedInstanced(count = 1)` per object with the model
  matrix in a per-object root CBV (`mesh.hlsl`), or one `DrawIndexedInstanced(count = N)` per
  batch with the model matrix, tint and emissive arriving in a per-instance vertex stream
  (`mesh_instanced.hlsl`). Instancing is the default; `tests/instanced_equivalence.rs`
  asserts the two produce the same image.
* Root signature: `b0` frame constants, `b2` material constants, `b1` per-object constants,
  bound in change-frequency order as root parameters `[0, 1, 2] → [b0, b2, b1]`.
* Lighting is one directional light plus ambient, Lambert only. No shadows, no specular
  response beyond the parameters being carried.

**Consequence for skinning:** the instanced batch path cannot carry a skinned character. All
instances in a batch share one vertex stream and one set of constants; a skinned character
needs its own bone palette per instance. Skinned characters therefore need their own pass —
which is correct anyway, since they need their own vertex layout and their own shader.

### 1.6 Shaders

`engine-renderer/build.rs` compiles every HLSL entry point to DXIL with `dxc` at build time
and embeds the blobs as `&'static [u8]`. There is no runtime compiler and no shader cache.
Adding a shader means adding a row to the `SHADERS` table — a file nothing references fails
to build rather than being compiled silently. Shader model 6.0.

Existing entry points: `triangle`, `mesh`, `mesh_instanced`, `ui`, `instanced` (benchmark).

### 1.7 Is the renderer capable of skeletal animation today?

No, and the gaps are enumerable:

| Needed | Present today |
|--------|---------------|
| Joint indices + weights on the vertex | ✗ `MeshVertex` is 32 bytes, pos/normal/uv |
| A skinning vertex shader | ✗ |
| Per-draw bone palette upload | ~ the frame upload ring can do it; nothing writes one |
| A skeleton/pose representation | ✗ nowhere in the workspace |
| Animation clips and sampling | ✗ |
| A model concept above "one mesh" | ✗ `MeshHandle` is one vertex+index buffer |
| Textures on a mesh material | ✗ (present for UI, unwired for meshes) |
| LOD | ✗ |

What is present and reusable: the staging uploader, `GpuTexture`, `GpuDescriptorHeap` with a
persistent region, the frame upload ring for transient per-draw constants, the culler, and a
mature build-time asset-processing step.

### 1.8 Where character/animation state should live

`engine-game` already derives everything the animation layer needs, and derives it from one
place:

* `movement::MovementState` — `Idle`, `Running`, `Jumping`, `Falling`. Its own doc comment
  says it exists *"for animation and for the developer overlay"* and that it is derived from
  velocity and ground contact every step rather than stored, because *"two sources for 'am I
  falling' is how a character ends up playing a fall animation while standing still."*
* `Body` — `position` (the **feet**, not the centre), `velocity`, `grounded`,
  `ground_normal`, `facing_yaw`, `state`, and `horizontal_speed()`.
* `Simulation::casting() -> Option<&ActiveCast>` — what is being cast and how far through.

So gameplay is already the authority and already publishes a clean state signal. The
animation layer must **read** these and never write them. `Land` is the one state gameplay
does not have — it is an edge (airborne → grounded), not a state — so it is derived and held
by the visual layer, which is where a purely cosmetic timer belongs.

### 1.9 Where model assets should live

Nothing in the workspace loads a file from disk at runtime except the settings JSON. Every
asset — shaders, icons — is processed at build time and embedded. Two consequences:

1. The shipped binary needs no files beside it, and startup does no decoding work. That is a
   deliberate, documented property and this phase should not casually discard it.
2. There is no asset directory convention yet to follow, so this phase gets to establish one.

### 1.10 Test architecture

* Unit tests live in `#[cfg(test)] mod tests` beside the code, named as sentences
  (`a_stale_handle_is_detected_rather_than_dereferenced`).
* Integration tests live in `crates/*/tests/`. Renderer integration tests create a real
  device and are skipped when no adapter is present.
* Workspace lints are strict: `missing_docs = warn`, and `unwrap_used` / `expect_used` /
  `todo` / `undocumented_unsafe_blocks` are **deny**. Every new type needs docs; no unwraps.
* `cargo clippy --workspace --all-targets -- -D warnings` is the gate.

### 1.11 Existing debug and benchmark scenes

* `--scene combat` — the training arena: player, dummies, abilities, HUD.
* `--scene collision` — a dedicated collision course, no combat. Keys 1–7 teleport to test
  stations, R resets. This is the precedent for what the character test scene should be.
* `engine-showcase` — a dev-only renderer test bed. Nothing depends on it and the game never
  links it.
* `engine-bench` — frame-time, culling, UI and gameplay-pipeline benchmarks.
* `--frames <count>` exits after N frames, and `--report <seconds>` prints frame-time
  reports, which is how a scene is measured without a human watching it.

### 1.12 Findings that shape the design

1. **Gameplay is already clean.** No milestone requires touching `engine-game`. The coupling
   to fix is `VisualBindings`, which lives in presentation.
2. **`VisualBindings` is the thing to replace** — a fixed field per known object cannot grow
   to 6–10 characters, NPCs and skins.
3. **Skinned characters need a separate pass.** Not a regression: the batched instanced path
   keeps drawing the hundreds of static dummies, and the handful of skinned characters get a
   path that suits them.
4. **The texture path exists and is proven** in the UI pipeline; meshes need it wired to
   their own root signature, not invented.
5. **Build-time asset processing is the house style.** Runtime should receive prepared data.
6. **Dummies must stay cheap.** 100 dummies is a benchmark case; they should remain static
   meshes in the instanced path, with no skeleton and no per-frame animation cost.
7. **Gameplay stays authoritative.** `MovementState` and `casting()` are inputs to the
   animation layer, never outputs of it.

---

## Milestones 2â€“9 â€” `engine-assets`

One new crate delivers eight milestones, because they are eight facets of one thing: a
representation of a model that gameplay cannot see and the renderer does not own.

```text
crates/engine-assets/src/
â”œâ”€â”€ lib.rs           the crate's contract and cost model
â”œâ”€â”€ error.rs         AssetError â€” every variant names the model and the place inside it
â”œâ”€â”€ mesh.rs          MeshVertex (32 B) / SkinVertex (56 B), Geometry, SubMesh
â”œâ”€â”€ texture.rs       TextureAsset, TextureFormat (sRGB vs data)
â”œâ”€â”€ material.rs      MaterialAsset: 5 constants + 3 texture slots
â”œâ”€â”€ skeleton.rs      Bone, Skeleton, Pose, skinning matrices
â”œâ”€â”€ animation.rs     Key, BoneTrack, AnimationClip, AnimationPlayer
â”œâ”€â”€ model.rs         ModelAsset, ModelLod, LOD selection
â”œâ”€â”€ character.rs     AnimationSlot, ClipBindings, CharacterDef
â”œâ”€â”€ library.rs       ModelId, ModelIndex, ModelLibrary
â”œâ”€â”€ validate.rs      ValidationLimits, validate_model, ModelReport
â”œâ”€â”€ gltf_import.rs   glTF 2.0 / GLB -> ModelAsset
â””â”€â”€ procedural/
    â”œâ”€â”€ mod.rs       Prism â€” the one primitive
    â”œâ”€â”€ firemage.rs  the FireMage: 21 bones, 8 clips, 4 materials
    â””â”€â”€ dummy.rs     the training dummies: rigid, 3 LODs, 2 variants
```

Dependencies: `engine-core`, `log`, `gltf` (default features off), `png`. **No `windows`, no
`engine-renderer`, no `engine-game`.** The crate sits between render-data and the renderer in
the graph: it describes what to draw without knowing how anything is drawn.

### Milestone 2 â€” Asset structure

There is no `assets/` directory of binary files, and that is a decision rather than an
omission. The engine's existing rule â€” established by the shader and icon pipelines â€” is that
assets are *processed at build time and embedded*, so the shipped binary needs no files beside
it and startup does no decoding. Character models follow the same rule.

The structure is therefore in code, and it is a **namespace**, not a directory tree:

```text
Source asset          Import step                Runtime asset        GPU
------------          -----------                -------------        ---
procedural::firemage  (executes)          â”€â”
procedural::dummy     (executes)           â”œâ”€â–º  ModelAsset  â”€â”€â”€â”€â”€â–º  GpuModel
artist .glb bytes     gltf_import          â”˜       â–²                (engine-renderer)
                                                   â”‚
                                            validate_model
```

| Concern | Decision |
|---|---|
| Source asset | A Rust function (procedural) **or** a `.glb` an artist exported |
| Processed asset | Not a separate stage. Import *is* the processing, and it runs at load |
| Runtime asset | `ModelAsset` â€” a plain value with no handles, no pointers, no device |
| Model format | glTF 2.0 / GLB for interchange; `ModelAsset` for runtime |
| Texture format | Uncompressed RGBA8, sRGB or linear per slot. PNG is a container only |
| Animation format | glTF sampler channels to a `BoneTrack` per bone, three channels each |
| Material format | glTF metallic-roughness, field for field |
| Naming | `ModelId` â€” an `Arc<str>` a scene registers in a `ModelLibrary` |

Where models *live* is `ModelLibrary`, keyed by `ModelId`. Adding "6â€“10 playable characters,
many NPCs, different dummies, different skins" is `library.insert(id, model)` per entry; skins
are a material override or a second `ModelId` pointing at the same geometry.

**What was deliberately not built:** an asset database, a GUID system, a dependency graph, a
hot-reload watcher, a streaming system. Each is a real feature; none is needed to put a
character on screen, and the phase brief says not to introduce an unnecessarily complicated
asset management system.

### Milestone 3 â€” Model format and import pipeline

**Runtime format:** `ModelAsset` â€” LODs of submeshes, materials, textures, an optional
skeleton, clips. Indices rather than handles, so a model is a self-contained value that can be
built, compared and validated with no device present.

**Interchange format: glTF 2.0 / GLB.** It was chosen rather than invented because its
coordinate system is already this engine's (right-handed, Y up, âˆ’Z forward, counter-clockwise
front faces), its material model is already `MaterialAsset` (metallic-roughness), and every
DCC tool exports it. `gltf_import::import_glb` covers meshes, submeshes, materials, embedded
PNG textures, skins, bones and animations.

Explicitly refused, with a precise error rather than a silent approximation:

| Refused | Why |
|---|---|
| Cubic spline interpolation | Resampling as linear changes timing the animator authored |
| `JOINTS_1` (more than 4 influences) | Truncating unnormalizes weights and deflates the mesh |
| External `.bin` / image files | No filesystem asset resolver exists; the error says "export GLB" |
| Non-PNG images | One decoder, deliberately |
| Multiple skins | A character has one |
| Morph targets, cameras, lights | Ignored, not refused â€” they are not this engine's concern |

The nastiest importer details, each with a test: glTF is column-major and so is `Mat4`, so the
matrix copy must **not** transpose; inverse bind matrices come from the file and are never
derived from the rest pose; a skinned primitive's node transform must be ignored per spec; a
joint's parent is its nearest *joint* ancestor, because exporters leave un-skinned pivot nodes
mid-chain.

**No runtime cost.** Import runs when a scene is assembled. A frame touches `ModelAsset` only
through `AnimationPlayer::update` and `Skeleton::skinning_matrices`, which are linear passes
over flat arrays with no allocation.

### Milestone 4 â€” Character visual abstraction

The `if entity == fire_mage { load fire_mage.glb }` the brief warns against is prevented by
three types, none of which gameplay can see:

```text
CharacterDef {            AnimationSlot            ClipBindings
  name                    Idle Walk Run            slot -> clip index,
  model: ModelId          Jump Fall Land           resolved once at load
  model_height            Cast CastHeavy
  walk_speed/run_speed
  land_time
  clips: [(slot, name)]
}
```

* The visual controller knows *"this character is running"*. It never knows that this
  character's run animation is called `run` â€” `AnimationSlot` is the vocabulary, and the
  mapping to a name is per character.
* `ClipBindings::get_or(slot, fallback)` is one level deep on purpose: a character with no
  walk clip falls back to run rather than sliding in its idle pose, but a missing clip stays
  visible instead of resolving through a chain to something unrelated.
* `AnimationSlot::loops()` is a property of the *role*, not of the clip: every character's
  idle loops and every character's landing does not, so it is one rule rather than a flag each
  character could set wrong.
* `CharacterDef::model_height` plus `scale_for_height` is what lets a model authored at any
  size line up with the capsule gameplay actually collides with. Gameplay stays authoritative
  about how tall the character is.

### Milestone 5 â€” FireMage model

Built by `procedural::firemage::fire_mage()`. Per the direction given for this phase, the
procedural model is a **technical baseline, not the final art direction**: simple,
performance-oriented, and swappable for an artist's `.glb` through the same runtime types.

| Property | Value |
|---|---|
| Height | 1.80 m (asserted against the built geometry) |
| Bind pose | T-pose, facing âˆ’Z, feet at y = 0 |
| Bones | 21 |
| LOD0 | 21 prisms, **252 triangles**, 504 vertices, 4 submeshes |
| LOD1 | 11 prisms, **132 triangles**, 3 submeshes, from 25 m |
| Materials | robe, skin, leather, ember (emissive) |
| Textures | 256Â² robe, 64Â² skin, 128Â² leather â€” 335,872 bytes total |
| Clips | idle, walk, run, jump, fall, land, cast, cast_heavy |

Silhouette: hooded figure, wide robe skirt, staff with an emissive ember, visible boots and
legs below a thigh-length robe so the locomotion cycles read. Original work â€” every vertex
comes from a documented function of numbers in this repository, and nothing was copied.

The one primitive is `Prism`: a four-sided tapered box between two points, 24 vertices, 12
triangles, with per-face normals computed from the quads themselves. Its winding is the part
that needed a test â€” getting it backwards draws the *inside* of the character and lights every
surface from the wrong side, which looks nearly right until it doesn't.

### Milestone 6 â€” Training dummy models

Built by `procedural::dummy::training_dummy(variant)`. **Rigid â€” no skeleton, no clips.** That
is the phase brief's "do not add unnecessary animation complexity" taken seriously: a skeleton
would add a palette upload per instance and take the dummies out of the instanced batch path
that draws all hundred of them in one call.

| Property | Single | Group |
|---|---|---|
| Height | 1.90 m | 1.67 m |
| Wrap colour | crimson | blue |
| LOD0 | 8 prisms, 96 triangles, 3 submeshes | same |
| LOD1 (22 m) | 4 prisms, 48 triangles, 2 submeshes | same |
| LOD2 (55 m) | 2 prisms, 24 triangles, 2 submeshes | same |
| Textures | 128Â² wood, 128Â² straw, shared byte-for-byte between variants | same |

The two variants share one builder and one texture pair; only the height scale and a material
constant differ. Distinct from the player at a glance: a post on a plinth with a straw body
and a crossbar, versus a hooded figure with a staff.

### Milestone 7 â€” Skeleton foundation

```text
global[i] = global[parent] Â· local[i]        one forward pass, no recursion
skin[i]   = global[i] Â· inverse_bind[i]      the palette the vertex shader indexes
```

* `Skeleton::new` **rejects** a bone whose parent index is not strictly less than its own. A
  cycle in a bone hierarchy is an infinite loop in the hottest pass in the animation system,
  so it is made unrepresentable at construction rather than guarded against per frame.
* `inverse_bind` is a stored field, not derived. Procedural models fill it from their own rest
  pose via `inverse_binds_from_rest`; the importer takes the file's, because an exporter is
  entitled to make bind and rest differ.
* **Nothing requires two characters to share a skeleton.** A `Pose` is only meaningful beside
  the `Skeleton` it was sampled for, and every function taking both checks their bone counts
  agree rather than reading out of range. Clips bind by *name* once, at load, so a clip
  authored against one skeleton can be bound to another without either knowing about the other.
* `MAX_BONES = 128` â€” 8 KB of `Mat4` per palette, about four times what a stylized humanoid
  needs. It is a guard against a broken export, not a constraint on the art.
* Constraint worth recording: the bone list must be topologically sorted. The importer reports
  an unsorted skin with an instruction ("re-export with a sorted hierarchy") rather than
  quietly producing a detached character.

### Milestone 8 â€” Skeletal animation

`AnimationClip` (name, duration, looping, per-bone tracks) feeds `AnimationPlayer`, which
produces a `Pose`.

Supported: playback, looping, duration, playback speed, switching clips, and a cross-fade
between the pose the character was in and the pose the new clip puts it in.

Three decisions that carry their own tests:

1. **`play(clip)` on the clip that is already playing does nothing.** That is what makes it
   safe to call unconditionally from a state mapper every frame â€” the caller says what the
   character *should* be doing, not what changed. Without it a walk animation restarts from
   frame zero sixty times a second and the character appears frozen mid-stride.
2. **`sample` writes over a rest-seeded pose.** A clip animating six bones of a thirty-bone
   skeleton says nothing about the other twenty-four, and they must stay where the skeleton
   put them. This is semantics, not an optimization.
3. **Nothing allocates after construction.** The player owns three poses â€” output, fade
   source, blend scratch â€” allocated once from the skeleton. `Pose::copy_from` exists because
   derived `Clone::clone_from` allocates, and poses are copied three times per character per
   frame.

Rotation blending is `Quat::nlerp` (added to `engine-core`): a lerp plus a normalize, which is
commutative and torque-minimal like slerp and differs only in how evenly it distributes angle
across `t`. It costs a reciprocal square root instead of two trigonometric calls per bone per
frame, and it flips the sign when the pair is more than a quarter turn apart in four
dimensions â€” without which half of all bone pairs take the long way round, which on a
character looks like a limb snapping through the body.

Deliberately absent, all of which are *consumers* of `AnimationPlayer::pose` rather than
changes to it: blend trees, layers, masks, additive poses, IK, root motion, events.

### Milestone 9 â€” Basic FireMage animations

| Clip | Length | Loops | What it does |
|---|---|---|---|
| `idle` | 3.2 s | yes | Slow breath on the hips, small chest and head sway, staff arm held across the body |
| `walk` | 1.0 s | yes | Two steps, 22Â° stride, knee bend, counter-swinging arms, two hip bobs |
| `run` | 0.62 s | yes | 44Â° stride, 12Â° forward lean, bigger bob and arm swing |
| `jump` | 0.40 s | no | Crouch, push, legs extend, arms rise |
| `fall` | 0.80 s | yes | Legs trailing, arms out for balance |
| `land` | 0.28 s | no | Knee compression that recovers |
| `cast` | 0.70 s | no | Staff arm sweeps up and forward, free hand leads, chest turns |
| `cast_heavy` | 1.10 s | no | Both arms overhead, then driven forward |

`walk` and `run` come from one `locomotion()` function parameterized by period, stride, knee
bend, arm swing, bob and lean â€” because they differ in amplitude and period, not in structure,
and two hand-written copies of a cycle drift apart the first time one is tuned.

Two properties are asserted rather than eyeballed:

* **Looping clips start and end in the same pose.** A cycle whose last key differs from its
  first pops once per period, which reads as a limp rather than as a bug.
* **A knee never bends forwards.** The shin's pitch is authored negative throughout, and the
  test checks the quaternion's X component. It is the single most obvious way for a walk cycle
  to look wrong.

Ability-specific animations for all ten spells are deliberately *not* built â€” `cast` and
`cast_heavy` are the two the brief asks to prepare.

### Tests at this checkpoint

```text
cargo test -p engine-assets                                 103 passed, 0 failed
cargo clippy -p engine-assets --all-targets -- -D warnings   clean
```

No GPU required for any of them: every model, skeleton, clip and validation rule is exercised
with no adapter present, which is the point of the crate having no device.


---

## Milestones 10â€“12 â€” Gameplay state to animation, casting, orientation

All three live in `engine-presentation/src/character.rs` and the extractor that drives it.

```text
Body { position, facing_yaw, velocity, grounded, state }      Simulation::casting()
                          â”‚                                            â”‚
                          â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¬â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
                                             â–¼
                                     CharacterState          (presentation, per frame)
                                             â–¼
                                     CharacterVisual
                                       AnimationSlot â†’ AnimationPlayer â†’ Pose â†’ palette
                                             â–¼
                                  RenderWorld::skinned_mut()
```

### The state machine, in priority order

```text
forced (test scene only)  >  casting  >  airborne  >  landing  >  locomotion
```

| Gameplay | Slot |
|---|---|
| `Simulation::casting()` is `Some`, duration < 1.4 s | `Cast` |
| `Simulation::casting()` is `Some`, duration â‰¥ 1.4 s | `CastHeavy` |
| `!grounded && state == Jumping` | `Jump` |
| `!grounded` otherwise | `Fall` |
| grounded, within `land_time` of touching down | `Land` |
| grounded, speed â‰¥ `run_speed` | `Run` |
| grounded, speed > 0.15 m/s | `Walk` |
| grounded, otherwise | `Idle` |

Details that are decisions rather than defaults:

* **`Land` is owned by the visual layer.** Gameplay has no "just landed" state, because
  landing is an edge and not a state. The controller detects the airborneâ†’grounded
  transition and runs its own timer. It is the only piece of state here that gameplay does
  not have, and it is purely cosmetic.
* **Heavy casts are chosen by *duration*, not by ability identity.** A new spell gets a
  sensible animation without this file learning its name.
* **The idle threshold is 0.15 m/s, not zero.** Deceleration leaves a body drifting for a
  frame or two, and a character that flickers between idle and walk at every stop looks
  broken.
* **Locomotion playback rate scales with actual speed**, clamped to 0.55â€“1.75Ã— the authored
  rate. Feet sliding â€” a walk cycle played at a fixed rate while the character moves at a
  different speed â€” is the most obvious animation error there is.
* **Gameplay never waits.** `gameplay_is_not_waited_on_by_the_animation` asserts it: a
  1.1 s `cast_heavy` that gameplay cancels after one frame leaves the character in idle on
  the next, mid-clip.

### Milestone 11 â€” Casting

```text
Ability system â†’ Simulation::casting() â†’ CharacterState.cast â†’ CastVisual â†’ AnimationSlot
```

The animation layer contains no ability logic and the ability system contains no animation
logic. A second cast of the same spell calls `AnimationPlayer::restart`, because `play` would
correctly decide nothing changed and the second cast really does want the clip from the top.

### Milestone 12 â€” Orientation

`CharacterVisual::transform` is `translation(position) Â· yaw(facing_yaw) Â· uniform_scale`,
where every input is gameplay's:

* Position is the **feet**, interpolated between the last two ticks with the same
  `lerp_position` every other drawable uses, so the character and the camera that follows it
  agree about where it is. Every model in this project is authored standing on its own
  origin, so there is no half-height lift â€” that existed only because a placeholder was a
  cube centred on itself.
* Yaw is `Body::facing_yaw`, interpolated with `lerp_angle` so a turn past the wrap point
  takes the short way round.
* Scale is uniform, `gameplay_height / model_height`. **Not** stretched to the capsule's
  radius, which is what the cube did: a collision volume is not a description of a shape.

No new facing rules were introduced. `the_character_faces_where_gameplay_says_it_faces` and
`a_character_animates_and_lands_in_the_skinned_list` both assert the drawn orientation
against the simulation's.

---

## Milestone 13 â€” Materials and textures

The renderer gained a texture path for meshes. It did not gain a material system.

### The root signature, after

```text
slot 0  CBV b0    frame constants      once per frame
slot 1  CBV b2    material constants   per batch
slot 2  CBV b1    object constants     per draw (unused when instancing)
slot 3  table t0  base colour map      per batch
slot 4  CBV b3    bone palette         per skinned character
static sampler s0 (linear, wrap)
```

Decisions:

* **One root signature for all three mesh pipelines** â€” rigid, instanced, skinned. An unused
  root parameter costs two of the 64 available DWORDs and buys one signature, one material
  bind path and one set of `ROOT_*` constants to keep correct.
* **The texture is a descriptor table, not a root descriptor**, because a root descriptor
  addresses a buffer and not a texture. This is the change `material.rs` predicted before
  there was anything to put in it.
* **The sampler is static**, declared in the signature. It never changes, so there is nothing
  to bind per material and no sampler heap at all.
* **Every material has a map.** An untextured one binds a shared 1Ã—1 white texel. One texture
  fetch that returns 1 is cheaper than a second pipeline state, a second pair of shaders and
  the branch that chooses between them.
* **The map multiplies the material's base colour** rather than replacing it, which is what
  lets one texture serve several tinted materials â€” the two dummy variants share their
  textures byte for byte and differ by a constant.
* **One shared descriptor heap.** The UI atlas used to have a heap of its own, on the
  grounds that it was the only shader-visible descriptor in the renderer. It is not any
  more, and only one CBV/SRV/UAV heap can be bound at a time â€” so both live in one heap,
  bound once in `begin_recording` and never switched.
* **Colour space is on the asset, not assumed at upload.** Base colour is
  `R8G8B8A8_UNORM_SRGB`; a normal or packed map is `R8G8B8A8_UNORM`. Getting this backwards
  is the classic washed-out import and has no other symptom.

### Material and texture budget

| Model | Materials | Textures | Texture bytes |
|---|---|---|---|
| FireMage | 4 (robe, skin, leather, ember) | 256Â², 64Â², 128Â² | 335,872 |
| Training dummy (single) | 3 (wood, straw, wrap) | 128Â², 128Â² | 131,072 |
| Training dummy (group) | 3 | 128Â², 128Â² | 131,072 |
| Shared white | â€” | 1Â² | 4 |
| Arena floor, marker, decals | 6 | â€” | 0 |
| **Total resident** | **16** | **8** | **598,020 B â‰ˆ 584 KiB** |

Normal and packed occlusion/roughness/metallic slots are **carried and validated but not
sampled**. A normal map without per-vertex tangents is a wrong answer rather than a cheap
one, and tangents are a vertex-format change this phase does not need.

---

## Milestone 14 & 20 â€” Performance

Measured on the development machine: AMD Radeon RX 6700 XT, 1920Ã—1080 windowed, vsync off,
release build, game UI on, developer overlay off. Each figure is the **median of three runs**
of 4,000 frames; run-to-run spread on a windowed desktop is about Â±5%, which is larger than
every difference below.

Baseline is commit `0e0d506` â€” the tree as it was before this phase â€” built and measured the
same way from a `git worktree`.

| Scene | Baseline FPS | Phase 5 FPS | Î” |
|---|---|---|---|
| A â€” 1 FireMage, 2 dummies | 3,263 | 3,134 | âˆ’4.0% |
| B â€” 1 FireMage, 11 dummies | 3,018 | 2,978 | âˆ’1.3% |
| C â€” 1 FireMage, 51 dummies | 2,772 | 2,756 | âˆ’0.6% |
| D â€” 1 FireMage, 101 dummies | 2,739 | 2,617 | âˆ’4.5% |

Phase 5 detail, from the frame report:

| Scene | CPU mean | GPU mean | Draws | Skinned draws | Triangles | Visible entities |
|---|---|---|---|---|---|---|
| A | 0.320 ms | 0.044 ms | 12 | 4 | 800 | 7 |
| B | 0.312 ms | 0.048 ms | 12 | 4 | 1,448 | 25 |
| C | 0.366 ms | 0.050 ms | 12 | 4 | 3,680 | 105 |
| D | 0.414 ms | 0.057 ms | 12 | 4 | 6,080 | 192 |

Allocations: **zero per tick**, over 3,000 measured ticks, at every scenario the gameplay
benchmark runs â€” including 1,000 dummies. The animation player allocates its three poses once
from the skeleton and never again; the palette, the skinned draw list and the render list all
reach a high-water mark and stop.

Gameplay benchmark, 1,000 dummies (2,175 render entities after the model change):

```text
simulation tick    mean   23.620 us
render extraction  mean   58.554 us
frustum culling    mean   15.390 us
visible 784 of 2175, 782 draw batches
allocations over 3000 measured ticks: 0
```

### What the 4% bought

* The player went from one 12-triangle cube to a 252-triangle skinned character with a
  21-bone palette uploaded and 4 draws recorded every frame.
* Each dummy went from one 12-triangle cube in a shared batch to a 2â€“3 submesh model of
  24â€“96 triangles.
* Every pixel in the world pass gained a texture fetch.
* Total scene triangles at 100 dummies went from roughly 1,300 to 6,080 â€” **4.7Ã—** â€” for a
  4.5% frame-rate cost.

**Both targets are met with a wide margin**: the 240 FPS minimum by more than 10Ã—, the
480 FPS aspiration by more than 5Ã—. Nothing here is close to being the constraint, and no
optimization was performed on the strength of these numbers, because none is indicated.

### Draw calls did not increase

The count stays at 12 across every scene, which deserves an explanation: the dummies'
submeshes are grouped by material and mesh exactly as the cubes were, so 100 dummies of 3
submeshes each is still 3 instanced draws, not 300. Only the character adds draws, and it
adds a fixed four.

---

## Milestone 15 â€” LOD

Levels of detail are **selected in presentation and resolved by mesh handle**, so the
renderer neither knows nor needs to know that LODs exist.

```text
camera position â”€â”
entity centre â”€â”€â”€â”¼â”€â–º distance â”€â–º ModelBinding::select_lod(distance, lod_bias) â”€â–º level index
lod_bias â”€â”€â”€â”€â”€â”€â”€â”€â”˜                                                                    â”‚
                                                                                      â–¼
                                                             the parts spawned for that level
```

| Question | Answer |
|---|---|
| How is a level selected? | The last level whose `min_distance` the biased distance has passed |
| What determines distance? | Camera position to the entity's **middle**, not its feet â€” a tall model whose head is still in frame must not swap |
| How does the renderer switch meshes? | It does not. Extraction despawns the entity's parts and respawns them at the new level |
| What controls it? | `ViewParams::lod_bias`. 1.0 is the authored thresholds; 0 pins LOD0, which is what a screenshot wants |
| How does it interact with culling? | Not at all, by design. Culling reads the bounds of whatever level was selected, and a level change is a respawn that happens on a threshold crossing rather than per frame |
| What about characters? | The same rule, applied in `CharacterVisual::submit`, choosing which submeshes to push into the skinned list |

Authored levels:

| Model | LOD0 | LOD1 | LOD2 |
|---|---|---|---|
| FireMage | 252 tris, 4 draws | 132 tris, 3 draws, from 25 m | â€” |
| Training dummy | 96 tris, 3 draws | 48 tris, 2 draws, from 22 m | 24 tris, 2 draws, from 55 m |

Validation enforces that LOD0 starts at distance 0 and that thresholds are **strictly
increasing** â€” two levels at the same distance is a level that can never be selected, which
is dead content nobody notices.

Deliberately not built: screen-coverage-based selection (needs the projection, and distance
is a good enough proxy at one field of view), hysteresis on the threshold (a character
oscillating across a boundary would respawn its parts each frame â€” worth adding when
something does it), and cross-fading between levels.

---

## Milestone 16 â€” Character test scene

`mygame --scene character`. A third independently launchable scene, beside `combat` and
`collision`, for the reason the other two are separate: judging a walk cycle should not
require winning a fight to get a clear view of the character.

Contents: the FireMage, one training dummy to judge scale against, the arena floor, the
existing lighting and the existing third-person camera. It is the *same* simulation the game
runs â€” `training_arena` with a group of zero â€” so the character being inspected is the
character the game draws, not a copy of it.

| Key | Effect |
|---|---|
| `1` | Hold Idle |
| `2` | Hold Walk |
| `3` | Hold Run |
| `4` | Hold Jump |
| `5` | Hold Cast |
| `6` | Hold CastHeavy |
| `7` | Hold Fall |
| `0` | Release â€” the character goes back to reflecting gameplay |

The override is `CharacterVisual::force_slot`, checked *before* everything else in
`choose_slot`, which is what makes it an override rather than a hint. Nothing in the game
ever sets it. `0` releasing it is what proves it is an override rather than a mode.

Movement, the camera, jumping and collision all still work in this scene, so the held
animation can be inspected from any angle while the character stands still.

---

## Milestone 17 â€” Asset validation

`engine_assets::validate_model` runs on **every** model before it is uploaded â€”
`Renderer::create_model` calls it first and refuses to upload a model that fails.

| Detected | Why it must be caught here |
|---|---|
| Index past the end of the vertices | D3D12 does not bounds-check an index buffer; the symptom is a triangle stretched to the horizon, or a hang |
| Index count not a multiple of three | A triangle list with a partial triangle |
| Submesh naming a material that does not exist | Would bind whatever is in that slot |
| Material naming a texture that does not exist | Would sample an unrelated image |
| Skinned geometry with no skeleton | Nothing to build a palette from |
| Joint index past the end of the skeleton | Checked **even at zero weight** â€” the shader multiplies before it adds, so it reads the palette entry either way |
| Weights not summing to 1 | Deflates the model towards the origin; reads as "slightly wrong" rather than as a bug |
| Clip animating a bone the skeleton lacks | The clip is bound to the wrong skeleton |
| Clip on a model with no skeleton | As above |
| Key at a negative or non-finite time | Breaks the binary search |
| LOD0 not starting at 0, or thresholds not increasing | Nothing drawn up close, or a level that can never be selected |
| Texture texels not matching its dimensions | The upload reads past the end of a `Vec` |
| Texture larger than 2048Â² | Budget |
| More than 100,000 triangles in a level | Catches a raw sculpt or a subdivision cage |
| More than 16 materials, or 32 submeshes in a level | Budget |
| Non-finite vertex positions, roughness or metalness outside `[0,1]` | `NaN` propagates through the whole lighting expression |

Every error names **the model, the place inside it, and the numbers** â€”
`"model 'fire_mage' at lods[0] submesh 2 'robe': index 991 is past the end of 640 vertices"`.
There is no silent fallback anywhere: a model that fails validation is not uploaded, and
`create_model` returns the failure with its text intact.

Budgets are in `ValidationLimits` and are deliberately about ten times what the phase's own
assets use, because a budget that trips on ordinary content teaches people to raise it.

---

## Milestone 18 â€” Automated tests

| Area | Where | What |
|---|---|---|
| Asset layer | `engine-assets` (103 tests) | Model registration, lookup by id, missing id reported by name, replacement on re-registration, every validation rule above, texture dimension checks, base64 and PNG decode, glTF refusal cases |
| Character visual | `engine-presentation::character` (14 tests) | Construction refuses a model with no skeleton or a mistyped clip name; transform follows gameplay's position, yaw and height; scale is derived from the authored height; submission pushes one palette and one instance per submesh; bounds are padded past the bind pose |
| Animation | `engine-assets::animation` (13 tests) | Sampling and interpolation, hold rather than extrapolate, looping wrap vs one-shot clamp, cross-fade, `play` on the current clip is a no-op, `restart`, speed clamping, a clip index the model lacks is an error, a bone the pose lacks is an error |
| Animation state | `character.rs` + `extract.rs` | Idle/Walk/Run thresholds, the idle deadband, Jump vs Fall, the landing edge and its expiry, cast priority, cast-heavy selection, gameplay not waited on, playback rate scaling |
| LOD | `model.rs`, `skinning.rs`, `character.rs`, `extract.rs` | Boundary selection, bias, `NaN` distance, out-of-range level falls back, a distant dummy costs fewer parts, a distant character submits fewer draws, strictly-increasing thresholds are enforced |
| Skinning maths | `engine-assets::skeleton` (10 tests) | Cycle rejected at construction, globals accumulate, **the rest pose skins to identity**, a parent rotation carries its children, a pose from another skeleton is refused |
| Rendering | `engine-renderer/tests/character_render.rs` (6 GPU tests) | The FireMage is drawn and lit; the bind pose is **pixel-identical** to the same geometry through the rigid pipeline; posing moves it; every one of the eight clips changes the picture; an oversized palette is refused; the dummy is drawn and does not look like the character |
| Regression | `engine-renderer/tests/instanced_scene_equivalence.rs` | Still pixel-identical between the per-object and instanced paths, now with a texture fetch in both |

The GPU tests skip themselves when no adapter is present; everything else runs with no device.

**Test-only functionality kept out of production:** `VisualBindings::unbound` and
`CharacterVisual::force_slot` are the two pieces of API that exist for tests and the developer
scene. Both are ordinary public functions rather than `#[cfg(test)]`, because the benchmark
crate and the character scene are separate binaries that need them â€” and both are inert
unless called.

---

## Milestone 19 â€” Visual verification

Run interactively and, more usefully, **asserted in pixels** so the verification survives.
`crates/engine-renderer/tests/character_render.rs` renders on the real GPU and checks:

| Claim | How it is checked | Result |
|---|---|---|
| The FireMage appears | Non-background pixel count at 320Ã—240 | 2,000+ pixels |
| It is lit rather than a silhouette | Spread of brightnesses across its faces | 1,888 bright / 25 dim |
| Its scale and orientation are right | It fills a frame framed for a 1.8 m character at 4.2 m | passes |
| Materials and textures are loaded | The white map path and the textured path both render; the two dummy variants differ by material constant alone | passes |
| **Skinning is correct** | The bind pose is **pixel-identical** to the same vertices through the rigid pipeline â€” 0 pixels differ | passes |
| Posing works | An arm brought down from the T-pose moves 229 pixels | passes |
| Every clip animates | All eight clips sampled at their midpoint differ from the rest pose | passes |
| Dummies appear and are distinct | Drawn, and 2,000+ pixels differ from the character | passes |

Interactive verification, run as `mygame --scene combat` and `mygame --scene character`:

* Player movement, jumping, falling and landing drive the animations; the character faces
  where it moves and where it casts.
* Single-target and group dummies appear at the arena's positions, at the right scale, with
  the featured dummy visually distinct from the group.
* Targeting, the target marker, nameplates, damage numbers, the hit flash, the health tint,
  the corpse tint and falling over all still work â€” they are per-entity styles, and the model
  change moved them from one render entity per dummy to three without changing what they do.
* Casting an ability plays the cast animation; a long cast plays the heavy variant; a
  cancelled cast returns to locomotion immediately.
* The full combat cycle â€” select dummy, cast, damage, dummy reacts, dummy dies and falls
  over â€” is unchanged.

---

## Milestone 21 â€” Full regression

```text
cargo fmt --all -- --check                                  clean
cargo clippy --workspace --all-targets -- -D warnings       clean
cargo test --workspace                                      1,487 tests, 0 failures
cargo build --workspace                                     clean
cargo build --workspace --release                           clean
```

Behaviour re-checked: movement, jumping, collision, the collision demo scene, target
selection, combat, all ten abilities, talents, damage, the damage meter, the pause and
settings UI, the ability bar, the targeting decals, the developer overlay, the wireframe
toggle, and both existing benchmark scenes.

Two behaviours changed on purpose, and both are recorded here rather than left to be
discovered:

1. **A dead dummy is now tinted rather than given a second material.** `InstanceStyle`
   already carried a per-entity multiply; a `dead_material` per model in the game was the
   alternative. It still falls over.
2. **The player is no longer a `RenderWorld` slot.** It is a skinned character in the frame's
   skinned draw list. `RenderExtractor::render_entity(player)` therefore returns `None`, and
   a scene that does not call `set_player_visual` does not draw its player â€” loudly, rather
   than falling back to a cube.

---

## Architecture

```text
                         Gameplay                     engine-game
                            â”‚                         (engine-core only; no assets,
                    Character State                    no render-data, no D3D12)
                            â”‚
                            â–¼
                    Visual Controller                 engine-presentation
                            â”‚                         CharacterVisual + RenderExtractor
             â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”¼â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
             â–¼              â–¼              â–¼
           Model         Skeleton       Material       engine-assets
                            â”‚                          (no device, no windows)
                            â–¼
                       Animation
                            â”‚
                            â–¼
                        Renderer                       engine-renderer
                            â”‚                          create_model / skinned pass
                            â–¼
                            GPU
```

Crate dependencies, all one-directional:

```text
engine-core
   â–²   â–²        â–²
   â”‚   â”‚        â”‚
   â”‚  engine-assets â—„â”€â”€â”€â”€â”€â”€â”€â”€ engine-renderer â”€â”€â–º engine-render-data
   â”‚        â–²                        â–²                    â–²
   â”‚        â””â”€â”€â”€â”€ engine-presentation â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
   â”‚                     â–²
engine-game â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
```

`engine-game` gained no dependency in this phase and lost none. It still depends on
`engine-core` and nothing else, and it cannot name a mesh, a material, a skeleton or a clip.

### How to add a second playable character

Without modifying the gameplay system, the renderer, the extractor or the visual controller:

1. **Provide a model.** Either write a function returning a `ModelAsset` â€” as
   `procedural::firemage` does â€” or hand `gltf_import::import_glb` the bytes of an artist's
   `.glb`. The two produce the same type and are interchangeable.
2. **Write a `CharacterDef`.** Which `ModelId`, how tall the model is authored, its walk and
   run speeds, its landing time, and which clip name fills each `AnimationSlot`.
3. **Upload and bind it:**

   ```rust
   let model = frost_knight()?;                       // or import_glb("frost_knight", bytes)?
   let binding = renderer.create_model(&model)?;      // validates, then uploads
   let visual = CharacterVisual::new(frost_knight_character(), Arc::new(model), binding)?;
   extractor.set_player_visual(visual);
   ```

That is the whole change. Nothing branches on which character it is: `AnimationSlot` is the
vocabulary the controller speaks, the clip names are per-character data, the scale comes from
the definition's authored height, and the skeleton may have entirely different bones â€” clips
bind by name at load and the palette is whatever length the skeleton is.

The same three steps add an NPC or a second dummy kind, with `binding_for` in `extract.rs`
gaining one line to say which entities use it.

---

## Known limitations

Deferred deliberately, with the reason:

| Limitation | Why it is acceptable now |
|---|---|
| The procedural models are stylized prisms, not sculpted art | Stated goal for this phase: a technical baseline, swappable for a `.glb` through the same types |
| Normal and packed ORM maps are carried but not sampled | A normal map without per-vertex tangents is a wrong answer; tangents are a vertex-format change |
| No mipmaps, no BC7 compression | Nothing measured says texture memory or minification is a constraint; 584 KiB total |
| Textures are not deduplicated across models | The two dummy variants upload identical 128Â² pairs twice â€” 128 KiB wasted |
| One skinned character is drawn per draw call, not instanced | Every instance needs its own palette; there is one character and a hundred rigid dummies |
| Animated bounds are a fixed 1.6Ã— padding of the bind pose | A real animated bound means posing every clip at load and taking the union â€” a build-time tool |
| No LOD hysteresis | A character oscillating across a threshold respawns its parts each frame. Nothing does yet |
| Only one skin per glTF file, no cubic splines, no morph targets, no external `.bin` | Each is refused with a precise message rather than approximated |
| No blend trees, layers, masks, additive poses, IK or root motion | All are consumers of `AnimationPlayer::pose`, not changes to it |
| No shadows, no ambient occlusion, no specular response | Out of scope; the lighting model is unchanged from Phase 2 |
| Bone palettes are uploaded as 8 KB regardless of bone count | The shader's array is fixed-size; 8 KB per character per frame is not measurable |
| The skinned pass has no wireframe equivalence test | The wireframe pipeline exists and is built; nothing asserts it pixel-for-pixel |

Explicitly **not started**, per the brief: VFX, audio, environment art, a new UI system.

## Future improvements

In the order they would pay off:

1. **Real art.** The importer is finished and tested; a `.glb` replaces a function call.
2. **Texture deduplication and BC7**, once more than a handful of models exist.
3. **Animated bounds computed at load**, replacing the padding factor.
4. **Instanced skinning** via a structured buffer of palettes, when there are enough
   characters to pay for the indirection.
5. **A blend tree or a layer system**, built on top of `AnimationPlayer` rather than inside it.
6. **Tangents and normal mapping**, which is one vertex-format change and one shader change.
7. **LOD hysteresis and screen-coverage selection**, when something oscillates.

