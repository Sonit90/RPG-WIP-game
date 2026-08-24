# Art Direction

## 1. Vision

The game is a stylized fantasy MMO/RPG with a visual direction inspired by the readability, proportions, color language, and visual clarity of classic MMORPGs, especially the classic World of Warcraft era.

The target is NOT photorealism.

The target is a world that is:

- stylized
- colorful
- readable
- exaggerated
- coherent
- performant
- visually recognizable from gameplay distance

The graphics should look intentionally designed rather than technically realistic.

The visual hierarchy is:

1. Silhouette
2. Large shapes
3. Color
4. Material identity
5. Lighting
6. Small surface detail

Never sacrifice the first three for micro-detail.

---

## 2. Core Visual Principles

### 2.1 Readability First

Every important object must be recognizable at gameplay distance.

Characters, enemies, props, buildings, and spell effects must have:

- clear silhouettes
- exaggerated primary shapes
- limited visual noise
- strong separation from the environment

A model that looks impressive when viewed at 30 cm but becomes unreadable at gameplay distance is a failure.

---

### 2.2 Stylization Over Realism

Do not attempt to reproduce physically realistic materials.

Prefer:

- simplified shapes
- exaggerated proportions
- simplified anatomy
- controlled roughness
- broad color regions
- painted surface variation
- stylized highlights

Avoid:

- photorealistic skin
- photographic textures
- excessive micro-surface detail
- physically accurate grime everywhere
- realistic lens effects
- cinematic realism

---

### 2.3 Strong Silhouettes

Primary shapes must be recognizable without textures.

Examples:

- large shoulder shapes
- readable helmets
- oversized weapons
- distinctive cloaks
- large ears/horns
- exaggerated boots
- broad armor plates

The silhouette should communicate the character's identity.

---

### 2.4 Large Shapes Over Micro Detail

Models should be designed from:

1. Primary forms
2. Secondary forms
3. Tertiary details

Primary and secondary forms should carry most of the visual identity.

Tertiary details should support the design rather than dominate it.

---

## 3. Character Direction

### Proportions

Characters should be stylized rather than anatomically realistic.

Use:

- slightly exaggerated heads
- strong shoulders
- readable hands and feet
- simplified anatomy
- exaggerated equipment
- distinctive class silhouettes

Do not make characters look like realistic humans shrunk into low-poly models.

---

### Geometry

Characters should use low-to-medium polygon density.

Initial targets:

- normal character: approximately 2k–8k triangles
- important creature: approximately 5k–12k triangles
- boss: approximately 10k–20k triangles
- exceptional large boss: up to approximately 30k triangles when justified

These are starting budgets, not absolute technical limits.

Silhouette quality takes priority over blindly hitting the lowest triangle count.

---

## 4. Environment Direction

Environments should be modular and reusable.

The world should be constructed from:

- modular walls
- floors
- corners
- pillars
- arches
- stairs
- rocks
- vegetation
- props
- doors
- furniture
- decorations

Avoid making every room a unique handcrafted asset.

The same asset family should be usable in many locations.

---

## 5. Environment Shape Language

Environment geometry should favor:

- large readable forms
- slightly exaggerated proportions
- chunky architecture
- simplified edges
- clear silhouettes
- intentional asymmetry

Avoid:

- perfectly realistic architecture
- excessive tiny geometry
- noisy surfaces
- unnecessary bevels everywhere
- photogrammetry-like detail

---

## 6. Materials

Materials should communicate identity through broad visual characteristics.

Primary material families:

- stone
- wood
- metal
- leather
- cloth
- skin
- vegetation
- dirt
- magic/emissive

Each material family should have a consistent visual treatment across the entire game.

---

## 7. Texture Direction

Textures should be stylized and readable.

Prefer:

- broad color gradients
- hand-painted variation
- painted shadows
- subtle surface variation
- controlled roughness
- moderate normal detail

Avoid:

- photographic textures
- extremely high-frequency noise
- excessive scratches
- realistic dirt overlays
- random grunge

Texture detail should support the shape of the model.

---

## 8. Color Direction

The game should use a controlled palette.

Characters should generally contain:

- primary color
- secondary color
- accent color

Do not use many unrelated colors on a single asset.

Important gameplay elements should use stronger color contrast.

Examples:

- friendly characters: readable, positive colors
- hostile characters: stronger threatening accents
- fire magic: warm orange/red/yellow
- frost magic: blue/cyan
- nature magic: green
- arcane magic: purple/blue
- holy magic: gold/white

---

## 9. Lighting Direction

The lighting system should eventually support a stylized presentation.

The intended direction is:

- readable directional light
- soft but visible shadows
- strong form separation
- moderate contrast
- ambient contribution
- readable characters

Lighting must reinforce shape rather than attempt cinematic realism.

---

## 10. VFX Direction

Spell effects should be:

- colorful
- exaggerated
- readable
- fast
- visually distinct by ability

VFX should communicate gameplay information.

A spell should be recognizable by:

- color
- silhouette
- motion
- timing
- impact shape

Avoid realistic particle simulation when a stylized visual communicates the effect better.

---

## 11. Camera and Gameplay Distance

Assets are designed primarily for gameplay viewing distance.

Do not optimize the art direction for close-up screenshots.

A character must remain readable when:

- several characters are visible
- enemies overlap
- spell effects are active
- the camera is zoomed out

---

## 12. Asset Consistency

Every new asset must belong to the same visual world.

Before approving an asset ask:

1. Does the silhouette fit?
2. Does the polygon density fit?
3. Does the color language fit?
4. Does the material treatment fit?
5. Does the texture detail fit?
6. Does the asset look correct at gameplay distance?
7. Does it look like it belongs next to existing assets?

If not, reject or revise it.

---

## 13. AI Generation Rules

AI-generated assets are raw production inputs, not final assets.

Every generated asset must pass:

1. visual review
2. technical validation
3. scale normalization
4. orientation normalization
5. material normalization
6. texture validation
7. polygon/LOD validation
8. engine import validation

AI-generated assets must never bypass the project's asset pipeline.

---

## 14. Canonical Asset Pipeline

The intended pipeline is:

Concept
↓
Approved concept
↓
AI 3D generation
↓
Blender cleanup
↓
Texture generation / processing
↓
LOD generation
↓
Technical validation
↓
GLB
↓
Engine import
↓
Visual review
↓
Approved game asset

---

## 15. Style Lock

Once a visual style has been approved, it becomes the reference for all future assets.

The project should maintain a small set of canonical reference assets:

- 1 player character
- 1 enemy
- 1 environment kit
- 5–10 representative props
- representative materials
- representative VFX

New assets should be compared against these references.

---

## 16. Forbidden Direction

Do not introduce:

- photorealistic assets
- hyper-realistic PBR
- anime aesthetics
- Pixar-like aesthetics
- realistic military/game assets
- unrelated asset-store visual styles
- excessive cinematic post-processing
- unnecessary high-poly geometry

The game should look like one deliberately authored fantasy world.