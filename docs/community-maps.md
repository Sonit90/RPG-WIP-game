# Community maps

A community map is **data only**: the editor's scene format plus a title and tags. It can
reference only approved primitives (library assets, enemy archetypes, the dungeon block's fixed
fields). There is no scripting, no custom loot and no custom abilities. Everything is checked by
`engine_realm::mapcheck`, the same code that validates the official dungeons (with larger limits),
so the pipeline is `parse -> validate -> runtime` for all content.

## File format

```json
{
  "format": 1,
  "meta": { "title": "My Crypt", "description": "...", "difficulty": 3, "tags": ["crypt", "short"] },
  "scene": { "...": "a SceneDocument, see docs/level-editing.md" }
}
```

Allowed `scene` keys: `version, name, half_extent, ground_height, spawns, lighting, objects,
collision_shapes, spawn_points, lights, monsters, dungeon, map`. Monsters additionally accept
`patrol: [[x, z], ...]`. Anything else is rejected (`terrain` and `water` are known but not
supported for community maps).

## Validating locally

```text
cargo run -p engine-realm --bin map_check -- my_map.json
cargo run -p engine-realm --bin map_check -- --json my_map.json   # machine-readable report
```

Exit code is non-zero when there are errors. In Rust: `mapcheck::check_str(text, &Rules::default_community())`
(or `CommunityMap::parse` + `validate`). `validate_official(scene, dungeon_def)` runs the relaxed
official rules.

## Rules and codes

Each issue has a stable `code`, a `path` (`scene.monsters[12].monster`) and a message. Errors refuse
the map; warnings do not.

| Code | Meaning |
|---|---|
| `too_large` | input over 8 MiB |
| `malformed_json` | not valid JSON |
| `schema_error` | JSON does not match the format (missing/mistyped field) |
| `unsupported_version` | `format` or `scene.version` is not 1 |
| `unsupported_feature` | a key outside the whitelist (scripts, `water`, `terrain`, ...) |
| `loot_not_allowed` | any `loot`/`drops`/`reward(s)`/`*_table` key |
| `bad_meta` | title 3-48 chars, description <= 1000, difficulty 1-5, <= 5 tags of 1-16 chars, no control characters |
| `bad_string` | pack/monster/scene/dungeon names over 48 chars or with control characters |
| `too_many_objects` / `too_many_monsters` / `too_many_lights` | over 4000 / 200 / 128 |
| `bad_extent` | `half_extent` not in (0, 320] |
| `non_finite` | NaN/inf in a position, yaw or scale |
| `bad_number` | scale outside 0.1-10, `aggro_radius` outside 0-60, `level` outside 1-60, `completion` outside 0-100, bad light/collision numbers, `required_completion` <= 0, `add_count` > 16 |
| `missing_asset` | asset name not in the environment library |
| `bad_collision_shape` | compound collision index does not exist |
| `unknown_monster` | monster id not an enemy archetype |
| `boss_outside_boss_pack` | a boss archetype outside the dungeon's `boss_pack` |
| `too_many_boss_packs` | more than one pack containing a boss |
| `no_spawn` | no `spawn_points` |
| `invalid_spawn` | spawn/monster/add-spawn position non-finite, outside the map, below `ground_height - 1`, or (spawns, add spawns) inside a blocking object |
| `missing_dungeon` | no `dungeon` block |
| `missing_boss` | `boss_pack` unset, or no enabled monster belongs to it |
| `bad_boss_gate` | gate non-finite, zero-sized or centred outside the map |
| `impossible_objective` | sum of enabled non-boss monster completion credit < `required_completion` |
| `bad_add_pool` | `add_pool` empty with `add_count > 0`, unknown id, or a boss |
| `bad_patrol` | patrol not a list of `[x, z]`, over 16 points, or out of bounds |

Warnings: `no_monsters`, `boss_pack_without_boss`.

Official content uses `Rules::official()`: any non-blank asset name, terrain/water/loot allowed, and
larger limits (20000 objects, 1000 monsters, 1000 lights, half extent 2000, scale up to 1000 for
ground planes).

