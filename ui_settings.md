# UI Framework & Settings

Working document for the UI/settings phase: what was built, what was tested, what was
measured, what was decided, and what is knowingly missing.

Status: **COMPLETE**. Every milestone below is DONE.

## Goals

A reusable UI system - not a menu with reusable parts - and the first two screens built on
it: a main menu and a settings screen with a camera-sensitivity slider whose value survives a
restart in `settings.cfg`.

The game is multiplayer, so **nothing here pauses it**. The menu is an overlay over a live
world: opening it skips no tick, holds no cooldown, and leaves the player's character exactly
as exposed as it was.

What the framework has to grow into without a rewrite: graphics settings, audio settings,
gameplay settings, key bindings, confirmation dialogs, dropdowns.

## 1. Architecture

```text
InputState (engine-core)
    |
    v
UiInput            pointer position, three pointer edges, one optional navigation intent
    |
    v
UiScreen<T>        widgets + focus, hit-testing, event emission
    |
    v
Widget             Button / Checkbox / Slider: state, a rule, a value
    |
    v
(T, WidgetEvent)   what happened, tagged by the screen's own item enum
    |
    v
MenuAction         what the game should do about it
    |
    v
App / SettingsManager / camera
```

Settings ownership, deliberately one-directional:

```text
GameSettings      values, bounds, the one definition of every default
    |
    v
SettingsManager   load, validate, hold, save
    |
    v
Settings UI       reads values, emits actions; never touches the file
```

The UI never writes `settings.cfg`, never calls into gameplay, and never terminates the
process. It emits values; the application applies them.

### Crates

| crate | what it gained |
| --- | --- |
| `engine-settings` | new. `GameSettings`, bounds, defaults, validation, `SettingsManager`, JSON |
| `engine-presentation` | new `ui` module: `input`, `widget`, `screen`, `menu`, `paint` |
| `engine-render-data` | `UiRect::contains` - half-open hit-testing |
| `engine-app` | wiring: menu update, quit, applying and saving settings |

`engine-settings` links no engine crate at all, so every load/validate/save rule is testable
with a temporary file. The `ui` module lives in `engine-presentation`, which links no D3D12,
so widget interaction, focus, layout and even what the menu *draws* are tested without a
device.

## 2. Widget system

| widget | states | emits |
| --- | --- | --- |
| `Button` | normal, hovered, pressed, focused, disabled | `Activated` |
| `Checkbox` | + checked / unchecked | `Toggled(bool)` |
| `Slider` | + dragging | `ValueChanged(f32)` per drag frame, `ValueCommitted(f32)` on release |

`WidgetState` is four independent flags (`hovered`, `pressed`, `focused`, `enabled`) rather
than one enum, because a widget can be focused *and* hovered *and* pressed at once.

Rules worth stating:

- activation is press **and** release on the same widget; releasing elsewhere does nothing
- a release on a widget that was never pressed does nothing - the tail of a drag started
  elsewhere must not click whatever it happens to end on
- a disabled widget takes no input, emits nothing, and cannot hold the focus
- a slider clamps and snaps every value it produces; it can never report outside `[min, max]`
- a slider being dragged keeps the handle even when the pointer leaves the track
- a degenerate slider range (reversed, non-finite) collapses to an immovable slider instead
  of reporting nonsense

`Checkbox` is implemented against the same interaction core as `Button` - it *is* a button
whose effect is a toggle - so the two can never disagree about what a click is.

## 3. Input routing

`UiInput` is the whole of what a widget sees:

- `pointer: (f32, f32)` in client pixels
- `pointer_down` / `pointer_pressed` / `pointer_released` - level plus both edges
- `nav: Option<Nav>` - `Prev`, `Next`, `Decrease`, `Increase`, `Activate`, `Cancel`

`UiInput::from_input` is the only place in the UI where a key code appears:

| device | intent |
| --- | --- |
| left mouse button | pointer |
| tab, down / up | `Next`, `Prev` |
| left / right | `Decrease`, `Increase` |
| enter, space | `Activate` |
| escape | `Cancel` |

Intents come from key *presses*, not holds, so a leaned-on arrow key cannot run a slider end
to end in three frames.

This is the seam a gamepad arrives through: a new source fills the same struct, and not one
widget changes.

## 4. Screen system

`UiScreen<T>` holds `(tag, widget)` pairs, one focus, and a reused event buffer. Events come
back tagged (`(PauseItem::Quit, Activated)`), never indexed, so reordering the layout cannot
silently change what a button does.

