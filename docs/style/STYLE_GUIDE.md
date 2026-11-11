# Style Guide

## 1. Purpose

This document is the operational style specification for creating game assets.

It must be supplied to AI tools, artists, technical artists, and asset-processing agents when generating new content.

The purpose is to prevent visual drift between independently generated assets.

---

# 2. Global Style

Style keywords:

- stylized fantasy MMORPG
- classic MMORPG readability
- low/medium-poly
- chunky forms
- exaggerated silhouettes
- hand-painted appearance
- colorful
- readable
- game-ready
- non-photorealistic

Avoid:

- photorealism
- hyper-detailed PBR
- realistic photography
- cinematic realism
- excessive micro-detail

---

# 3. Geometry Rules

## Primary Forms

Primary forms must be visually obvious.

Use:

- broad armor plates
- large cloth shapes
- chunky weapons
- simplified anatomy
- large architectural masses

## Secondary Forms

Secondary forms provide identity.

Examples:

- belts
- shoulder armor
- straps
- pouches
- decorative stones
- weapon guards

## Tertiary Forms

Use sparingly.

Examples:

- scratches
- tiny rivets
- small seams
- tiny cracks

Tertiary details must never overwhelm the silhouette.

---

# 4. Character Rules

## Polygon Targets

Normal character:

2k–8k triangles

Important creature:

5k–12k triangles

Boss:

10k–20k triangles

Large exceptional boss:

up to ~30k triangles when justified.

## Character Design

Characters should have:

- strong silhouettes
- exaggerated equipment
- readable class identity
- distinctive proportions
- simplified anatomy

Equipment should be visually recognizable.

---

# 5. Prop Rules

Props should generally use:

- 100–3,000 triangles depending on importance
- simple silhouettes
- strong color blocks
- limited materials

Small props should not contain unnecessary geometry.

---

# 6. Environment Rules

Environment assets should be modular.

Preferred dimensions should use a consistent world scale.

Walls, floors, doors, pillars and other modular pieces must align to the project's modular grid.

Avoid tiny differences in scale between assets from the same kit.

---

# 7. Texture Rules

## Texture Types

Supported material texture set:

- Base Color
- Normal
- ORM
- Emissive

ORM convention:

R = Ambient Occlusion
G = Roughness
B = Metallic

---

## Texture Style

Base color should contain:

- broad color variation
- painted gradients
- intentional highlights/shadows
- restrained surface variation

Avoid:

- photographic texture
- excessive noise
- random grunge
- extreme sharpness

---

# 8. Texture Density

Initial project targets:

Characters:
~256 px/m

Environment:
~128 px/m

These are guidelines.

Assets from the same environment kit should have approximately consistent texture density.

---

# 9. Material Presets

Use a limited number of shared material presets.

Initial canonical presets:

1. Stone
2. Wood
3. Metal
4. Leather
5. Cloth
6. Skin
7. Foliage
8. Magic/Emissive

Whenever possible, modify a preset rather than inventing a new material.

---

# 10. Color Rules

Each asset should have:

- primary color
- secondary color
- accent color

Avoid many unrelated colors.

Colors should be slightly saturated and readable.

Gameplay-important colors should have strong contrast against their surroundings.

---

# 11. Character Color Examples

Fire Mage:

Primary:
warm red/orange

Secondary:
dark red/brown

Accent:
bright orange/yellow

Nature Druid:

Primary:
green

Secondary:
brown

Accent:
bright nature green

Warrior:

Primary:
desaturated steel / dark material

Secondary:
leather / cloth

Accent:
strong class-defining color

These are examples, not mandatory exact RGB values.

---

# 12. Environment Color Rules

Environment palettes should contain:

- dominant environment color
- secondary material color
- controlled accent color

A dungeon should not contain unrelated colors simply because AI generated them.

---

# 13. AI Concept Art Rules

Concept art should primarily communicate:

- silhouette
- proportions
- materials
- color palette
- equipment
- visual identity

Do not optimize concept art for cinematic composition.

