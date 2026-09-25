# Dungeon map and minimap

A circular minimap in the top-right corner and a full-screen dungeon map (`M`) whose right
sidebar describes the selected pack or boss from the game's own tables. The map picture is
baked from the level's real geometry through the real renderer; everything drawn over it —
encounter markers, the player, the party — is data.

```text
cargo run --release --features editor --bin mygame -- --scene graveyard
    M                 open / close the map        (settings.cfg: map.toggle_key)
    wheel, drag       zoom about the cursor, pan
    click a marker    select it; click empty ground to clear
    Esc               close
cargo run --release --bin map_bench              the CPU numbers below
cargo test -p engine-presentation --test dungeon_map
```

---

## Architecture

```text
engine-environment  map.rs         MapFrame (world XZ <-> UV, north = -Z = top), MapSettings,
                                   encounters(), frame_for(), walkable_mask(), geometry_hash(),
                                   MapDocument (the .map.cfg sidecar), map_paths()
engine-assets       map_image.rs   style_map() (capture + mask -> parchment map), PNG save/load
engine-renderer     renderer.rs    capture_world() (top-down frame, fenced, never presented),
                                   set_ui_map_texture() (one mipmapped RGBA8 texture),
                                   request_/take_screenshot() (QA)
                    ui_pipeline.rs UiVertex.clip (circle clip + picture select), rect clip on CPU
                    shaders/ui.hlsl t1 = map picture, s2 = linear/mip/border sampler
engine-render-data  ui.rs          UiCommand::{Clip, Disc, MapImage, RotatedIcon}, 10 new icons
engine-game         enemy/mod.rs   EnemyAbilityDef.{description, counter}, EnemyArchetype.loot
                    input_map.rs   Bindings.toggle_map (M), in the collision lists
engine-settings     map.rs         MinimapOrientation {Rotating (default), NorthUp}, toggle key
engine-presentation ui/dungeon_map.rs  model, projections, minimap, full-screen map, sidebar
                    ui/menu.rs     Settings -> MAP screen
engine-editor       ui.rs, panels.rs   map preview (F4), Bake Map, map UV in the inspector
engine-app          map_bake.rs    load-or-bake at level load; app.rs wiring, input routing
```

Crate boundaries hold: `engine-game` still has no renderer or render-data type (its map
contribution is two text columns and a loot table of item ids); `engine-environment` still
depends only on `engine-core` + serde (the mask is arithmetic over the collision boxes the scene
already describes); the renderer names no gameplay concept (it draws "the map picture", not a
dungeon).

### One draw call is kept

The map is drawn inside the existing single UI batch. Each UI vertex grew a 16-byte `clip`
attribute — `[centre x, centre y, radius, picture]`: a circle the pixel shader clips to with a
one-pixel anti-aliased edge (the minimap, and discs), and a flag choosing the atlas or the map
picture (bound as a second descriptor table). Rectangular clips (the canvas, the scrolling
sidebar) are cut on the CPU, texture coordinates included, so text scrolled half out of view is
cut cleanly. The alternative — a second pipeline and a draw per clipped region — would have split
the interface's one draw into many. `tests/ui_budgets.rs` still asserts one batch.

### One mapping

Every world <-> screen conversion goes through `MapFrame` and one of two projections:

* `MinimapProjection` — player-centred, `pixels_per_metre = radius / 48 m`, turned by the camera
  yaw in rotating mode. The picture is a single quad over the circle's bounding square with a
  *rotated affine UV mapping*, so the picture, the markers and the arrow share one transform.
* `CanvasProjection` — north up, panned and zoomed (1x..8x of "fits"), zoom anchored at the
  cursor, centre clamped to the frame.

Tests assert that for any point the UV the picture samples equals `frame.world_to_uv(point)` —
in both minimap modes and at several zoom/pan states — and that a non-square level keeps square
texels (never stretched).

Rotating mode uses the **camera** yaw as "up" (the direction the player looks), and the arrow
shows the **character's** facing relative to it. Using the character's facing for rotation made
the map spin on every strafe, because facing follows velocity when the camera is not steering.

