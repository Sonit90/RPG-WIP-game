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
