# Nameplates: settings and smart stacking

Working document for the nameplate phase: what a nameplate setting is, where it is stored,
and how plates are kept from covering one another without being rearranged into a list.

Status: **COMPLETE**.

## The rule the whole thing turns on

> A nameplate belongs to its monster. Keep it as close as possible to the position above that
> monster's head, and only move it when necessary to avoid a collision or to leave the allowed
> screen area.

The layout is therefore **minimal-displacement collision resolution around each plate's own
projected position**, not a vertical list. Sorting plates by screen Y and stacking them
produces a tidy column that has stopped saying where anything is; the player then has to match
names to bodies by hand, which is the problem nameplates exist to solve.

## Pipeline

```text
NameplateSettings (engine-settings)      the six numbers, in the units the screen prints
        |
        v
NameplateConfig::with_settings           metres, alpha, glyph scale, margins
        |
        v
NameplateSet::build                      cull, project, measure  ->  preferred positions
        |
        v
NameplateLayout::resolve                 separate, cap, clamp     ->  final positions
        |
        v
NameplateLayout::smooth                  slide towards them, frame by frame
        |
        v
GameUi::push_nameplates                  panel, outline, text — at the plate's alpha
```

Each stage is in a different module and none of them knows the next one's business. The
resolver in particular takes rectangles with identities and returns positions: no simulation,
no camera, no projection — which is what makes every rule below a unit test over numbers.

## The six settings

| setting | default | range | step | internally |
| --- | ---: | --- | ---: | --- |
| Size | 11 | 6 – 24 | 1 | a glyph-scale multiplier; 11 is exactly 1.0 |
| Height Above Head | 4 | 0 – 20 | 1 | metres: `value * 0.225`; 4 is the 0.90 m the plate already used |
| Opacity for Non Targets (In Combat) | 40 | 0 – 100 | 5 | alpha `value / 100` |
| Vertical Margin | 0.93 | 0 – 3 | 0.01 | multiple of a plate's **height** |
| Horizontal Margin | 0.63 | 0 – 3 | 0.01 | multiple of a plate's **width** |
| Top of Screen Safe Zone | 0.07 | 0 – 0.5 | 0.01 | fraction of the viewport's height |

Displayed units are not internal units, and the conversion lives in one place
(`engine_settings::nameplate`). The defaults are chosen so that a fresh install draws the plate
exactly where and how the game drew it before the settings existed — a default settings file
changes nothing.

Margins are multiples and the safe zone is a fraction, deliberately: a pixel value would mean
one thing at 720p and another at 4K, and the layout would have to be re-tuned per display.

## Storage

`settings.cfg`, in the existing file, as a nested section:

```json
{
  "camera_sensitivity": 1.0,
  "ui_scale": 1.0,
  "master_volume": 0.8,
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

No second file, no second manager. The existing rules apply unchanged: a missing section is
every default, a value out of range or of the wrong type becomes *that field's* default, and a
file written before this section existed loads with everything else in it intact.

One addition to the existing validation: the six nameplate fields deserialize leniently, so
`"size": "big"` costs the size rather than the document. Strict parsing fails the whole file,
and a player with one hand-edited typo would silently lose their camera sensitivity with it.

## The layout algorithm

Per frame, over the plates that are actually going to be drawn:

1. Every plate starts at its preferred position — the projection of `head + height` — clamped
   into the allowed area (the viewport minus the top safe zone).
2. Up to eight separation passes. Each pass sorts by x (a sweep-and-prune broad phase, so the
   inner loop stops as soon as a neighbour is further away than any pair could need) and
   separates every overlapping pair.
3. A pair is pushed apart along **whichever axis costs less**, by exactly the overlap, split
   between the two in inverse proportion to their weights. For a wide, short plate the vertical
   push is almost always the smaller one, so a group standing together stacks rather than
   fanning across the screen — but the axis is worked out, not assumed.
4. After each pass every displacement is capped (ten plate heights) and every plate is clamped
   back into the allowed area.
5. The result is smoothed towards frame by frame, exponentially, with a snap for a new plate or
   a monster that teleported.

The current target is eight times heavier than everything else: the one plate the player is
certainly reading is the one that should not move.

Nothing is stored back. The preferred position is recomputed from the world every frame, so a
plate cannot accumulate its own displacement and drift away from the monster it names.

### Crowds

Fifteen monsters on one point cannot be separated at any margin. The system degrades rather
than failing: plates spread as far as the margins and the displacement cap allow, nothing
leaves the screen, nothing enters the safe zone, and nothing is flung across the screen to
resolve somebody else's collision. Eight plates on one point still separate completely.

## Non-target opacity

While a fight is running — any enemy in a combat AI state — every plate that is not the target
is drawn at the configured opacity, panel, outline and text alike. Out of combat every plate is
drawn in full: the setting exists to make a pull readable, not to dim a room the player is
walking through.

## The settings screen

`SETTINGS -> NAMEPLATES` opens a page of its own with the six sliders and BACK. A section
rather than six more rows on the settings panel: twelve sliders in one column is taller than a
720p window.

Each slider prints its value in the form the reference layout uses — `11`, `4`, `40`, `0.93`,
`0.63`, `0.07` — which is what the `Slider::with_decimals` addition is for. Values apply while
the handle is being dragged and are written to `settings.cfg` on release, exactly as the camera
sensitivity and the volumes are.

## Tests

| area | file | covers |
| --- | --- | --- |
| the resolver | `nameplate_layout.rs` | one plate untouched; two, three and eight separated; unrelated plates not moved; safe zone; screen edges; relative order kept; both margins increasing separation; fifteen-plate crowd; a moving monster; no drift over 600 frames; smoothing without overshoot; new-plate and teleport snaps; target weight; determinism; no allocation; nonsense input |
| the set | `nameplate.rs` | crowded arena produces no overlapping plates and no plate far from its monster; a lone plate is left exactly where its monster is; the safe zone holds in the real projection; height and size settings move the anchor and the box; opacity applies only in combat and only to non-targets; a plate follows a walking monster for 600 frames |
| the settings | `engine-settings` | defaults and their conversions; per-field validation; snapping; round trip; a file with no nameplate section; an invalid field costing only itself; a setting surviving a restart in the same file |
| the screen | `ui/menu.rs`, `ui/paint.rs` | opening and leaving the section; every setting's bounds, step and decimals; dragging reports one setting; the range is never left; loaded settings reach the sliders silently; layout inside the panel at three resolutions; the printed values match the reference form |

## Known limitations

- **The resolver runs every frame, whether or not anything moved.** With a sweep-and-prune
  broad phase and a first pass that exits when nothing overlaps, twenty plates cost a sort and a
  walk; caching a layout would need an invalidation rule covering the camera, every monster's
  position and six settings, which is more state than the pass costs.
- **Displacement is capped, not solved.** A crowd past what the margins can satisfy is left
  overlapping at the cap. The alternative — growing the search until everything fits — is the
  vertical list this design exists to avoid.
- **The plate limit is still twenty.** Beyond that the nearest twenty are drawn, and the layout
  only ever sees those; the setting for it is not exposed on the screen.