---

## Baking

`engine-app/src/map_bake.rs`, at level load (after the environment, terrain and water are
uploaded):

1. `MapDocument::load(<assets>/generated/maps/<level>.map.cfg)`; if its `geometry_hash`
   matches the level, the PNG is decoded and uploaded — no rendering.
2. Otherwise bake: a private `RenderWorld` holds only the static level (every placement at LOD0
   via a second `EnvironmentPlacer`, plus copies of the terrain and water entities), so nothing
   that moves is ever baked. An orthographic camera looks straight down (`up = -Z`) over the
   vertical range `[floor, ceiling]` (default ground −2 m .. +12 m; roofs and sky are cut by
   the near plane). A bright "surveyor's" sun replaces the level's light for legibility; the
   level's local lights still add their pools. The frame is captured a back-buffer-sized tile at
   a time through `Renderer::capture_world` — the real culling, materials, textures, lights and
   shadows, without particles/overlay/UI, fenced and never presented.
3. `walkable_mask` rasterizes every collision box that crosses knee-to-chest height, grows it by
   a character radius (so hairline gaps between wall segments are not doorways), and flood-fills
   from every spawn, monster and add spawn. Floor / wall / outside per texel.
4. `style_map` turns capture + mask into a tactical map: limestone flagstones (two-metre courses,
   per-stone shade, grout) carrying the render's light and shade, corridors toned apart from rooms
   (by distance to the nearest wall), ambient occlusion and a south-east cast shadow from the
   walls, bevelled dressed-stone walls with an inked face, and a flat blue-black surround that
   matches the interface's canvas so the picture has no edge. Deterministic.
5. PNG + `.map.cfg` (frame, size, vertical range, density, bake time, and a snapshot of the
   encounters with their UVs) are written. The sidecar is JSON with a `.cfg` extension because
   the editor's asset browser reads `.json` under `assets/` as materials.

Encounters are **not** baked: `engine_environment::map::encounters(scene)` derives one marker per
pack (centroid of enabled placements, keyed by the stable pack name) every time a level loads or
the editor changes it. `geometry_hash` excludes monsters, dungeon settings and spawns, so moving a
pack never makes the picture stale; moving a wall does.

`--bake-map` forces a bake; `--no-map` disables the whole feature (the measurement baseline).
`MYGAME_MAP_CAPTURE=1` also writes the unstyled capture beside the map.

Scene file (all optional, defaults omitted on save):

```json
"map": { "floor": -1, "ceiling": 8, "padding": 8, "texels_per_metre": 5,
         "bounds": [-96, -296, 96, 8] }
```

---

## The interface (redesign)

The map was redesigned from its prototype look — bitmap all-caps text, thin gold outlines, yellow
"X" markers, a text-dump sidebar — into a finished game interface. Three engine capabilities made
that possible, and the design sits on top of them.

### Engine capabilities added for it

* **Interface fonts.** `engine-render-data/fonts/` bundles **Inter** (Regular, Medium, SemiBold)
  and **Cinzel** SemiBold, SIL OFL (licences beside them). At build time
  `engine-renderer/build_fonts.rs` rasterizes every glyph 4× supersampled, turns it into a signed
  distance field with an exact Euclidean distance transform, and packs one 1024 × 1024 atlas
  (48 texels/em, ±6 texel range). `engine-render-data/build.rs` reads the same files for advance
  widths, so `UiFont::width/wrap/fit` measure text in `engine-presentation` exactly as the renderer
  lays it out — no renderer dependency. `UiCommand::Label { font, size, align }` draws a run; the
  pixel shader converts distance to coverage with a one-pixel ramp computed from the screen-space
  texel rate, so text is crisp at 11 px and at 4K. Nothing is loaded at runtime; no CDN.
* **Rounded panels.** `UiCommand::Panel` — rounded rectangle with a vertical gradient, a feather
  (soft shadows, glows) or a stroke (outlines, selection rings) — evaluated as a signed distance
  per pixel. Helpers: `panel`, `panel_stroke`, `shadow`.