For 3D generation, prefer:

- front view
- side view
- rear view when possible
- neutral pose
- neutral background
- full body
- visible feet
- visible hands
- no cropping

---

# 14. Character Concept Prompt Structure

Use:

[SUBJECT]

[ROLE]

[PROPORTIONS]

[OUTFIT]

[WEAPONS]

[COLOR PALETTE]

[MATERIAL LANGUAGE]

[GEOMETRY STYLE]

[LIGHTING]

[BACKGROUND]

Example:

"Stylized fantasy MMORPG fire mage, readable full-body character design, exaggerated heroic proportions, chunky silhouette, medium-length robe, oversized leather belt, simple metal shoulder pieces, large magical staff, warm red and orange color palette with dark brown secondary colors and bright orange magical accents, hand-painted game texture appearance, low-to-medium polygon game asset design, simplified anatomy, broad readable shapes, restrained surface detail, non-photorealistic, neutral studio lighting, neutral background, full body, front three-quarter view."

---

# 15. 3D Generation Rules

When converting concept art to 3D:

Prefer:

- GLB/glTF
- game-ready topology
- clean UVs
- separate material slots where appropriate
- consistent scale
- upright character orientation
- centered origin

The generated mesh is considered intermediate until validated.

---

# 16. AI 3D Prompt Rules

Always describe:

- stylized fantasy MMORPG
- low/medium-poly
- game-ready
- clean silhouette
- simplified forms
- non-photorealistic
- hand-painted texture style

Avoid requesting:

- ultra detailed
- photorealistic
- cinematic
- 8K realism
- physically accurate materials

---

# 17. Environment Generation Rules

Generate environment assets as modular families.

Example:

STONE DUNGEON KIT

- wall straight
- wall damaged
- wall corner
- inner corner
- outer corner
- floor tile
- floor edge
- arch
- doorway
- pillar
- column
- stairs
- large decorative prop

All assets must share:

- stone type
- color palette
- edge treatment
- texture density
- scale
- material response

---

# 18. Asset Family Rule

Never generate an important asset in isolation.

Generate families.

Bad:

"Create a fantasy stone wall."

Good:

"Create a complete stylized fantasy dungeon stone architecture kit consisting of straight walls, corners, arches, pillars, floor pieces and damaged variants, all using the same stone language, scale, texture density and color palette."

---

# 19. Approval Process

Every AI-generated asset has three states:

RAW
↓
CANDIDATE
↓
APPROVED

RAW:

AI output has not been evaluated.

CANDIDATE:

Technically valid and visually promising.

APPROVED:

Matches the art direction and style guide.

Only APPROVED assets enter the canonical asset library.

---

# 20. Reference Assets

The project must maintain canonical references.

At minimum:

- player character
- enemy
- dungeon wall
- dungeon floor
- rock
- tree
- weapon
- material sample

When generating new assets, compare them against these references.

---

# 21. Consistency Test

Before approving an asset:

1. Put it next to existing assets.
2. View it from gameplay camera distance.
3. Disable textures and inspect silhouette.
4. Inspect base color.
5. Inspect material response.
6. Inspect polygon density.
7. Inspect texture density.
8. Check whether it looks like it belongs to the same game.

---

# 22. Technical Requirements

Every approved asset must:

- use GLB/glTF
- have correct orientation
- have correct scale
- have valid UVs
- have valid normals
- have tangents where required
- use supported material types
- respect triangle budgets
- contain appropriate LODs
- reference valid textures
- pass engine asset validation

---

# 23. What AI Must Never Do

AI must never:

- invent a new visual style without approval
- introduce photorealism
- introduce anime styling
- dramatically change proportions
- introduce unrelated materials
- increase geometry simply to add detail
- add random decorative elements
- use excessive texture noise
- create unnecessary material slots

When uncertain, prefer simplicity and consistency.

---

# 24. Golden Rule

The goal is NOT:

"Make the asset look impressive."

The goal is:

"Make the asset look like it belongs to this game."