- hover: whichever widget the pointer is over, exactly one at a time
- pressed: whichever widget the pointer went down on
- focus: exactly one widget; moved by `Prev`/`Next` with wrapping, skipping disabled widgets,
  and also moved by a click so keyboard and mouse agree about where the player is
- `reset()` on close, so a hovered button does not light up before the player moves the mouse

`MenuSystem` owns both screens, the current `MenuScreen`, and the action buffer. It is the
only module that knows the game's menus exist.

## 5. Main menu

```text
         0.1.0
+-------------------------+
|        SETTINGS         |
|-------------------------|   <- rule drawn in the gap
|          QUIT           |
|-------------------------|   <- rule drawn in the gap
|         RESUME          |
+-------------------------+
```

The title is the running build's version, passed in by the application as
`env!("CARGO_PKG_VERSION")`. A single-player game would print `PAUSED` there; this one has
nothing to pause, and the build number is what a player reporting a bug needs to read off the
screen.

- ESC from gameplay opens it; ESC again closes it; RESUME closes it
- SETTINGS opens the settings screen; BACK or ESC returns to the main menu (not to the game)
- QUIT emits `MenuAction::QuitRequested`; the application calls `EngineContext::request_exit`,
  the loop finishes the frame, `shutdown` runs, the renderer drains its frames in flight, the
  window closes. Nothing calls a process-termination function.
- **the simulation keeps running.** `advance(real_delta)` is unconditional and gameplay
  commands are built every frame whether or not a menu is open, so the player can still move
  and the world still ticks. `MenuSystem` deliberately exposes no way to stop it - there is no
  `is_paused`, no `gameplay_delta`, no input gate - so no future screen can reintroduce a
  pause by accident.
- opening the menu cancels an in-progress mouse-look so the cursor comes back and buttons can
  be clicked; the camera itself is untouched

The visual grouping is asserted by a test, not eyeballed: SETTINGS above QUIT above RESUME,
and the gap above RESUME at least as large as the gap above QUIT.

## 6. Settings model

```rust
GameSettings { camera_sensitivity: f32 }   // multiplier; 1.0 = the camera's tuned rate
```

One definition of every bound, in `engine-settings`:

| constant | value |
| --- | --- |
| `CAMERA_SENSITIVITY_MIN` | 0.10 |
| `CAMERA_SENSITIVITY_MAX` | 5.00 |
| `CAMERA_SENSITIVITY_DEFAULT` | 1.00 |
| `CAMERA_SENSITIVITY_STEP` | 0.05 |

The camera does not hardcode a rate any more: `BASE_MOUSE_SENSITIVITY` (0.0014 rad/px) is the
tuned rate, and `ThirdPersonCamera::set_sensitivity` scales it. A setting of 1.0 is exactly
the previously tuned camera, which is why a default settings file changes nothing about how
the game plays. A new value applies to the very next mouse movement - no restart, no rebuild.

## 7. Settings persistence