## Authoring in the editor

The level editor (F6 in a development build) builds a community map end to end: place the level, edit
the dungeon block, write the listing, watch the validator, export a file the client's "Upload level
file" accepts. Nothing in the editor re-implements a rule; it writes the map out as the community
file it would be and runs `mapcheck::check_str` on that text, the same call the server makes.

The inspector has four tabs under its header.

| Tab | What it edits |
|---|---|
| **Selection** | the selected thing, as before |
| **Dungeon** | the `dungeon` block: objective, boss pack, gate, start points, boss adds, patrols |
| **Listing** | the `meta` block: title, description, difficulty, tags |
| **Validate** | the validator's issues, the *Validate* and *Export...* buttons |

**Dungeon.** *Placed* is the sum of completion over every enabled, non-boss monster, computed by
`mapcheck::monster_credit` (the validator's own rule), against *Needed*, the dungeon's
`required_completion`; the panel says when it falls short (`impossible_objective`). The boss pack is
picked with the arrows from the packs that exist, or by selecting a monster and *Mark Selected Pack*;
*Move Selected Into Boss Pack* assigns monsters to it. The gate is placed at the middle of the view and
then typed into (centre, size, turn). *Add Spawn At Centre* adds a start point (`P` in the world does
too); select one to move it with the gizmo or its position fields. *Boss Adds* edits `add_count`,
duplicates and the pool (*Add Selected Type* takes the selected monster's type; bosses are refused).
Select a monster to give it a patrol: *Add Waypoint At Centre* appends an `[x, z]`, up to the
validator's limit. Every edit is one undo step, and so is every listing edit.

`timer_seconds` is not part of the community format (the dungeon block's whitelisted keys are in
`mapcheck::DUNGEON_KEYS`), so the editor has no field for it; the server sets the timer.

**Listing.** *Make Community Map* gives the level a listing; until then the level is an ordinary one
and is never validated behind its author's back. Fields hold the validator's limits as constants
exported from `mapcheck` (`TITLE_MAX`, `DESCRIPTION_MAX`, `TAGS_MAX`, `TAG_MAX`, `NAME_MAX`,
`DIFFICULTY_RANGE`). Text is typed with the keyboard: letters (shift for capitals), digits, space,
`-` and `.`. Tags are separated by spaces. The listing is saved beside the level as
`<level>.meta.json` by every save, and read back when the level is opened.

**Validate.** In a community map the validator runs once after the level has stopped changing for
about 0.8 s (and not while a drag or a typed number is in progress), or immediately on *Validate*.
Never per frame. Each issue is a row: red for errors, amber for warnings, `code: message`. Clicking a
row selects and frames what the issue's `path` names (`mapcheck::Issue::target` reads it: objects,
monsters, spawn points, lights); dungeon and gate issues open the Dungeon tab, listing issues the
Listing tab. A row whose target no longer exists says so and selects nothing.

**Approved library only.** Placing an asset or a monster that `Rules::default_community()` does not
allow (`Rules::allows_asset`, `Rules::allowed_monster`, the same functions the validator calls) puts
an "Approved library only" warning in the status line and on the Listing and Validate tabs. It is
shown in community maps only.

**Export.** *Validate > Export...* (or *File > Export Community Map...*) asks for a name and writes
`<level folder>/<name>.community.json`. If the validator reports any error nothing is written, the
Validate tab opens on the issue list, and the status line says so; warnings do not refuse. The file
is exactly `{format, meta, scene}`: what `map_check` and the server accept. Opening a
`*.community.json` from the asset browser (double-click) makes a working copy as an ordinary level
plus its `.meta.json` and opens that, so an exported map loads back unchanged and the export itself
is never overwritten.

**New community map.** *File > New Community Map* (or the Listing tab) writes the template,
`assets/templates/community_blank.json`, as a working copy and opens it: a floor, walls, a spawn, one
pack of skeletons, a boss pack, a gate and an add. It is data; edit the file to change the starting
point. A test keeps it valid.
