# HUD & Action Bars

Working document for the HUD phase: what is being built, on top of what, and why each seam
is where it is.

Status: **COMPLETE**. Every item in section 13 is done and tested.

> **Since:** two more arrangeable elements — `RAID` (raid frames) and `CAST BAR`, whose width
> and height are set separately with NARROWER / WIDER / SHORTER / TALLER — and the frames are
> drawn in the nameplate style. See [combat_ui.md](combat_ui.md#unit-frames-raid-frames-and-the-cast-bar).
> Then `BUFFS` (the player's buffs, `frames.buffs` in `hud.cfg`), with the player's debuffs
> attached under the player frame and each member's beside the party and raid frames. See
> [auras.md](auras.md).

> **Since (HUD polish pass):** Edit HUD is an inspector card rather than a column of buttons;
> every element shares one generic layout (direction, spacing, width/height); tooltips open
> from a movable dock; the party list is hidden and the raid frames shown by default. See
> [§15](#15-edit-hud-the-inspector-and-the-generic-layout).

## Goals

Eight independent action bars, per-slot spells and keybinds, movable and scalable unit
frames (player, target, target-of-target, party), an Edit HUD mode to arrange them, and a
layout that means the same thing at 1920x1080 and at 2560x1440.

Everything the player arranges survives a restart.

## What already exists, and is therefore not being rebuilt

The inspection pass found a UI stack that already has most of the parts:

| existing | where | reused for |
| --- | --- | --- |
| `UiInput` | `engine-presentation/src/ui/input.rs` | every click and drag in edit mode |
| `Widget` / `UiScreen<T>` | `ui/widget.rs`, `ui/screen.rs` | edit-mode controls, the slot menu, the spell picker |
| `MenuSystem` / `MenuAction` | `ui/menu.rs` | the EDIT HUD button and its action |
| `UiDrawList`, `UiRect`, `UiScale`, `UiIcon` | `engine-render-data/src/ui.rs` | all drawing; one batch, one draw call |
| `GameUi` + `HudMetrics` | `game_ui.rs` | slot painting, bars, the theme |
| `UiSnapshot` | `snapshot.rs` | the only thing the UI reads about gameplay |
| `SettingsManager` | `engine-settings` | the load/validate/save shape the HUD file copies |
| `Bindings` / `KeyCode` | `engine-game/src/input_map.rs`, `engine-core/src/input.rs` | keys, and the collision check a new binding must pass |
| `FireAbility` | `engine-game/src/ability/table.rs` | the spell registry the picker lists |
| `PlayerCommand::UseAbility(slot)` | `engine-game/src/command.rs` | how a pressed slot reaches the simulation |

Two things do not exist and are added, minimally:

- **A party.** `engine-game` has no roster at all. A party frame with nothing behind it
  would be a mock, so a small real `Party` is added to the simulation (§7).
- **A name for a key.** `KeyCode` has no string form, and a keybind has to be written to a
  file and printed in a slot corner (§3).

Target-of-target needs *nothing* new: `EnemyBrain::target()` already answers it.

## 1. Layering

The existing crate rules hold. Nothing below gains a dependency it did not have.

```text
engine-core        KeyCode, KeyCode::name           input vocabulary
engine-game        FireAbility, Party, Bindings     gameplay: what a spell and a party are
engine-settings    HudConfig, HudConfigManager      persistence: plain data, no engine crate
engine-presentation
    ui/hud_layout   HudElement, HudLayout           where a thing is, at any resolution
    ui/action_bar   ActionBarSet                    which spell, which key, which bar
    ui/hud_edit     HudEditor                       moving them
    ui/slot_menu    SlotMenu                        assigning them
    ui/unit_frame   frames                          drawing player/target/tot/party
engine-app          wiring                          the only place that acts
```

`engine-settings` still links no engine crate. That is what keeps every load/validate/save
rule testable with a temporary file, and it is why the config stores a spell as a string and
a key as a string rather than as a `FireAbility` and a `KeyCode`. The conversion between the
two vocabularies lives in `engine-presentation`, which already depends on both sides.

The UI still performs no gameplay. A pressed slot produces a `PlayerCommand`; the
application pushes it. Nothing in `ui/` touches the simulation.

## 2. Resolution-independent positioning

A HUD element is stored as **an anchor plus a normalized offset from that anchor**, never as
a pixel pair.

```text
HudElement
  id        which element
  enabled   drawn and interactive, or neither
  anchor    which corner/edge of the screen the offset is measured from
  offset    (x, y) in [0, 1] of the viewport, from the anchor
  scale     0.5 .. 2.0, multiplied into the element's own size
```

```rust
Anchor = TopLeft | Top | TopRight | Left | Center | Right | BottomLeft | Bottom | BottomRight
```

Resolving to pixels:

```text
anchor_point = (anchor.x * viewport.w, anchor.y * viewport.h)
origin       = anchor_point + (offset.x * viewport.w, offset.y * viewport.h)
rect         = align(origin, size * scale, anchor)
```

An anchor as well as a fraction, because a fraction alone does not survive an aspect-ratio
change the way a player expects: an action bar centred at the bottom should stay centred at
the bottom on an ultrawide, not drift to 21:9 of the way across. The anchor carries the
intent ("bottom centre"), the offset carries the nudge away from it.

`align` also decides which part of the element the origin refers to — a right-anchored
element grows leftwards — so an element parked against the right edge stays against it
instead of hanging off.

**Clamping.** `HudLayout::resolve` clamps the final rect so that at least
`MIN_VISIBLE_FRACTION` of it stays inside the viewport. Dragging cannot push an element off
screen, and neither can a resolution change: the clamp runs on resolve, not only on drag, so
a config written at 3440x1440 and loaded at 1280x720 still produces something reachable.

Sizes stay in the logical pixels the HUD is already authored in, multiplied by the global
`UiScale` and then by the element's own `scale`. That is deliberate: a health bar that grew
with the window would be a different physical size on every display, and the bitmap font
only looks right at integer scales. Position scales with the screen; size scales with the
two scale factors the player controls.

## 3. Keybind model

`engine-core` gains:

```rust
KeyCode::name(self)      -> Option<&'static str>  // "A", "DIGIT1", "F5", "SPACE"
KeyCode::from_name(&str) -> Option<Self>          // exact, uppercase
KeyCode::label(self)     -> &'static str          // a slot corner: "1", "F5", "SPC"
```

`name` is `Option` because `KeyCode::Unknown` deliberately has none: a key the engine cannot
identify is a key nothing can bind, and writing `"UNKNOWN"` to a file would invite reading it
back as a binding.

`name` is the persistence form and is stable across builds — it is not the discriminant,
because reordering the enum must not silently rebind every player's keys. `label` is the
display form and may change freely.

A binding lives on the slot:

```text
ActionBarSlot
    ability: Option<FireAbility>
    key:     Option<KeyCode>
```

Not in a widget, and not in `Bindings`. `Bindings` keeps the movement and camera keys, which
are one fixed set; the action bar's are eighty slots' worth of player data that has to be
saved. They meet in one place, the conflict check (§4).

## 4. Keybind conflicts

One key, one action. `ActionBarSet::conflict(address, key, bindings)` answers who already
owns a key, and it consults both tables:

```rust
enum KeyConflict {
    Slot(SlotAddress),      // another action bar slot
    Reserved(&'static str), // what the game already uses it for, from `Bindings::reserved`
}
```

- **Reserved** keys (W/A/S/D, space, tab, escape, the mouse-look button, the developer keys)
  are refused outright. The UI says so; nothing is rebound. Escape in particular must keep
  working or a player can bind their way out of the menus.
- **Another slot** raises a prompt: `Cancel` leaves everything alone, `Replace` clears the
  old slot's binding and assigns the new one. Never both — `assign_key` unbinds the previous
  owner in the same call, so no intermediate state exists in which two slots claim one key.

The default bar-1 bindings (`1`..`9`, `0`) are exactly what `Bindings::ability_key` produces
today, so a fresh install plays identically to the current build.

## 5. Action bar model

```text
ActionBarSet
    bars: [ActionBar; 8]

ActionBar
    index:       0..8
    enabled:     bool
    orientation: Horizontal | Vertical
    slots:       [ActionBarSlot; 10]
```

Ten slots, because that is `ABILITY_SLOTS` and the size the existing bar, its geometry and
its keys are all built around.

- Orientation changes the geometry function only. Slot *n* is slot *n* in both directions,
  so flipping a bar cannot reorder or lose an assignment — asserted by a test.
- A disabled bar returns no rectangles from `slot_rect`, is skipped by the painter, is
  skipped by hit-testing, and its keys do not fire. "Does not consume interaction space" is
  enforced by there being no rectangle to consume it.
- Bar 1 defaults to the current ten abilities with the current keys. Bars 2–8 default to
  disabled and empty, because nothing in the design asks for them yet.

Casting is unchanged: a slot holds a `FireAbility`, and the application already knows how to
turn one into a command via `cast_or_target`, including the aimed-ability path that opens a
targeting circle instead. A slot whose ability is not on `FireAbility::BAR` cannot be cast,
so the picker offers only castable abilities.

## 6. Persistence

A second file beside `settings.cfg`:

```text
hud.cfg      JSON, in the game directory
```

Separate from `settings.cfg` on purpose. Dragging an element rewrites the layout on every
committed move; camera sensitivity has no business being rewritten with it, and a corrupt
layout must not cost the player their audio settings. The two managers are the same shape,
and `HudConfigManager` copies `SettingsManager` line for line: `load` returns a config and
never a `Result`, an invalid field falls back to *that field's* default, and a malformed
file is replaced by a valid one.

```json
{
  "bars": [
    {
      "placement": {
        "enabled": true,
        "anchor": "bottom",
        "offset_x": 0.0,
        "offset_y": -0.03,
        "scale": 1.0
      },
      "orientation": "horizontal",
      "slots": [
        { "spell": "firebolt", "key": "DIGIT1" },
        { "spell": null, "key": null }
      ]
    }
  ],
  "frames": {
    "player":           { "enabled": true, "anchor": "left", "offset_x": 0.03,  "offset_y": -0.12, "scale": 1.0 },
    "target":           { "enabled": true, "anchor": "top",  "offset_x": -0.09, "offset_y": 0.03,  "scale": 1.0 },
    "target_of_target": { "enabled": true, "anchor": "top",  "offset_x": 0.14,  "offset_y": 0.03,  "scale": 0.8 },
    "party":            { "enabled": true, "anchor": "left", "offset_x": 0.03,  "offset_y": 0.06,  "scale": 1.0 }
  }
}
```

`placement` is nested rather than flattened into the bar. `serde(flatten)` cannot be combined
with the per-field `default` that makes a missing field fall back instead of failing the
parse, and that fallback is the whole point of how this file loads.

Validation rules, matching the existing ones:

| input | result |
| --- | --- |
| file missing | defaults, file created |
| duplicate key across slots | the later slot loses its binding, with a warning |
| offset outside [-1, 1], non-finite | that element's default offset |
| scale outside bounds, non-finite | that element's default scale |
| wrong type, missing field | that field's default |
| fewer/more bars or slots than expected | padded with defaults / extra ignored |
| malformed JSON | all defaults, file repaired |

An unknown *spell* or *key* name is not repaired here: `engine-settings` does not know what a
spell is, so the name is carried through untouched and `ActionBarSet::from_config` resolves
it to an empty half-slot with a warning. Dropping it here would delete a slot that a newer
build could read perfectly well.

Out-of-range is replaced by the default rather than clamped, for the reason the settings
file already gives: a value that means nothing is not a request for the nearest value that
does.

Writes happen on commit, not per frame: a drag mutates memory and marks the config dirty,
and the file is written when the pointer is released, when edit mode is left, and at
shutdown. A test asserts the file is untouched mid-drag.

## 6a. Whose bars they are

Bar one follows the class: a hero swap rewrites its ten slots to the new hero's abilities,
because a bar of spells nobody in the room can cast is not a bar. Everything else — the
other seven bars, every key — is the player's own.

That left one hole, and it was a bug worth writing down. `hud.cfg` is *one* arrangement for
the account, a swap rewrote bar one, and nothing rewrote it on the way back in. A player who
quit as the Druid started again as the FireMage with ten Druid spells on bar one, which the
snapshot draws as empty slots because the FireMage cannot cast them. The bar looked like it
had lost its art, and dragging each spell back was the only cure — which worked, because
that is exactly what a rebind does.

A launch is now told who is playing:

```text
ActionBarSet::for_hero(config, scope, hero)
  account    the saved set, with bar one rebound only if it holds another class's spells
  character  this character's own saved set, or its class bar if it has never been played
```

`rebind_if_foreign` rewrites bar one only when it holds a spell this hero cannot cast, so a
slot the player deliberately cleared stays cleared: emptiness is not somebody else's spell.

### The setting

`settings.cfg` gains `binding_scope`, offered on the settings screen as **KEYBINDS PER
CHARACTER**:

| scope | what a swap does | where the slots live |
| --- | --- | --- |
| `account` (default) | bar one follows the class; the rest are untouched | `bars` in `hud.cfg` |
| `character` | the old character's bars are saved, the new one's loaded | `heroes[]` in `hud.cfg` |

```json
"heroes": [
  { "hero": "druid", "bars": [ { "slots": [ { "spell": "moonfire", "key": "DIGIT1" } ] } ] }
]
```

Only the *slots* are per character. Where a bar sits, how big it is and which way it runs
stay one arrangement for the account, because dragging a bar to a new place is a fact about
the player's screen rather than about a character — and a per-character placement would move
the bar for one hero and leave it behind for the others.

Turning the setting off does not delete anything: every `heroes[]` record stays in the file,
so a player who tries per-character bars and changes their mind finds their characters as
they left them if they turn it back on.

## 7. Party

`engine-game` gains `party.rs`:

```rust
pub const MAX_PARTY: usize = 5;

pub struct Party { members: Vec<EntityId> }   // the local player is always index 0
```

with `join`, `leave`, `contains`, `len`, `iter`, and `Simulation::party()` /
`party_mut()`. The player is enrolled when the player entity is set.

This is as small as it can be while still being real. There are no ally entities in the game
yet, so a fresh session is a party of one — which is what the party frame will show, and it
will grow the moment something calls `join`. The alternative, a UI that fabricates four
teammates, is exactly the mock the brief rules out.

The snapshot gains `party: Vec<PartyMemberView>` (rebuilt in place, capacity kept) and
`target_of_target: Option<TargetView>`, read through `brain(target).target()`.

## 8. Edit mode

```text
Playing --ESC--> Pause --EDIT HUD--> Editing --DONE/ESC--> Playing
```

`MenuAction::EditHudRequested` is emitted by a new pause-menu button; the application closes
the menu and switches `HudEditor` on. The editor is not a `MenuScreen`, because it is not a
panel over the world — it is the HUD itself, made draggable.

```text
HudEditor
    active:    bool
    selected:  Option<HudElementId>
    drag:      Option<{ element, grab_offset }>
    controls:  UiScreen<EditItem>       DONE, and the selected element's toggles
```

Per frame, while active:

1. Every enabled element is outlined and labelled with its name.
2. A press inside an element selects it and starts a drag from the pointer's offset within
   the rect, so the element does not jump to the cursor.
3. Movement writes a new normalized offset through `HudLayout`, which clamps.
4. Release ends the drag and marks the config dirty.
5. The control strip acts on the selected element: `SHOWN` (toggle), `SMALLER`/`BIGGER`
   (scale by one `HUD_SCALE_STEP` of 5 %, clamped), `HORIZONTAL`/`VERTICAL` for a bar, and `DONE`.

**Gameplay cannot be triggered while editing.** `update_firemage_ui` returns early when the
editor is active, so no click casts, no slot opens a menu, and mouse-look is cancelled on
entry. The simulation itself keeps running — the world does not stop for a layout, exactly
as it does not stop for the pause menu.

## 9. Slot interaction outside edit mode

Left click a slot: casts, as today.

Right click a slot: opens `SlotMenu` centred over the slot, rising from it. The panel's
bottom edge sits on the slot's top edge and it grows *upwards*, because a bar is normally at
the bottom of the screen; it is still clamped into the viewport, so a bar dragged to the top
opens downwards instead of off-screen. A click — either button — outside the panel closes
the menu, in every mode, the way a context menu is expected to behave.

```text
+-----------------------+
| FIREBOLT              |
| KEY: CS-1             |
+-----------------------+
| CHANGE SPELL          |
| ASSIGN KEY            |
| CLEAR SLOT            |
+-----------------------+
```

Three modes, one state machine:

- `Menu` — the three buttons above.
- `Picker` — a list of every castable ability, with its icon, in a scrollless grid sized to
  the registry. Choosing one assigns it and closes.
- `Capture` — prints `PRESS A KEY...` over `HOLD CTRL/SHIFT/ALT TOO`. The next key press
  that is not escape and not a modifier is the candidate, together with whichever of control, shift and alt was
  held with it; escape cancels. A *bare* reserved key is refused with a printed reason —
  `SHIFT+W` is not `W`, so a combination over a reserved key is allowed, which is what makes
  eighty slots reachable from a keyboard the game already half uses. A chord held by another
  slot opens `Confirm`.
- `Confirm` — `KEY "1" IS ALREADY ASSIGNED TO BAR 1 SLOT 1. REPLACE?` with Cancel/Replace.

Capture is the one place in the UI that reads an arbitrary key, and it does so through a new
`UiInput::captured_key: Option<KeyChord>` field, filled only while a capture is in progress.
That keeps the "the menu reads the mouse and escape only" rule intact everywhere else: the
field is `None` unless something explicitly asked to capture, and the existing test that
asserts no other key reaches the UI keeps passing.

## 10. Defaults

```text
                        [ Target ]  [ ToT ]

  [ Player ]

  [ Party ]

                       [ Action Bar 1 ]
```

Bar 1 enabled with the current ten abilities and the current keys, bars 2–8 disabled and
empty; all four frames enabled. A fresh install therefore looks like the build that exists
today plus the new frames, and everything else is one visit to Edit HUD away.

## 11. Tests

Every suite is `#[cfg(test)]` beside the code, as the project already does.

| area | covers |
| --- | --- |
| `engine-core/input` | key name round trip, unknown name, every key has a distinct name |
| `hud_layout` | anchor resolution, the same normalized position lands at the same relative place at 1280x720 / 1920x1080 / 2560x1440 / 3440x1440, clamping on drag and on resolve, scale, enable/disable, move each of the five element kinds |
| `action_bar` | eight bars exist, enable/disable, horizontal and vertical geometry, orientation preserves assignments, assign spell, clear slot, assign key, conflict with another slot, conflict with a reserved key, replace unbinds the old owner |
| `engine-settings` | round trip of every field, missing file, malformed file, unknown spell, unknown key, duplicate key, out-of-range offset/scale, wrong types, short arrays, no write mid-drag |
| `slot_menu` | picker assigns, clear unassigns, capture assigns, escape cancels, reserved refused, confirm replaces / cancels |
| `hud_edit` | drag moves, drag clamps, selection, toggle, scale steps, orientation flip, exiting saves, no gameplay while editing |
| `unit_frame` | player/target/tot/party draw from a snapshot, empty target state, party grows and shrinks with the roster |

## 12. What was built, against the plan

Two things came out differently from the design above, both for reasons found while building:

- **The editor's control strip is a column in the top-left corner**, not a strip across the
  top. The default layout puts the target and target-of-target frames along the top edge, and
  a strip there sat on top of two of the things the editor exists to move. A test covers it:
  a press on a control never starts a drag on what is underneath, disabled controls included.
- **`HudEditor` gained a `RESET ALL` control.** Not in the brief, but a layout editor with no
  way back is one nobody experiments with, and the default arrangement was already the one
  authoritative value.

Everything else is as designed.

## 13. Definition of done

| item | where |
| --- | --- |
| up to 8 action bars | `ActionBarSet`, `ACTION_BARS` |
| each independently enabled | `HudLayout::set_enabled`, per-bar `HudPlacement` |
| horizontal or vertical | `HudOrientation`, `action_bar::slot_rect` |
| moved through Edit HUD | `HudEditor::update` |
| resolution-independent | `HudLayout::resolve`, anchor + normalized offset |
| positions persist | `hud.cfg`, `HudConfigManager` |
| right click assigns a spell | `SlotMenu`, `assignable_spells` |
| assignments persist | `ActionBarSet::write_config` / `from_config` |
| a keybind per slot | `ActionBarSlot::key` |
| keybinds persist | `KeyCode::name` / `from_name` |
| conflicts handled | `ActionBarSet::conflict`, `SlotMenuMode::Confirm` / `Refused` |
| player / target / target-of-target / party frames | `ui::unit_frame` |
| all four movable, scalable, toggleable | one `HudPlacement` each |
| clear visual feedback while editing | `push_hud_editor` |
| nothing becomes unreachable | `clamp_on_screen`, on resolve as well as on drag |
| validated configuration, safe defaults | `HudConfig::validated` |
| automated coverage | 125 new tests across five crates |
| existing behaviour intact | the whole workspace suite, unchanged and passing |

## 14. Known limitations

- **The party is a party of one.** `engine-game` has no ally entities, so nothing ever calls
  `Party::join` in the shipped game. The roster, the snapshot view and the frame are all real
  and all update dynamically; there is simply nobody to put in them yet. The alternative was
  a frame that invents four teammates, which the brief rules out.
- **A frame prints what an entity *is*, not who it is.** There are no character names in the
  world. `PlayerView` has no name field because the simulation has nothing to fill it with.
- **The editor has no undo beyond `RESET ALL`.** A misdrag is fixed by dragging back.
- **Elements can be dragged on top of one another.** Nothing snaps and nothing repels: the
  topmost is the one picked up, which is predictable but is not a layout guide.
- **There is no per-bar slot count.** Every bar has ten slots, which is `ABILITY_SLOTS` and
  the size the existing geometry, keys and art are all built around. A bar of six would be a
  configuration field and a change to `bar_size`; nothing in the model assumes ten twice.
- **`Bindings` itself is still not rebindable.** Movement, jump, targeting and the Ultimate
  keep their fixed keys — they are reserved, and the slot menu says so when one is pressed.
  A key-bindings screen is now a screen over an existing conflict check rather than a new
  system.
- **The old fixed HUD paths are still there.** `GameUi::build`, `push_ability_bar`,
  `push_target_frame` and `push_player_bars` are unchanged and still tested; the game calls
  `build_arranged` instead. They are what the collision, VFX and audio demo scenes would use
  if they ever wanted a HUD, and deleting them would have been a change to code this phase
  did not otherwise touch.

## 15. Edit HUD, the inspector, and the generic layout

### What the player sees

* A small **Edit Mode** badge, top centre. Nothing opaque covers the world.
* Every element outlined with a hairline and named with a chip; hidden ones are faint ghosts
  that light up under the pointer. The **selected** element is drawn in the accent with a glow,
  corner handles and a `Name  W×H` chip.
* The **inspector**, a translucent card docked right (left when the selection is under it):
  * nothing selected — every element by name (hidden ones marked with a crossed eye; the way
    back to anything hidden or tucked under another), **Nameplates**, the alignment **grid** and
    **snap to guides** switches, **Reset all**, **Done**;
  * an element — **Position** (3×3 anchor grid, X/Y steppers that keep the chosen anchor),
    **Size** (scale; width and height where the element has them), **Layout** (orientation,
    fill across, fill down, spacing — where it has items), **Visibility**, **Reset**, **Done**;
  * **Nameplates** — nameplate scale, **enemy debuff size**, enemy buff size (`settings.cfg`).
* Dragging snaps an edge or centre to the screen's edges and centre lines, other elements'
  edges and centres, and the grid when it is on; the guide in use is drawn in blue.

### One layout system

`HudElementId::controls()` is the one table of what each element offers; the inspector draws
whatever it says. Elements made of items lay them out through `ui::flow::Flow`, a grid filled in
either direction on either axis — direction is applied when a cell is *placed*, so the first
buff of three in a right-to-left bar is against the right edge. Buffs, raid cells, action bar
slots and party rows all use it, for painting and for hit-testing.

| element | layout | width / height |
| --- | --- | --- |
| action bars | orientation, fill across, fill down, spacing | — |
| buffs | fill across, fill down, spacing | icons per row (1–16), rows (1–4) |
| raid | fill across, fill down, spacing | — |
| party | fill down | — |
| cast bar | — | px (120–640), px (12–48) |
| tooltip dock | fill across, fill down (which way a tooltip grows) | px (300–560) |

### Persistence

All in `hud.cfg`, per element on `HudPlacement`: `flow_x`, `flow_y` (`auto` = the element's
natural direction, so an older file keeps its arrangement), `spacing`, `width`, `height`; the
tooltip dock is `frames.tooltip`; the editor's switches are `editor.grid` / `editor.snap`. The
nameplate icon sizes are `nameplates.debuff_size` / `buff_size` in `settings.cfg`. Every field
falls back on its own, like the rest of the file.

### Defaults

A fresh layout shows the **raid frames** where the party list was and hides the party list
(where the raid frames were, so it is never under them); the tooltip dock is bottom right.

### Shared kit

`ui::kit` holds the panel, button (primary / secondary / ghost / danger with hover, pressed,
active and disabled states), stepper, segmented control, switch, anchor grid, section heading
and chip — drawn from the map's tokens (`ui::map_style`) in the proportional face. The tooltip,
the editor and the action bar's labels are built from it.