* **Still one draw call.** Both ride the existing UI batch: the vertex's picture code selects
  atlas / map / font field / panel shape, the font atlas is a fourth root table. `ui_budgets`
  still asserts a single batch.

### Design system (`ui/map_style.rs`)

Centralized tokens used by every map surface: colours (blue-black surfaces 0–3, restrained bronze,
ember / frost / teal / crimson / verdant / amber / arcane accents, three text tones), a type scale
(Title Cinzel 28, Encounter Cinzel 22, Name 17, Heading/Ability 15, Body 14 at 1.5 line height,
UI 13, Meta 12.5, Label 11.5 — nothing smaller), spacing 4/8/12/16/24/32, radii 4/6/10/14, motion
(120 ms hover, 180 ms content, 350 ms tooltip delay). `MapStyle` converts logical sizes at the
player's interface scale once. `title_case` turns the tables' upper-case names into
"Mark of the Host".

### Full-screen map

* **Window** — scrim, rounded 14 px window with a soft 48 px shadow and a bronze hairline.
* **Header** — crypt emblem, "Graveyard Crypt" in Cinzel, a context line ("Dungeon map · 13 packs ·
  37 enemies · 1 boss" or "Inspecting Bone Choir"), **Dungeon completion** bound to the run
  (percent, bar, "Boss sealed until 100%" / "Boss unsealed" / "Dungeon complete"), and a close
  button with an icon, hover state and tooltip.
* **Canvas** — the baked map (now flagstone limestone floors, corridors toned apart from rooms,
  ambient occlusion and cast shadows into the walls, bevelled dressed-stone walls, a flat
  blue-black surround that meets the canvas seamlessly), framed with a margin. Insignias:
  **pack** (iron shield, crossed blades), **elite** (violet crest with a star — packs with an
  elite- or empowered-tier monster), **boss** (spiked crimson medallion with a horned skull and a
  soft crimson glow). Badges: monster count, a green check when **cleared**, a lock while the boss
  is **sealed**; a crimson ring while a pack is **in combat** — all from the simulation's encounter
  states. Hover eases a scale-up and halo; selection draws a bronze ring and halo; hit areas are
  larger than the art. Player arrow with a warm glow, party arrows in green.
* **Controls** — one rounded plate: zoom in, zoom out, fit, centre on the player; hover, disabled
  (zoom out/fit at the fitted view, recentre without a player) and tooltips with their keys.
  A compass, and a hint strip with mouse glyphs and an **Esc** keycap ("Pan · Zoom · Inspect ·
  Close").
* **Keys (map open)** — ↑/↓ or − zoom, ← / → step through encounters, Home fits, `.` centres on the
  player, M / Esc close. Wheel zooms about the cursor and **glides** (eased), holding the world
  point under the cursor.

### The encounter journal

* **Fixed header** — the encounter's insignia, title (Cinzel), type line ("Monster pack · 3
  enemies", "Boss encounter · Hollow Steward"), a status badge (Cleared / In combat / Sealed), and
  roster chips (portrait + "2 × Crypt Ghoul").
* **Scrolling body** (independent scroll, bronze thumb, reveal fade/slide on selection change):
  * Nothing selected: a **progress card** (percent, bar, encounters cleared, seal status), the
    **encounter list** (click to select; cleared ones marked) and a legend.
  * **Filter tabs** (All / Damage / Control / Interrupts, with counts) when the encounter's
    abilities fall into more than one kind.
  * **Monster cards** — portrait in a tinted frame, name, "Ranged · Level 1 · 1,000 health",
    count, chevron. The first card opens by default, the rest fold.
  * **Ability rows** — school icon in an accent frame, name, "1.5 s cast · 22 s cooldown · 35 m
    range" (only fields the row has), a one-line description, and tag glyphs (interruptible, area,
    control). Click to open: full description, an effect list in sentences read off the table
    ("8 growing ticks, 178 arcane damage in all", "Charges its target, slowing them 50% for 1 s"),
    and a **Counterplay** callout (teal accent bar) from the table's `counter` column.
  * **Boss** — lore, "16,000 health · 2 phases · Level 1", abilities grouped under phase headings
    ("Unmasked · below 30% health").
  * **Encounter mechanic: Summoned guests** — the add count and pool from the level's dungeon
    settings, as empowered-monster chips.
  * **Loot** — rarity-framed item rows (name in quality colour, "Uncommon ring · Item level 10",
    stats, drop chance) grouped Armor / Jewelry / Trinkets; with no configured loot, a deliberate
    empty state: chest glyph, "No loot has been configured for this encounter."

### Minimap and HUD

The minimap uses the same system: a pill name plate in Inter, bronze gradient ring with a shadow
and inner rim shade, new insignias (cleared packs greyed), a warm player glow, an **N** badge on
the rim, delayed tooltips. The pinned out-of-range boss sits clear of the badge. The HUD's
dungeon-progress panel beneath it was restyled in the same language (Cinzel name, rounded panel,
bronze bar turning green when the seal breaks).

### Settings

Settings → MAP: minimap orientation (rotating / north up), **Reduce map motion** (every
transition snaps; persisted as `map.reduced_motion`), and the map key (`map.toggle_key`).

## Measurements

Release build, Graveyard Crypt, hidden 1920×1080, 12 000 frames after 500 warm-up, the engine's
own report. After the redesign:

| | CPU mean | CPU p99 | GPU mean | UI build | UI quads |
|---|---|---|---|---|---|
| `--no-map` | 0.285 ms | 0.380 | 0.239 ms | 0.012 ms | 328 |
| minimap | 0.293 ms | 0.508 | 0.243 ms | 0.014 ms | 358 |
| full-screen map, overview | 0.458 ms | 0.690 | 0.407 ms | 0.030 ms | 953 |
| full-screen map, boss expanded | 0.458 ms | 0.596 | 0.408 ms | 0.036 ms | 1300 |

GPU time of the UI pass alone (GPU timestamps, `--benchmark-at 1920x1080,3840x2160`), which is
where the map's cost lives:

| | 1080p | 4K (interface size 1.0) |
|---|---|---|
| no map | 0.004 ms | 0.004 ms |
| minimap | 0.007 ms | 0.008 ms |
| full-screen map, first redesign | 0.166 ms | 0.260 ms |
| full-screen map, after the overdraw fix | **0.062 ms** | **0.145 ms** |

The minimap costs ~0.003 ms. The open map's cost was **overdraw**, not layout or CPU (the CPU
side is 0.03 ms; the higher "CPU" frame time while it is open is the CPU waiting on the GPU).
Every rounded outline was one quad covering its whole rectangle, every drop shadow shaded the
middle hidden under its panel, the scrim was blended under the opaque window, and the window's
own surface was shaded under the opaque canvas and journal — about seven window-sized blended
layers a frame. Now outlines and drop shadows (`UiDrawList::shadow`, hollow; `glow` for soft
shapes seen whole) emit only their edge band as four strips, the scrim covers only what is
around the window, and the window's surface is drawn only around and between the canvas and the
journal. The picture looks the same. What remains is the canvas (ground + map picture), the
journal and its cards, and text — roughly two to three layers. The 3D world still renders beneath
the open map, as in the genre's games; skipping it would save more, but freeze the world behind
a translucent scrim.