`settings.cfg`, beside the executable (`std::env::current_exe()`'s directory).

```text
slider drag   -> SensitivityChanged  -> SettingsManager::set_camera_sensitivity + camera
slider release-> SensitivityCommitted-> ... + save_if_dirty()  (one write)
shutdown      ->                        save_if_dirty()        (catches keyboard changes)
```

Dragging never writes to disk. `SettingsManager` holds the value in memory with a dirty flag
and writes on commit only; a test asserts the file is untouched mid-drag.

## 8. Validation rules

Loading is total: it returns settings, never a `Result`, and never panics.

| input | result |
| --- | --- |
| file missing | defaults, file created |
| valid file | values loaded |
| missing field | that field defaults |
| wrong type (`"abc"`) | that field defaults |
| below minimum / above maximum | that field defaults |
| non-finite | that field defaults |
| malformed JSON | all defaults, file repaired |
| unreadable (permissions) | defaults in memory, file left alone |
| unknown fields | ignored; known fields still load |

For `camera_sensitivity`: finite, `>= 0.10`, `<= 5.00`.

Out-of-range values are **replaced by the default, not clamped**: `-100` is not a request for
the minimum, it is a value that means nothing. The file is rewritten whenever what was read
differs from what is in use, so `settings.cfg` always describes the settings actually running.

## 9. JSON format

```json
{
  "camera_sensitivity": 1.0
}
```

Written pretty-printed with a trailing newline, because the file exists to be hand-edited.
Serialized with `serde` + `serde_json`; `#[serde(default)]` on the struct is what makes a
missing field a default rather than a parse error, and what makes unknown fields harmless.

## Milestones

| # | milestone | status |
| --- | --- | --- |
| 1 | `ui_settings.md` | DONE |
| 2 | UI architecture | DONE |
| 3 | Button widget | DONE |
| 4 | Checkbox widget | DONE |
| 5 | Range slider | DONE |
| 6 | Focus and navigation | DONE |
| 7 | Screen system | DONE |
| 8 | ESC menu (non-blocking) | DONE |
| 9 | Resume behaviour | DONE |
| 10 | Quit | DONE |
| 11 | Settings model | DONE |
| 12 | Camera sensitivity | DONE |
| 13 | Settings file format | DONE |
| 14 | Default settings | DONE |
| 15 | Settings file creation | DONE |
| 16 | Settings validation | DONE |
| 17 | Save behaviour | DONE |
| 18 | Settings reload | DONE |
| 19 | Invalid settings tests | DONE |
| 20 | Settings UI behaviour | DONE |
| 21 | Visual polish | DONE |
| 24 | Automated UI tests | DONE |
| 25 | Manual verification | DONE |
| 26 | Performance validation | DONE |
| 27 | Full regression | DONE |

## 10. Tests

All automated tests live outside the production paths they exercise - in-module `#[cfg(test)]`
suites, following the project's existing test architecture.

| area | file | count | covers |
| --- | --- | --- | --- |
| input routing | `ui/input.rs` | 4 | pointer edges, key-to-intent table, no auto-repeat, idle frame |
| widgets | `ui/widget.rs` | 21 | button normal/hover/press/activate/disabled, release-off, release-without-press, keyboard activation; checkbox toggle/set/disabled; slider min/max/midpoint/clamp/step/drag/commit/keyboard/disabled/degenerate range/NaN |
| screens | `ui/screen.rs` | 12 | hover, press, tagged events, focus wrap, focus skips disabled, one focus flag, click moves focus, keyboard activation, reset, no per-frame allocation, empty screen |
| menus | `ui/menu.rs` | 18 | ESC opens/closes, resume, quit request, settings open/back, ESC from settings, full journey, no way to stop the game, version title, layout hierarchy, widgets inside panel, slider drag/commit, range never exceeded, applied settings, keyboard-only journey, hover reset, closed menu ignores input, repeated open/close, no allocation |
| drawing | `ui/paint.rs` | 10 | closed menu draws nothing, menu contents + scrim + version title (no PAUSED), settings shows name/min/max/value/back, printed bounds, handle stays on track, hover changes pixels, focus outline, group rules, no allocation after first frame, every glyph exists, checkbox drawing |
| camera | `camera.rs` | 4 new | sensitivity scales the tuned rate on both axes, applies immediately, rejects nonsense, base rate is half the original |
| settings | `engine-settings/src/lib.rs` | 19 | defaults, JSON round trip, missing file, valid file, missing field, wrong type, below min, above max, non-finite, malformed, unknown fields, empty file, in-memory-then-save, restart persistence, snapping, NaN snap, invalid update, pretty output, game directory |
| hit-testing | `engine-render-data/src/ui.rs` | 1 new | half-open rectangles: adjacent widgets never both claim a point |

Workspace result:

```text
cargo fmt --all -- --check                              clean
cargo clippy --workspace --all-targets -- -D warnings   clean
cargo test --workspace                                  exit 0, 54 suites, 0 failed
cargo build --workspace                                 ok
cargo build --workspace --release                       ok
```

`engine-presentation` went from 146 to 210 lib tests; `engine-settings` adds 19.

## 11. Performance measurements

Measured on the release build at 1280x720, 1 s reporting interval, seven samples per phase,
from the engine's own report line (RX 6700 XT):

| phase | CPU frame | UI build | UI quads | UI commands | draw calls |
| --- | --- | --- | --- | --- | --- |
| gameplay, no menu | 0.203 ms | 0.004 ms | 214 | 77 | 5 |
| main menu open | 0.213 ms | 0.004 ms | 242 | 86 | 5 |
| settings menu open | 0.223 ms | 0.005 ms | 267 | 91 | 5 |

- **draw calls do not change.** The menu is appended to the draw list the HUD already builds,
  so it is batched into the same UI pass - no extra pass, no extra pipeline, no extra buffer.
- **no GPU resources are created by interaction.** Opening a menu, hovering, dragging and
  closing all mutate plain data; the renderer sees only the per-frame draw list it already
  consumed.
- **no per-frame allocation.** Widgets and screens are built once at startup; the action
  buffer, the event buffer, the draw list and the number-formatting scratch are cleared and
  reused. Four tests assert capacities do not move over 1000 frames of interaction.
- **a closed menu costs nothing**: `push_menu` returns immediately and a test asserts the
  draw list is empty when no menu is open. The 0.01 ms difference between phases is the
  menu's own commands, not a change to the gameplay path.

## 12. Manual verification

Driven against the real release build with synthetic Win32 input, screenshots captured per
step (client area, 1280x720).

| check | result |
| --- | --- |
| ESC in gameplay | menu appears over a dimmed world, version as the title, SETTINGS focused |
| holding W with the menu open | the character keeps moving; the world keeps ticking |
| SETTINGS button | settings screen appears |
| slider legibility | `CAMERA SENSITIVITY`, `0.10` at the left, `5.00` at the right, current value under the handle |
| dragging the slider | handle and value track the pointer; camera rate changes immediately |
| value after release | `settings.cfg` contains `3.8` - exactly the snapped value the screen showed (3.80) |
| BACK | returns to the main menu |
| RESUME | menu closes, gameplay resumes, movement works again |
| restart | the slider shows 3.80 on the next launch; the log reports loading it |
| QUIT | `quit requested from the menu` -> `clean shutdown`, process exits by itself |
| malformed `settings.cfg` | game starts, warns, uses defaults, rewrites the file as valid JSON |
| `{"camera_sensitivity": -100, "future_setting": true}` | starts at the default 1.00, unknown field ignored, file repaired |

Log excerpt from the menu session:

```text
menu opened
settings: camera sensitivity 3.80 saved
menu closed
menu opened
quit requested from the menu
clean shutdown
```

## 13. Known limitations

- **The checkbox is built, tested and drawn, but nothing in the game uses one yet.** There is
  no boolean setting to bind it to. It is included because the brief asks for the widget and
  because the next settings screen should be a line of layout, not a new renderer path.
- **The menu is drawn at a fixed pixel size and centred.** At 4K it is physically smaller than
  at 1080p. The HUD has the same property for the same reason (a bitmap font only looks right
  at integer scales); a UI scale setting is the obvious future fix.
- **No mouse-wheel handling in the UI.** Nothing scrolls yet, so a settings screen taller than
  the panel would need a scroll container.
- **The menu and gameplay share the keyboard.** Because the game does not stop, keys reach
  both: space jumps *and* activates the focused widget, tab cycles targets *and* moves the
  focus. The pointer is unambiguous, and arrow keys plus enter drive the menu without touching
  gameplay, but a proper input-focus policy (menu swallows what it uses) is the obvious next
  step and is the one thing here that key rebinding will want to settle.
- **`engine-core`'s `Time::pause` still exists.** It is a Phase 1 clock feature, unused by the
  game and untouched by this phase; the pause that was removed is the gameplay one.
- **The settings screen has no explicit "reset to defaults".** `GameSettings::default()` exists
  and `apply_settings` would push it into the widgets; only the button is missing.
- **`serde` and `serde_json` are the project's first third-party dependencies** beyond `log`
  and `windows`. The alternative was a hand-written JSON parser, which the brief rules out and
  which is exactly the sort of code that turns a malformed config into a crash.
- Milestones 22 and 23 do not exist in the brief; the numbering skips them.

## 14. Future extension points

Adding a setting is four small edits and touches no framework code:

1. a field on `GameSettings` with a bound and a default constant
2. one line in `GameSettings::validated`
3. a widget pushed onto the settings screen with its tag
4. an arm in the application's `MenuAction` match

Worked examples:

- **VSync, fullscreen, bloom** - `Checkbox`, already implemented and drawn. The action carries
  a `bool`; the application applies it to the renderer and saves it.
- **Audio volume** - `Slider::new("MASTER VOLUME", 0.0, 1.0, 0.01, value)`. Continuous sliders
  are supported (a zero step means continuous, and the keyboard still nudges by 1% of range).
- **Graphics quality / resolution** - a dropdown is a new widget: a struct with an `update`, a
  variant in `Widget`, an arm in `paint`. Nothing existing changes. The screen, focus,
  navigation and event plumbing already handle it.
- **Key bindings** - a new screen: a tag enum, a `UiScreen<T>`, a layout function, an arm in
  `MenuSystem`. Capturing a key needs one new input source feeding `UiInput`, which is the
  same seam a gamepad would use.
- **Confirmation dialogs** - a `MenuScreen` variant with two buttons; the state machine
  already supports pushing a screen and returning from it (settings does exactly that).
- **Gamepad support** - fill `UiInput.nav` from a pad. No widget, screen or menu changes.

## Example `settings.cfg`

```json
{
  "camera_sensitivity": 1.0
}
```

## Revision - multiplayer: no pausing

The phase originally shipped an ESC menu that froze the simulation. The game is multiplayer,
so that was removed outright rather than made optional:

- `MenuSystem` lost `is_paused`, `gameplay_delta` and `accepts_gameplay_input`; there is now
  no API by which a screen could stop the world.
- `MenuAction::Paused`/`Resumed` became `Opened`/`Closed`.
- The application advances the simulation with the real delta unconditionally and builds
  gameplay commands every frame, menu open or not.
- The panel's `PAUSED` heading became the running build's version, supplied by the
  application as `env!("CARGO_PKG_VERSION")` so the UI crate does not hardcode it.
- A test asserts the menu exposes nothing that could stop the game, and another asserts the
  title is the version and not `PAUSED`.

## Milestone log

### Milestone 1 - `ui_settings.md` - DONE

This document, created before any production code was touched. It fixed the architecture, the
widget set, the input model, the settings model and the validation table up front.

Decision: a dedicated `engine-settings` crate rather than extending `engine-core::config`.
`EngineSettings` is engine wiring validated once at startup and never changed; `GameSettings`
is player-facing, changes at runtime, and is persisted. Giving `engine-core` a `serde`
dependency to serve the second would put it in every crate that depends on it.

### Milestones 11, 13, 14, 15, 16, 19 - settings model, format, defaults, creation, validation - DONE

Implementation: `crates/engine-settings`. `GameSettings` with `#[serde(default)]`; bounds and
defaults as `pub const`; `validated()` replaces non-finite and out-of-range fields;
`snap_camera_sensitivity` clamps and snaps; `SettingsManager` loads (never failing), holds,
tracks dirty, and writes only when asked; the file is repaired whenever it does not describe
the settings in use.

Tests: 19, all passing - every row of the validation table plus save/restart behaviour.

Decision: replace rather than clamp out-of-range values. Recorded because the brief allowed
either policy.

### Milestones 2-6 - UI architecture, button, checkbox, slider, focus - DONE

Implementation: `ui/input.rs` (device-independent `UiInput`), `ui/widget.rs` (`WidgetState`,
`Button`, `Checkbox`, `Slider`, the `Widget` enum), `ui/screen.rs` (`UiScreen<T>` with focus,
hover, press and tagged events). `UiRect::contains` added to `engine-render-data` with
half-open edges so adjacent widgets never both claim a point.

Tests: 37 across the three modules.

Decisions: events rather than callbacks - a callback holding `&mut` game state would force the
whole menu into `Rc<RefCell<..>>`. An enum rather than `Box<dyn Widget>` - three `Copy` kinds
in one contiguous allocation. Retained rather than immediate mode - an immediate-mode tree is
the per-frame allocation this engine spends its time avoiding.

### Milestones 7-10, 20, 21 - screens, menu, resume, quit, settings screen, polish - DONE

Implementation: `ui/menu.rs` (`MenuSystem`, `MenuScreen`, `PauseItem`, `SettingsItem`,
`MenuAction`, layout) and `ui/paint.rs` (`MenuTheme`, `MenuScratch`, `push_menu`). The
application gained `update_menu` and `apply_sensitivity`.

Tests: 28 across menu and paint, including the visual-hierarchy assertion and the
"closed menu draws nothing" property.

Revision: the menu originally froze the simulation while it was open. That was removed for
multiplayer - a shared world cannot stop for one client - along with `is_paused`,
`gameplay_delta` and `accepts_gameplay_input`, and the `Paused`/`Resumed` actions became
`Opened`/`Closed`. The panel title became the build version, which is what the space the
`PAUSED` heading occupied is now worth.

Decisions: drawing is a separate module reading the same data, so restyling changes no
behaviour and widget tests never mention a colour. The menu is appended to the HUD's own draw
list so the interface stays one batch.

### Milestone 12 - camera sensitivity - DONE

Implementation: `BASE_MOUSE_SENSITIVITY` is now public and multiplied by
`ThirdPersonCamera::sensitivity`; the application seeds it from the loaded settings at startup
and updates it on every `SensitivityChanged`. The camera never reads a file.

Tests: 4 new camera tests. Manual: dragging the slider changes the camera immediately.

### Milestones 17, 18 - save behaviour and reload - DONE

Implementation: change on drag, write on commit, plus a `save_if_dirty` on shutdown.

Manual: dragged to 3.80, exited, inspected `settings.cfg` (`3.8`), restarted, and the settings
screen showed 3.80.

### Milestones 24-27 - tests, manual verification, performance, regression - DONE

Recorded in sections 10, 11 and 12 above.
