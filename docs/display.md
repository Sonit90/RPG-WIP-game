# Display settings: resolution and window mode

**SETTINGS → GRAPHICS** sets the resolution and the window mode (and, below them, the shadow
quality — see `shadows.md`). A display change is applied, then confirmed: if the player does
not press **KEEP CHANGES** within 15 seconds (or presses **REVERT**, or escape), the previous
configuration comes back. Only a kept change is saved. **Alt+Enter** switches between a window
and fullscreen at any time, except in the editor.

```text
┌──────────── GRAPHICS ───────────┐
│ RESOLUTION                      │
│ [ 1920 x 1080               ▼ ] │
│ WINDOW MODE                     │
│ [ WINDOWED                  ▼ ] │
│ A FRAMED WINDOW ...             │
│ NOW: 1920 x 1080, WINDOWED      │
│ SHADOW QUALITY                  │
│ [ HIGH                      ▼ ] │
│ SHARPER SHADOWS ...             │
│ [ APPLY      ]  [ BACK       ]  │
└─────────────────────────────────┘
```

Choosing from a dropdown changes nothing; **APPLY** carries out every choice at once and
**BACK** forgets them. A display change is then confirmed (it can leave the player looking at
a black screen); a shadow level is simply applied and saved. Both changed: the shadows are
applied, then the display goes to the confirmation, and REVERT undoes only the display.

## The editor is always a window

Opening the world editor (F6, `--editor`, or QUIT TO EDITOR) from borderless or exclusive
fullscreen switches the game to a framed window: the saved resolution if a window that size
fits the monitor's work area, otherwise the same shape shrunk until it does
(`DisplaySettings::for_editor`). Nothing is saved; leaving the editor puts the previous mode
back. Alt+Enter does nothing while the editor is open, and a display change still waiting for
KEEP CHANGES is reverted on the way in. Started with `--editor`, the window is created as that
window directly, so the monitor never switches mode for a game nobody is playing yet.

## Modes

| mode (menu) | `window_mode` | what happens |
| --- | --- | --- |
| WINDOWED | `windowed` | a framed, resizable window; the resolution is its client size; centred on the work area of its monitor |
| BORDERLESS FULLSCREEN | `borderless_fullscreen` | a borderless popup covering the monitor at the **desktop's** resolution; no mode change, instant alt-tab. The resolution list is greyed out and says which size is used |
| FULLSCREEN | `fullscreen` | the monitor is **switched** to the chosen resolution (`ChangeDisplaySettingsExW`, `CDS_FULLSCREEN`) and a borderless popup covers it |

Exclusive fullscreen is a real display-mode switch made by the platform layer, not by DXGI:
the swap chain stays flip-model and is resized through the same path as any other resize, and
Windows promotes a fullscreen flip-model window to independent flip. `CDS_FULLSCREEN` makes the
switch temporary — Windows undoes it when the process ends, even by crashing. While the game
is not in front (alt-tab) the desktop mode is restored and the window minimizes; coming back
switches again. If the monitor refuses the mode, the window falls back to borderless and the
menu shows that.

## The file

```json
"display": {
  "resolution_width": 2560,
  "resolution_height": 1440,
  "window_mode": "borderless_fullscreen"
}
```

In `settings.cfg`, beside everything else. A missing section, a size outside 640..16384 or an
unknown mode falls back to the default (1920 x 1080, windowed) field by field. At startup the
saved resolution is checked against what the monitor offers and replaced by the closest one it
does (same shape preferred) before the window is created; the swap chain is then created at its
final size.

The command line overrides the file for one run without changing it: `--width`, `--height` or
`--fullscreen`, and every unattended run (`--hidden`, `--benchmark`, `--screenshot`,
`--frames`) uses exactly the window it asked for.

## The resolution list

From the monitor the window is on (`MonitorFromWindow`, `EnumDisplaySettingsW`): every size at
24-bit colour or deeper, each once whatever its refresh rates, nothing smaller than 640 wide or
larger than the desktop. The common sizes (3840x2160, 3440x1440, 2560x1440, 1920x1200,
1920x1080, 1600x900, 1366x768, 1280x720) the monitor supports, the desktop's own size and the
current one are always listed; other sizes fill up to twelve rows. Largest first. An
exclusive switch uses the desktop's refresh rate when the size offers it, otherwise the highest.

Labels read `1920 x 1080`: the menus use the 5x7 bitmap font, which is ASCII-only.

## Where the code is

| layer | file | what |
| --- | --- | --- |
| rules | `engine-settings/src/display.rs` | `DisplaySettings`, `WindowMode`, validation, `resolution_options`, `closest_resolution`, the `DisplayConfirmation` state machine |
| platform | `engine-platform/src/display.rs` | monitors, their modes, switching and restoring a mode |
| platform | `engine-platform/src/window.rs` | `Window::apply_display`, alt-tab in exclusive mode, `WM_DISPLAYCHANGE`, Alt+Enter without the system beep, restoring the monitor on drop |
| interface | `engine-presentation/src/ui/graphics_menu.rs`, `ui/widget.rs` (`Dropdown`), `ui/paint.rs` | the screen, the dropdowns, the dialog |
| application | `engine-app/src/app.rs` | startup, APPLY / KEEP / REVERT, the countdown, Alt+Enter, display-change events, the editor's window |

Nothing in the renderer changed: a mode change produces `WM_SIZE`, the application calls
`Renderer::resize` (swap chain, depth, contact-shadow targets) and `Camera::set_viewport`
exactly as for a dragged window edge, and the interface lays itself out against the new size.

## Tests

- `engine-settings` `display` (8): defaults, the file round trip, a bad mode or size costing
  only that field, fallback to the closest size, the list (unique, sane, common first, largest
  first, ASCII labels), keep / revert / timeout, the editor's window fitting the work area;
  `lib` persists a kept change and repairs a bad section.
- `engine-platform` `display` (2) and `tests/window_display.rs` (4, plus 1 `#[ignore]`d):
  monitor description, best mode, windowed size and centring, borderless covering the monitor
  and back, zero sizes refused. The ignored test switches the real monitor:
  `cargo test -p engine-platform --test window_display -- --ignored`.
- `engine-presentation`: `Dropdown` (3), `graphics_menu` (7: pending vs in force, borderless
  greying, an open list taking the click, the dialog, discarding, a lost size), `menu` (the
  screen from SETTINGS, layout at three resolutions, APPLY and escape-reverts as actions, a
  shadow level waiting for APPLY, BACK forgetting it, one APPLY carrying both),
  `paint` (the screen's text, the dialog's countdown).
- `engine-app` `args`: when the saved display applies.
- Manual: MT-034 in `manual-tests.md`.