The journal is laid out only when its content changes; per-frame UI build stays under 0.04 ms. No
per-frame allocation growth, no new render targets, no continuous animation when idle.

Bake: 170–420 ms end to end (release/debug), CPU half 13 ms mask + 155 ms styling (`map_bench`);
a current picture loads in ~10 ms. Picture 7.5 MB (RGBA8 + mips). Font atlas: 4 MB (1024² RGBA,
single upload at start-up); build-time generation adds a few seconds to a clean renderer build.

`map_bench` (CPU per frame, µs):

| markers | minimap mean / p99 | full map mean / p99 | batching the full map |
|---|---|---|---|
| 14 | 0.8 / 0.8 | 6.5 / 7.3 | 17.1 |
| 64 | 1.6 / 1.7 | 8.4 / 9.2 | 18.3 |
| 214 | 4.4 / 4.5 | 12.9 / 18.3 | 21.7 |
| 1014 | 19.8 / 20.7 | 35.1 / 38.1 | 37.9 |

## Visual QA

Screenshots via `--screenshot out.png --hidden --width W --height H [--open-map --map-select N]
[--minimap north_up] [--editor]`, checked at 1920×1080, 2560×1440 and 3840×2160: the minimap
is round at every size, markers sit on the rooms they mark, the sidebar never covers the canvas
and scrolls. `the_minimap_fits_on_screen_at_every_target_resolution_and_scale` asserts the layout
at the three resolutions × UI scales 0.75–2.0. At 4K with interface size 1.0 the map is small
like the rest of the HUD; the interface-size setting (2.0 at 4K) is the intended answer.

