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
| `bad_meta` | title 3-48 chars, description <= 1000, claimed difficulty 1-5, <= 5 tags of 1-16 chars, no control characters |
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

## Publishing and playing

### Lifecycle

```text
 upload ──► draft ──publish──► published ──unpublish──► draft
              │  ▲                 │
         update│  │restore    hide/reject (moderator)
              ▼  │                 ▼
          new version        hidden / rejected
```

* **Create** stores a draft (version 1) with the validator's report. Text that is not a map at all
  (too large, malformed JSON, wrong shape) stores nothing and returns the report.
* **Update** inserts version n + 1 (author only). A published map goes back to `draft`: the older
  version leaves the realm and must be published again, so `cm:<id>:<version>` is always exactly
  what was validated. A rejected map cannot be edited.
* **Publish** (author only) re-runs the validator on the stored text; any error refuses it (422 with
  the report). A hidden or rejected map cannot be published by its author.
* **Unpublish** returns a published map to draft.
* Versions are immutable and kept (up to 200); at most 50 maps per author.

The validator runs **server-side on every create, update and publish**, and again in the realm when
a map is handed to it and when an instance is built. Nothing a client claims about validity is read.

### Editor-exported files

The upload accepts the wrapper format above, or an editor-exported scene (a scene with a `dungeon`
block) that also has a top-level `community` block `{ "title", "description", "difficulty", "tags" }`;
`engine_realm::community::wrap_editor_scene` turns the latter into the former before validation.

### API (`/api/v1`, bearer token)

| Route | What |
|---|---|
| `GET /community/maps?q=&author=&mine=&sort=&difficulty=&status=&page=` | published maps; `mine=true` lists the caller's own (any status). `sort` is `newest`, `plays`, `fastest` (fastest completed run), `difficulty` (easiest) or `hardest`. 12 per page. `status` is honoured for moderators only |
| `POST /community/maps` | create a draft: `{ "request_id", "data": <map file> }` |
| `GET /community/maps/{id}` | detail with the latest report and versions (published, or author, or moderator) |
| `PUT /community/maps/{id}` | save a new version (author only) |
| `GET /community/maps/{id}/versions/{v}` | the stored map file (what clients build the level from) |
| `POST /community/maps/{id}/publish` / `unpublish` | author only |
| `POST /community/validate` | dry run, returns the report, stores nothing |
| `POST /community/maps/{id}/hide` / `reject` / `restore` | moderators |

Write bodies over 2 MiB are refused (413) before they are parsed. Writes are rate limited per account
(10 burst, one per 6 s; validate 10 burst, 2 per s). Every write may carry a `request_id` (UUID):
a retry returns the first answer and changes nothing (`game.community_requests`, the account-scoped
twin of `game.hub_requests`). The report body is `{ "errors": [{code, path, message}], "warnings": [...] }`
with the codes in the table above plus `scene_error` (the level could not be built).

### Moderation

Permission `community.moderate` (migration 0015; held by the `admin` role, or granted to one user
with a `user_permissions` row). `hide` takes a map down (the author can still edit but not publish),
`reject` is final for the author, `restore` returns a hidden or rejected map to draft. Hiding,
rejecting, unpublishing and editing a published map also remove it from the realm.

### Playing

* A queue request for `cm:<id>:<version>` carries `difficulty` 1 to 10 (the community scale; the key
  and modifier rows of `modifiers.json` apply as for official keys, with no random modifiers). The
  API checks the map is published at exactly that version, makes a key for the run (the character's
  real key is never read or changed) and enqueues. The ready check, key choice and start are the
  normal party flow.
* **Realm.** Table `community_map { id: "cm:<id>:<ver>", data, active }`, private, written only by
  the bridge-authorised reducers `svc_community_put` (re-validates, refuses what does not pass),
  `svc_community_remove` and `svc_community_list`. The API pushes on publish, removes on
  unpublish/hide/reject/update, re-puts a map the realm lost when someone queues, and a 30 s
  reconciler (`realm_bridge::sync_community`) repeats both directions (a schema publish wipes the
  realm). `level::dungeon` builds the instance from the row through `community::prepare`
  (mapcheck + the same runtime conversion). A map in use by a party stays loaded until the run ends.
* **Client data: fetched from the API** (`GET .../versions/{v}`), not subscribed. The realm tables
  are private and a subscription would send every map to every player; versions are immutable, so
  the text the client builds from is the text the realm validated. The client builds the level
  from that data only.
* The start barrier is a wall 8 m ahead of the spawn points, square to the axis the first spawn faces.
  There is no timer, so every completion is `timed`.

### Isolation guarantees (no loot, currency or key progress)

Community runs count for leaderboards only. This is enforced in layers, server-side:

1. the fight is armed with `community::no_loot_rules()` (every chance and amount zero);
2. the realm skips the chest roll and the drop and currency tallies (`loot::roll`, `record_drops`,
   `instance::finish`), pays `community::zero_rewards()`, gives no key progress and refuses rerolls;
3. the API's `apply_result` sends a `cm:` result to `apply_community_result`, which writes only the
   zeroed result row, the run row on the board id `cm:<id>` and the plays counter, with no character
   row lock, loadout write, debit, key change, completion count or damage report;
4. `apply_loot` refuses a delivery that names a community result.

Tests: `engine_realm::community` (zero rewards, sanitized results, no-loot rules), the DB tests in
`backend/crates/game/tests/community.rs` (a `cm:` result with experience, resources, loot, currency
and a key bump changes nothing on the character), and the `community` realm-bot scenario.

### Leaderboards

Runs are stored on `dungeon_runs` with `dungeon_id = "cm:<id>"` (all versions share a board).
`GET /leaderboards?dungeon=cm:<id>&board=` supports `fastest` (per difficulty, `key=`), `highest`
(difficulty) and `completions` (one row per character; `LeaderboardPage.completions`). Ordering is
`ScoreKind::KeyThenTime`. `plays` counts runs (not players) once, when the run row is first written.

### Not covered

No scripted content, no per-map loot, no terrain or water, no timers. The editor's metadata and
dungeon-block inspector is a separate task.

### In the game

Hub -> Dungeon Finder -> **Community maps** (`engine-presentation::ui::frontend::community`, state in
`engine-app::online_community`). Browse: title search (Enter), sort (newest, most played, fastest clear,
easiest, hardest), difficulty filter, paging, a detail view (title, author, description, tags, plays,
clears, fastest clear) and **Play** at a chosen level 1 to 10, which continues to role selection and
queues for `cm:<id>:<version>`. My maps: every status with the validator's findings listed as
`[code] Title: path - message`, Publish / Unpublish, **Upload** (type the path of a level `.json`;
an editor-exported scene with `dungeon` and `community` blocks, or a community-format file) and
**Save as new version** for the selected map. Inside an instance the client fetches the version's data,
validates and builds the level with `community::prepare`, and uses the same dungeon row for the start
barrier as the realm.

### Live check

`realm-bot scenario community` (with the local stack up) uploads the fixture, publishes it, refuses an
invalid map and a stranger, plays it as a party at difficulty 3 to completion, and asserts no
experience, loot, currency, key progress or items, one play, one board row, three completions rows and
a history entry. `realm-bot scenario all` runs it with the rest.
