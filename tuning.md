# Tuning: where the numbers are

One file holds every default the game is built from:

```text
crates/engine-core/src/tuning.rs
```

Change a value there and it changes everywhere it is used, including in the tests that assert
against the defaults. Nothing restates a default anywhere else.

## What is in it

| module | what it tunes |
| --- | --- |
| `tuning::player` | health, body size, out-of-combat health regeneration and its bounds |
| `tuning::movement` | speed, acceleration, air control, gravity, jump, arena extent |
| `tuning::character` | the collision capsule: radius, height, step height, slope, snap |
| `tuning::simulation` | RNG seed, how long a refusal stays on screen |
| `tuning::camera` | mouse sensitivity default and slider bounds |
| `tuning::interface` | interface scale default and slider bounds |
| `tuning::audio` | volume bounds and the four channel defaults |
| `tuning::nameplates` | size, height, opacity, margins, safe zone, and the layout's own limits |

Who reads them:

```text
tuning::movement::*   ──▶ MovementConfig::default()   (engine-game)
tuning::character::*  ──▶ CharacterConfig::default()  (engine-collision)
tuning::player::*     ──▶ SimConfig::default(), EntityDesc::player()
tuning::camera::*     ──▶ GameSettings::default()     (engine-settings) ──▶ settings.cfg
tuning::nameplates::* ──▶ NameplateSettings, NameplateConfig, LayoutConstraints
```

## What is deliberately not in it

- **Content.** The enemy archetype table, the enemy attack table, the FireMage's abilities and
  the talent tree are data describing *specific things*, not defaults. A Brute's health belongs
  beside the Brute; a table with one row per monster does not become more editable by being
  moved into a file with the jump height in it.
- **Layout and presentation metrics.** HUD pixel sizes, panel geometry, colours, renderer
  limits and audio mix curves live beside the code that draws or plays with them.

The dividing line: *would a designer change this to tune the game?* → `tuning.rs`. *Does it
describe one particular monster, spell or widget?* → it stays where it is.

## Compile-time defaults, runtime settings

`tuning.rs` is compile-time constants: no parsing, no runtime cost, nothing that can be missing
or malformed. The subset a **player** may change while the game runs lives in `settings.cfg`
and falls back to these values:

```json
{
  "camera_sensitivity": 1.0,
  "ui_scale": 1.0,
  "master_volume": 0.8,
  "sfx_volume": 1.0,
  "ui_volume": 0.7,
  "ambient_volume": 0.5,
  "gameplay": {
    "health_regen_percent_per_second": 1.0
  },
  "nameplates": {
    "size": 11.0,
    "height_above_head": 4.0,
    "non_target_opacity_in_combat": 40.0,
    "vertical_margin": 0.93,
    "horizontal_margin": 0.63,
    "top_safe_zone": 0.07
  }
}
```

Every section is optional and every field within it is: a missing section is all its defaults,
a value out of range or of the wrong type becomes *that field's* default, and a file written
before a section existed keeps everything else in it. There is one settings file, not several.

## Health regeneration

The player regains **1% of maximum health per second while out of combat**, in one tick a
second rather than a fraction every simulation tick — one heal of a readable size, announced
once, instead of sixty events and sixty floating numbers a second.

Out of combat means **nothing is fighting them**: no enemy is chasing, attacking, casting or
recovering. An enemy walking home has already given up, so the fight ends when the last one
stops chasing, and the clock resets the instant anything engages.

Regeneration is not counted as healing done by the combat meter — a player who took the long
way round between pulls has not out-healed one who fought well.

To change it: `tuning::player::HEALTH_REGEN_PER_SECOND` for the default everyone gets, or
`gameplay.health_regen_percent_per_second` in `settings.cfg` for one installation. `0` turns it
off; the file accepts up to 25% a second.