## Tests

| Where | What |
|---|---|
| `engine-environment/src/map.rs` (11) | UV round trip, north-up, aspect/texel size, frame derivation, mask floods rooms, doorway vs hairline gap, encounters per pack + boss, hash ignores monsters, sidecar round trip + staleness, paths, scene-file round trip |
| `engine-assets/src/map_image.rs` (5) | floor/wall/outside contrast, render shade kept, determinism/resampling, size check, PNG round trip |
| `engine-renderer/src/ui_pipeline.rs` (5) | rect clip trims UVs, circle clip per vertex, disc, rotated icon, parallel chunk inherits clip |
| `engine-render-data/src/ui.rs` (3) | appended clip ends, clip containment, degenerate commands dropped |
| `engine-settings/src/map.rs` (4) + round trip | defaults, both modes by name, key names, per-field fallback, persisted in `settings.cfg` |
| `engine-presentation/tests/dungeon_map.rs` (24) | both minimap modes, picture/marker agreement, canvas zoom/pan alignment, zoom glides and holds the cursor's world point, reduced motion is instant, no stretching, select/deselect/drag, open/close, every control acts and disables, keys step/zoom/fit, minimap click, layout at 3 resolutions × 4 scales (map stays the focus), no bitmap text on the map, journal lists every monster and ability, folding defaults, filter counts, boss phases/guests/empty loot, run state reaches badges and header, loot rows from the item table, sentence-case numbers from the table, editor edits move markers, the real crypt |
| `engine-render-data/src/font.rs` (4) | charset mapping, proportional metrics scale, wrapping stays in width, fitting ellipsizes |
| `engine-renderer/src/text.rs` (2) | atlas and metrics describe the same characters; glyph quads sane |
| `engine-presentation/src/ui/map_style.rs` (2) | title case, nothing below the label size, sizes scale |
| `engine-app` | `--bake-map/--no-map/--screenshot/...` parsing, map root |

## Known limitations

1. **No loot is configured** for any boss (see above); the panel says so.
2. **No in-game rebinding** for the map key (named bindings have none); it lives in `settings.cfg`.
3. **Party markers** show whatever `Party` holds; the game currently has one controllable hero,
   so in the crypt the only other member is the friendly healing dummy.
4. **The mask treats terrain as walkable** and water as floor; a level whose playable area is
   bounded by steep terrain rather than collision boxes floods past it.
5. **No room names** exist in the level data, so rooms are not labelled on the map; optional
   routes are not flagged in the data either, so they are not distinguished (both are data
   additions the map would pick up).
6. **The rest of the HUD** (bars, frames, nameplates) still uses the 5×7 bitmap font; the
   interface fonts are available to it through `UiCommand::Label`.
7. **The editor's map preview** keeps the editor's own chrome.
8. **Rotated icons under a rectangular clip** are kept or dropped by their centre (a marker at
   the canvas edge may overhang by half its size).
9. **The picture is uncompressed** (7.5 MB for the crypt).
10. **Re-baking waits for the GPU** (it is a load-time/editor action, ~0.2–0.4 s).
