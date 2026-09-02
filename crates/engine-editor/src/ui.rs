//! What the editor looks like: a palette down the left, a status line, and a key list.
//!
//! ```text
//!  ┌──────────────┐
//!  │ PLACE        │   the sixteen library assets, the current one highlighted
//!  │ ground_plain │
//!  │ arena_floor  │
//!  │ ...          │
//!  ├──────────────┤
//!  │ placed pillar│   what just happened
//!  │ grid 1 m     │
//!  └──────────────┘
//!                              LMB select · drag a row in to place · X/Y/Z arms move
//!                              X delete · C copy · Z undo · G grid · CTRL+S save · F6 play
//! ```
//!
//! It also paints the *world* overlay: what is selected, the gizmo standing on it, and — when
//! the camera has turned away from the selection — a striped silhouette at the edge of the
//! window saying where it went. See [`push_world_overlay`].
//!
//! It paints into the same [`UiDrawList`] the HUD and the menus use, so the whole interface
//! is still one batch and one draw call — see `engine_presentation::ui::paint`.

use engine_core::math::{Quat, Vec3};
use engine_environment::EnvironmentScene;
use engine_presentation::ui::paint::MenuTheme;
use engine_render_data::ui::{UiColor, UiDrawList, UiRect, UiScale};

use crate::browser::BrowserRow;
use crate::{Editor, Selected, hierarchy_len, hierarchy_row_at, pick};

/// Layout constants, in logical pixels.
pub mod layout {
    /// Width of the asset browser panel.
    pub const PANEL_WIDTH: f32 = 230.0;
    /// Height of one palette row.
    pub const ROW_HEIGHT: f32 = 22.0;
    /// Gap between rows.
    pub const ROW_GAP: f32 = 2.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 8.0;
    /// Height of the panel's heading.
    pub const HEADING: f32 = 22.0;
    /// Distance from the left and top edges of the screen.
    pub const MARGIN: f32 = 12.0;
    /// Height reserved under the browser for the filter and the status lines.
    pub const STATUS_HEIGHT: f32 = 58.0;
    /// How much of the window's height the asset browser may take.
    pub const BROWSER_FRACTION: f32 = 0.72;
    /// How far one level of nesting indents a browser row.
    pub const INDENT: f32 = 10.0;

    /// Width of the hierarchy and inspector panels down the right-hand side.
    pub const SIDE_WIDTH: f32 = 210.0;
    /// Height of the inspector panel under the hierarchy.
    pub const INSPECTOR_HEIGHT: f32 = 134.0;
    /// Gap between the hierarchy panel and the inspector under it.
    pub const PANEL_GAP: f32 = 8.0;
    /// How much of the window's height the hierarchy list may take.
    pub const HIERARCHY_FRACTION: f32 = 0.55;
}

/// Where the hierarchy panel sits, in physical pixels.
///
/// Down the right-hand side, opposite the palette: the palette answers "what am I about to
/// place" and the hierarchy answers "what is already here", and an author reads them
/// alternately rather than together.
#[must_use]
pub fn hierarchy_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    let margin = scale.px(layout::MARGIN);
    let width = scale.px(layout::SIDE_WIDTH);
    let height = (viewport.1 as f32 * layout::HIERARCHY_FRACTION).max(scale.px(120.0));
    UiRect::new(viewport.0 as f32 - margin - width, margin, width, height)
}

/// Where the inspector panel sits, in physical pixels: under the hierarchy.
#[must_use]
pub fn inspector_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    let hierarchy = hierarchy_rect(scale, viewport);
    UiRect::new(
        hierarchy.x,
        hierarchy.bottom() + scale.px(layout::PANEL_GAP),
        hierarchy.width,
        scale.px(layout::INSPECTOR_HEIGHT),
    )
}

/// How many hierarchy rows fit in the panel.
///
/// A count rather than a scroll bar: the list scrolls with the wheel, and a scroll bar is a
/// widget with a drag, a thumb and a track that buys nothing an author of a few hundred
/// objects would notice.
#[must_use]
pub fn hierarchy_visible_rows(scale: UiScale, viewport: (u32, u32)) -> usize {
    let panel = hierarchy_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let heading = scale.px(layout::HEADING);
    let row = scale.px(layout::ROW_HEIGHT) + scale.px(layout::ROW_GAP);
    if row <= 0.0 {
        return 0;
    }
    (((panel.height - padding * 2.0 - heading) / row)
        .floor()
        .max(0.0)) as usize
}

/// Where one hierarchy row sits, counting from the top of the visible window.
#[must_use]
pub fn hierarchy_row_rect(scale: UiScale, viewport: (u32, u32), slot: usize) -> UiRect {
    let panel = hierarchy_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let row = scale.px(layout::ROW_HEIGHT);
    let gap = scale.px(layout::ROW_GAP);
    let top = panel.y + padding + scale.px(layout::HEADING) + slot as f32 * (row + gap);
    UiRect::new(panel.x + padding, top, panel.width - padding * 2.0, row)
}

/// Where the asset browser sits, in physical pixels.
///
/// Down the left-hand side, opposite the hierarchy: the browser answers "what am I about to
/// place" and the hierarchy answers "what is already here", and an author reads them
/// alternately rather than together.
#[must_use]
pub fn browser_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    let margin = scale.px(layout::MARGIN);
    let width = scale.px(layout::PANEL_WIDTH);
    // Clamped so the panel is reachable on a short window rather than hanging off the bottom
    // of it - the same rule the HUD's own panels follow.
    let height = (viewport.1 as f32 * layout::BROWSER_FRACTION)
        .max(scale.px(160.0))
        .min((viewport.1 as f32 - margin * 2.0).max(scale.px(160.0)));
    UiRect::new(margin, margin, width, height)
}

/// Where the panel down the left sits. The browser's rectangle, under the name the rest of
/// the editor knew it by when it held a palette.
#[must_use]
pub fn panel_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    browser_rect(scale, viewport)
}

/// How many browser rows fit in the panel, above its filter and status lines.
#[must_use]
pub fn browser_visible_rows(scale: UiScale, viewport: (u32, u32)) -> usize {
    let panel = browser_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let heading = scale.px(layout::HEADING);
    let status = scale.px(layout::STATUS_HEIGHT);
    let row = scale.px(layout::ROW_HEIGHT) + scale.px(layout::ROW_GAP);
    if row <= 0.0 {
        return 0;
    }
    (((panel.height - padding * 2.0 - heading - status) / row)
        .floor()
        .max(0.0)) as usize
}

/// Where one browser row sits, counting from the top of the visible window.
#[must_use]
pub fn browser_row_rect(scale: UiScale, viewport: (u32, u32), slot: usize) -> UiRect {
    let panel = browser_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let row = scale.px(layout::ROW_HEIGHT);
    let gap = scale.px(layout::ROW_GAP);
    let top = panel.y + padding + scale.px(layout::HEADING) + slot as f32 * (row + gap);
    UiRect::new(panel.x + padding, top, panel.width - padding * 2.0, row)
}

/// Draws the editor: its palette, what is selected, and the keys.
///
/// Nothing is drawn when the editor is closed, so the application can call this
/// unconditionally and a shipping build simply never has it to call.
pub fn push_editor(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    theme: &MenuTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    if !editor.is_active() {
        return;
    }
    let scale = editor.scale();
    let panel = panel_rect(scale, viewport);
    let border = scale.px(theme.border_pixels);
    let text = scale.text(theme.label_scale);
    let padding = scale.px(layout::PADDING);

    // The world first, so the panels are painted over it: an outline that ran across the
    // browser would be a selection marker floating on a list of file names.
    push_world_overlay(list, editor, scene, scratch, viewport);

    list.rect(panel, theme.panel);
    list.outline(panel, border, theme.panel_border);

    // The heading counts what the scan found, so "the directory is empty" and "the scan found
    // nothing" are two different things on screen rather than one blank panel.
    let counts = editor.browser().catalogue().counts();
    scratch.clear();
    scratch.push_str("ASSETS ");
    write_count(scratch, counts[0]);
    scratch.push_str(" MODELS");
    list.text_at(
        (panel.x + padding, panel.y + padding),
        scratch,
        theme.title,
        text,
    );

    let visible = browser_visible_rows(scale, viewport);
    let first = editor.browser().scroll();
    for slot in 0..visible {
        let Some(row) = editor.browser().row(first + slot) else {
            break;
        };
        let rect = browser_row_rect(scale, viewport, slot);
        let selected = editor.browser().is_selected(row);
        if selected {
            list.rect(rect, theme.panel_border);
            list.outline(rect, border, theme.title);
        }
        let (depth, label) = editor.browser().label(row);
        // A row that cannot be placed - a material, a texture, a level - is dimmed rather
        // than hidden: it is content, and an author looking for it must be able to see that
        // it arrived.
        let placeable = match row {
            BrowserRow::Builtin(_) => true,
            BrowserRow::Entry(index) => editor
                .browser()
                .catalogue()
                .entries()
                .get(index)
                .is_some_and(|entry| entry.kind.is_placeable()),
            BrowserRow::Folder(_) | BrowserRow::BuiltinFolder => true,
        };
        let color = if selected {
            theme.title
        } else if placeable {
            theme.text
        } else {
            theme.text_disabled
        };
        list.text_at(
            (
                rect.x + padding * 0.5 + scale.px(layout::INDENT) * depth as f32,
                rect.y + padding * 0.25,
            ),
            &label,
            color,
            text,
        );
    }

    // Under the browser: the filter, what just happened, and what is selected.
    let status_top = panel.bottom() - scale.px(layout::STATUS_HEIGHT);
    list.rect(
        UiRect::new(
            panel.x + border,
            status_top,
            panel.width - border * 2.0,
            border,
        ),
        theme.separator,
    );
    scratch.clear();
    if editor.browser().is_typing() {
        scratch.push_str("FIND: ");
        scratch.push_str(editor.browser().filter());
        scratch.push('_');
    } else if editor.browser().filter().is_empty() {
        scratch.push_str("F FIND   F2 RESCAN");
    } else {
        scratch.push_str("FIND: ");
        scratch.push_str(editor.browser().filter());
    }
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5),
        scratch,
        if editor.browser().is_typing() {
            theme.title
        } else {
            theme.text_disabled
        },
        text,
    );
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5 + text * 10.0),
        editor.status(),
        theme.text,
        text,
    );

    scratch.clear();
    match editor.selection() {
        Some(Selected::Object(index)) => match scene.objects.get(index) {
            Some(object) => {
                scratch.push_str(object.asset.name());
                scratch.push_str(" @ ");
                write_metres(scratch, object.position().x);
                scratch.push(' ');
                write_metres(scratch, object.position().z);
            }
            None => scratch.push_str("nothing selected"),
        },
        Some(Selected::Water(index)) => match scene.water.get(index) {
            Some(body) => {
                scratch.push_str("water @ ");
                write_metres(scratch, body.center.x);
                scratch.push(' ');
                write_metres(scratch, body.center.z);
            }
            None => scratch.push_str("nothing selected"),
        },
        Some(Selected::Spawn(index)) => match scene.spawn_points.get(index) {
            Some(spawn) => {
                scratch.push_str("spawn @ ");
                write_metres(scratch, spawn.position.x);
                scratch.push(' ');
                write_metres(scratch, spawn.position.z);
            }
            None => scratch.push_str("nothing selected"),
        },
        None => scratch.push_str("nothing selected"),
    }
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5 + text * 20.0),
        scratch,
        theme.text_disabled,
        text,
    );

    // The object count, so an author can see a scene growing past what they meant to build.
    scratch.clear();
    scratch.push_str("objects ");
    write_count(scratch, scene.len());
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5 + text * 30.0),
        scratch,
        theme.text_disabled,
        text,
    );

    push_hierarchy(list, editor, scene, theme, scratch, viewport);
    push_inspector(list, editor, scene, theme, scratch, viewport);

    // The key list, bottom left, out from under the two right-hand panels.
    // Grouped by *what an author is building* rather than by key, because the question
    // somebody has in front of a level editor is "how do I put a cave in this" and not "what
    // does M do". Cliffs and rocks have no group of their own: they are assets, placed from
    // the browser like everything else, and saying so is the point.
    let keys = [
        "PLACE    DRAG A ROW INTO THE WORLD   OR DOUBLE-CLICK IT",
        "ASSETS   TAB NEXT   CTRL+LMB ADD   F FIND   F2 RESCAN",
        "TERRAIN  B SCULPT   TAB MODE   1 2 SIZE   3 4 STRENGTH",
        "CAVES    M MODULE SNAP: 4 M GRID, QUARTER TURNS",
        "WATER    V POUR ONE UNDER THE CURSOR",
        "SPAWN    P PUT ONE   R TURNS IT   FIRST ONE IS USED",
        "SELECT   LMB PICKS ONE   ESC DROPS IT   CTRL+A ALL",
        "MOVE     DRAG THE X Y Z ARMS   ARROWS NUDGE   R YAW   T SIZE",
        "         X DELETE   C COPY   Z UNDO   L LOCAL AXES",
        "GRID     G STEP   SHIFT+G ROTATION   0 RESET TRANSFORM",
        "FILE     CTRL+S SAVE   F6 BACK TO PLAYING",
    ];
    let line = text * 12.0;
    let bottom = viewport.1 as f32 - scale.px(layout::MARGIN);
    for (index, key) in keys.iter().enumerate() {
        let y = bottom - line * (keys.len() - index) as f32;
        list.text_at(
            (scale.px(layout::MARGIN), y),
            key,
            theme.text_disabled,
            text,
        );
    }
}

/// The hierarchy: every object in the level, in the order the file holds them.
///
/// Rows are painted rather than made of [`Button`](engine_presentation::ui::widget::Button)s
/// because a row's label is `"12 PILLAR"` — a string built from the scene, which a widget's
/// `&'static str` label cannot hold. The hit test lives in [`crate::Editor::update`] and uses
/// [`hierarchy_row_rect`], so the rectangle a click is tested against and the rectangle drawn
/// are one function.
fn push_hierarchy(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    theme: &MenuTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let panel = hierarchy_rect(scale, viewport);
    let border = scale.px(theme.border_pixels);
    let text = scale.text(theme.label_scale);
    let padding = scale.px(layout::PADDING);

    list.rect(panel, theme.panel);
    list.outline(panel, border, theme.panel_border);

    scratch.clear();
    scratch.push_str("HIERARCHY ");
    write_count(scratch, hierarchy_len(scene));
    if editor.selected().len() > 1 {
        scratch.push_str("  SEL ");
        write_count(scratch, editor.selected().len());
    }
    list.text_at(
        (panel.x + padding, panel.y + padding),
        scratch,
        theme.title,
        text,
    );

    let rows = hierarchy_len(scene);
    let visible = hierarchy_visible_rows(scale, viewport);
    let first = editor.hierarchy_scroll().min(rows);
    for slot in 0..visible {
        let Some(row) = first.checked_add(slot) else {
            break;
        };
        // One row index over both lists: the placements, and then the water. A body of water
        // is a row like any other because it is a thing in the level an author points at,
        // and hiding it would leave the only pond in a valley unfindable in plan view.
        let Some(what) = hierarchy_row_at(row, scene) else {
            break;
        };
        let rect = hierarchy_row_rect(scale, viewport, slot);
        let selected = editor.is_selected(what);
        let primary = editor.selection() == Some(what);
        if selected {
            list.rect(rect, theme.panel_border);
            // Only the primary selection is outlined: with a dozen rows highlighted, the one
            // the inspector is describing has to be findable at a glance.
            if primary {
                list.outline(rect, border, theme.title);
            }
        }
        scratch.clear();
        match what {
            Selected::Object(index) => {
                write_count(scratch, index);
                scratch.push(' ');
                if let Some(object) = scene.objects.get(index) {
                    scratch.push_str(object.asset.name());
                }
            }
            Selected::Water(index) => {
                write_count(scratch, index);
                scratch.push_str(" WATER");
            }
            Selected::Spawn(index) => {
                write_count(scratch, index);
                // The one the runtime actually starts the player on is the first, and an
                // author with three of them has to be able to see which that is.
                scratch.push_str(if index == 0 { " SPAWN *" } else { " SPAWN" });
            }
        }
        list.text_at(
            (rect.x + padding * 0.5, rect.y + padding * 0.25),
            scratch,
            if selected { theme.title } else { theme.text },
            text,
        );
    }

    // Whether there is more list than window, said in words rather than with a scroll bar.
    if rows > visible {
        scratch.clear();
        write_count(scratch, first + visible.min(rows.saturating_sub(first)));
        scratch.push_str(" OF ");
        write_count(scratch, rows);
        list.text_at(
            (panel.x + padding, panel.bottom() - padding - text * 8.0),
            scratch,
            theme.text_disabled,
            text,
        );
    }
}

/// The inspector: the selected object's transform, as numbers.
///
/// Read-only text and the keys that change it. A field an author types into needs a text
/// caret, a parser and a commit rule; the keys already exist, and what was missing was being
/// able to *see* what they did.
fn push_inspector(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    theme: &MenuTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let panel = inspector_rect(scale, viewport);
    let border = scale.px(theme.border_pixels);
    let text = scale.text(theme.label_scale);
    let padding = scale.px(layout::PADDING);
    let line = text * 10.0;

    list.rect(panel, theme.panel);
    list.outline(panel, border, theme.panel_border);
    list.text_at(
        (panel.x + padding, panel.y + padding),
        "INSPECTOR",
        theme.title,
        text,
    );

    let row = |index: usize, out: &String, list: &mut UiDrawList| {
        list.text_at(
            (
                panel.x + padding,
                panel.y + padding + scale.px(layout::HEADING) + index as f32 * line,
            ),
            out,
            theme.text,
            text,
        );
    };

    // While sculpting, the brush replaces the transform settings: they are the numbers the
    // pointer is about to use, and the grid does not apply to a brush stroke.
    if editor.is_sculpting() {
        scratch.clear();
        scratch.push_str("BRUSH ");
        scratch.push_str(editor.brush().as_str());
        row(1, scratch, list);

        scratch.clear();
        scratch.push_str("SIZE ");
        write_metres(scratch, editor.brush_radius());
        scratch.push_str(" M   STRENGTH ");
        write_metres(scratch, editor.brush_strength());
        row(2, scratch, list);

        scratch.clear();
        scratch.push_str("TAB MODE  1 2 SIZE  3 4 STRENGTH");
        row(3, scratch, list);

        scratch.clear();
        scratch.push_str("DRAG TO SHAPE THE GROUND");
        row(0, scratch, list);
        return;
    }

    // The three settings every verb reads, always on screen: an author who cannot see the
    // grid step cannot predict what an arrow key will do.
    scratch.clear();
    scratch.push_str(if editor.is_module_snap() {
        "MODULE "
    } else {
        "GRID "
    });
    if editor.grid() > 0.0 {
        write_metres(scratch, editor.grid());
    } else {
        scratch.push_str("OFF");
    }
    scratch.push_str("  ROT ");
    write_metres(scratch, editor.yaw_step());
    scratch.push_str("  ");
    scratch.push_str(if editor.is_local_space() {
        "LOCAL"
    } else {
        "WORLD"
    });
    row(4, scratch, list);

    // A body of water is described by what it is, not by a transform: an author who cannot
    // see the depth cannot tell a puddle from a lake looking down at it.
    if let Some(body) = editor
        .selected_water()
        .and_then(|index| scene.water.get(index))
    {
        scratch.clear();
        scratch.push_str("WATER");
        row(0, scratch, list);

        scratch.clear();
        scratch.push_str("SURFACE ");
        write_metres(scratch, body.center.y);
        scratch.push_str(" M   DEPTH ");
        write_metres(scratch, body.depth);
        row(1, scratch, list);

        scratch.clear();
        scratch.push_str("SIZE ");
        write_metres(scratch, body.half_extents.x * 2.0);
        scratch.push_str(" X ");
        write_metres(scratch, body.half_extents.y * 2.0);
        scratch.push_str(" M");
        row(2, scratch, list);

        scratch.clear();
        scratch.push_str("YAW ");
        write_metres(scratch, body.yaw.to_degrees());
        row(3, scratch, list);
        return;
    }

    if let Some(spawn) = editor
        .selected_spawn()
        .and_then(|index| scene.spawn_points.get(index))
    {
        scratch.clear();
        scratch.push_str(if editor.selected_spawn() == Some(0) {
            "SPAWN POINT (THE ONE USED)"
        } else {
            "SPAWN POINT"
        });
        row(0, scratch, list);

        scratch.clear();
        scratch.push_str("POS ");
        write_metres(scratch, spawn.position.x);
        scratch.push(' ');
        write_metres(scratch, spawn.position.y);
        scratch.push(' ');
        write_metres(scratch, spawn.position.z);
        row(1, scratch, list);

        scratch.clear();
        scratch.push_str("FACING ");
        write_metres(scratch, spawn.yaw.to_degrees());
        scratch.push_str(" DEG");
        row(2, scratch, list);

        scratch.clear();
        scratch.push_str("R TURNS IT   X DELETES IT");
        row(3, scratch, list);
        return;
    }

    let Some(object) = editor
        .selected_object()
        .and_then(|index| scene.objects.get(index))
    else {
        scratch.clear();
        scratch.push_str("NOTHING SELECTED");
        row(0, scratch, list);
        return;
    };

    scratch.clear();
    scratch.push_str(object.asset.name());
    if editor.selected().len() > 1 {
        scratch.push_str(" +");
        write_count(scratch, editor.selected().len() - 1);
    }
    row(0, scratch, list);

    let position = object.transform.translation;
    scratch.clear();
    scratch.push_str("POS ");
    write_metres(scratch, position.x);
    scratch.push(' ');
    write_metres(scratch, position.y);
    scratch.push(' ');
    write_metres(scratch, position.z);
    row(1, scratch, list);

    scratch.clear();
    scratch.push_str("YAW ");
    write_metres(scratch, crate::yaw_of(object.transform).to_degrees());
    row(2, scratch, list);

    scratch.clear();
    scratch.push_str("SCALE ");
    write_metres(scratch, object.transform.scale.x);
    scratch.push(' ');
    write_metres(scratch, object.transform.scale.y);
    scratch.push(' ');
    write_metres(scratch, object.transform.scale.z);
    row(3, scratch, list);
}

/// A metre value with one decimal, without allocating.
fn write_metres(out: &mut String, value: f32) {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded < 0.0 {
        out.push('-');
    }
    let magnitude = rounded.abs();
    write_count(out, magnitude as usize);
    out.push('.');
    write_count(out, ((magnitude * 10.0).round() as usize) % 10);
}

/// A whole number, without allocating.
fn write_count(out: &mut String, value: usize) {
    if value >= 10 {
        write_count(out, value / 10);
    }
    out.push(char::from(b'0' + (value % 10) as u8));
}

// ---------------------------------------------------------------------------------------
// The world overlay: what is selected, where it is, and the handles that move it.
// ---------------------------------------------------------------------------------------

/// What a selected thing is outlined in.
///
/// Cyan, and drawn over the level rather than into it: this crate has no renderer and cannot
/// tint a material, and an outline painted in the interface layer is visible whatever the
/// object's own material, whatever the light on it, and even when the object is inside
/// another one.
const SELECTION_COLOR: UiColor = [0.20, 0.95, 1.00, 0.95];

/// What everything *else* in a multiple selection is outlined in: the same colour, quieter.
///
/// The primary is what the inspector describes and what the gizmo stands on, so it is the one
/// drawn in full.
const SELECTION_SECONDARY: UiColor = [0.20, 0.95, 1.00, 0.45];

/// What the striped silhouette of an off-screen selection is drawn in.
///
/// Amber rather than cyan, because it is not the object: it is a *report* about an object
/// that is not on screen, and the two must not be mistaken for one another at a glance.
const OFFSCREEN_COLOR: UiColor = [1.00, 0.68, 0.18, 0.95];

/// How many selected things get an outline before the overlay stops drawing them.
///
/// `CTRL+A` on a finished level selects every placement in it, and a wireframe box each is
/// hundreds of quads that say nothing an author can read anyway — past a few dozen the screen
/// is a mesh of lines. The primary is always drawn, so the gizmo never stands on an object
/// with no outline under it.
const OUTLINE_LIMIT: usize = 48;

/// How thick the selection outline is, in logical pixels.
const OUTLINE_PIXELS: f32 = 2.0;

/// How thick a gizmo handle is, in logical pixels.
const HANDLE_PIXELS: f32 = 3.0;

/// How wide the striped silhouette is when nothing about the object can be projected, in
/// logical pixels. See [`push_offscreen`].
const SILHOUETTE_MIN: f32 = 26.0;

/// The largest a striped silhouette may be drawn, in logical pixels.
///
/// A selected floor slab seen from a metre away would otherwise cover the whole screen with
/// stripes, which is the one thing an editor-only marker must never do.
const SILHOUETTE_MAX: f32 = 220.0;

/// How far a silhouette is kept from the edge of the window, in logical pixels.
const SILHOUETTE_MARGIN: f32 = 16.0;

/// Draws what is selected and the handles that move it, over the level and under the panels.
///
/// Under the panels deliberately: the browser and the hierarchy are opaque, and an outline
/// painted over them would be a selection marker floating on a list.
fn push_world_overlay(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let primary = editor.selection();

    for (order, what) in editor.selected().iter().enumerate() {
        let is_primary = primary == Some(*what);
        if order >= OUTLINE_LIMIT && !is_primary {
            continue;
        }
        let Some((center, half_extents, rotation)) = pick::selection_box(scene, *what) else {
            continue;
        };
        let corners = box_corners(center, half_extents, rotation);
        let projected: Vec<(f32, f32)> = corners
            .iter()
            .filter_map(|corner| {
                engine_presentation::nameplate::project(*corner, &view_projection, viewport)
            })
            .collect();
        let bounds = bounds_of(&projected);
        let on_screen = bounds.is_some_and(|rect| overlaps_viewport(rect, viewport));

        if on_screen && projected.len() == corners.len() {
            // Every corner is in front of the eye: the box can be drawn as a box.
            push_wireframe(
                list,
                &corners,
                &view_projection,
                viewport,
                scale.px(OUTLINE_PIXELS),
                if is_primary {
                    SELECTION_COLOR
                } else {
                    SELECTION_SECONDARY
                },
            );
        } else if on_screen {
            // Partly behind the eye: the corners that project still say where it is, and a
            // rectangle around them is honest about that where a half-drawn box is not.
            if let Some(rect) = bounds {
                list.outline(
                    rect,
                    scale.px(OUTLINE_PIXELS),
                    if is_primary {
                        SELECTION_COLOR
                    } else {
                        SELECTION_SECONDARY
                    },
                );
            }
        } else if is_primary {
            // Off screen entirely. The selection is not hidden because the camera turned
            // away from it — it is reported at the edge it went out by.
            push_offscreen(
                list,
                editor,
                scene,
                *what,
                center,
                half_extents,
                bounds,
                scratch,
                viewport,
            );
        }
    }

    push_gizmo(list, editor, scene);
}

/// The eight corners of an oriented box, in world space.
fn box_corners(center: Vec3, half_extents: Vec3, rotation: Quat) -> [Vec3; 8] {
    let mut corners = [Vec3::ZERO; 8];
    for (index, corner) in corners.iter_mut().enumerate() {
        let sign = |bit: usize| if index & (1 << bit) == 0 { -1.0 } else { 1.0 };
        let local = Vec3::new(
            half_extents.x * sign(0),
            half_extents.y * sign(1),
            half_extents.z * sign(2),
        );
        *corner = center + rotation.rotate(local);
    }
    corners
}

/// The twelve edges of a box, as pairs of indices into [`box_corners`]'s output.
///
/// Two corners share an edge when their sign bits differ in exactly one place, which is what
/// this table is: the same box every time, written once.
const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (0, 2),
    (0, 4),
    (1, 3),
    (1, 5),
    (2, 3),
    (2, 6),
    (3, 7),
    (4, 5),
    (4, 6),
    (5, 7),
    (6, 7),
];

/// Draws a box as twelve lines.
fn push_wireframe(
    list: &mut UiDrawList,
    corners: &[Vec3; 8],
    view_projection: &engine_core::math::Mat4,
    viewport: (u32, u32),
    thickness: f32,
    color: UiColor,
) {
    let mut screen = [None; 8];
    for (slot, corner) in corners.iter().enumerate() {
        screen[slot] = engine_presentation::nameplate::project(*corner, view_projection, viewport);
    }
    for (from, to) in BOX_EDGES {
        let (Some(from), Some(to)) = (screen[from], screen[to]) else {
            continue;
        };
        stroke(list, from, to, thickness, color);
    }
}

/// The rectangle that contains every projected point, if there are any.
fn bounds_of(points: &[(f32, f32)]) -> Option<UiRect> {
    let (mut left, mut top) = (f32::MAX, f32::MAX);
    let (mut right, mut bottom) = (f32::MIN, f32::MIN);
    for (x, y) in points {
        left = left.min(*x);
        right = right.max(*x);
        top = top.min(*y);
        bottom = bottom.max(*y);
    }
    (!points.is_empty()).then(|| UiRect::new(left, top, right - left, bottom - top))
}

/// Whether any of `rect` is inside the window.
fn overlaps_viewport(rect: UiRect, viewport: (u32, u32)) -> bool {
    rect.right() >= 0.0
        && rect.bottom() >= 0.0
        && rect.x <= viewport.0 as f32
        && rect.y <= viewport.1 as f32
}

/// Draws the striped silhouette that stands for a selection the camera is not looking at.
///
/// # Why stripes
///
/// It has to be unmistakably *not the object*. A solid shape at the edge of the screen reads
/// as a thing standing there; a hatched one reads as a marker, and the hatch is what makes it
/// legible over a bright sky and a dark cave mouth alike. It is drawn where the object is in
/// the direction it lies, clamped inside the window, so turning the camera walks it around
/// the edge towards the object and it meets the object's real outline as it comes into view.
#[allow(
    clippy::too_many_arguments,
    reason = "one call site; splitting it would only move the arguments"
)]
fn push_offscreen(
    list: &mut UiDrawList,
    editor: &Editor,
    scene: &EnvironmentScene,
    what: Selected,
    center: Vec3,
    half_extents: Vec3,
    bounds: Option<UiRect>,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let scale = editor.scale();
    let view_projection = editor.view_projection();
    let anchor = edge_anchor(center, &view_projection, viewport);

    // How big it would be if it were on screen: the projection's own arithmetic, done by hand
    // because there is nothing to project when the object is behind the eye.
    let size = match bounds {
        Some(rect) => rect.width.max(rect.height),
        None => {
            let distance = (center - editor.eye()).length().max(0.1);
            let focal = viewport.1 as f32 * 0.5 / (crate::camera::FOV_Y * 0.5).tan();
            half_extents.max_element() * 2.0 * focal / distance
        }
    }
    .clamp(scale.px(SILHOUETTE_MIN), scale.px(SILHOUETTE_MAX));

    let margin = scale.px(SILHOUETTE_MARGIN);
    let half = size * 0.5;
    let x = anchor.0.clamp(
        margin + half,
        (viewport.0 as f32 - margin - half).max(margin + half),
    );
    let y = anchor.1.clamp(
        margin + half,
        (viewport.1 as f32 - margin - half).max(margin + half),
    );
    let rect = UiRect::new(x - half, y - half, size, size);

    stripes(list, rect, scale.px(7.0), scale.px(2.0), OFFSCREEN_COLOR);
    list.outline(rect, scale.px(OUTLINE_PIXELS), OFFSCREEN_COLOR);

    // Named, so a silhouette at the edge of the screen is "the pillar I selected" rather than
    // "something out there".
    scratch.clear();
    scratch.push_str("SELECTED ");
    match what {
        Selected::Object(index) => match scene.objects.get(index) {
            Some(object) => scratch.push_str(&object.asset.name().to_uppercase()),
            None => scratch.push_str("OBJECT"),
        },
        Selected::Water(_) => scratch.push_str("WATER"),
        Selected::Spawn(_) => scratch.push_str("SPAWN"),
    }
    list.text_at(
        (rect.x, rect.bottom() + scale.px(2.0)),
        scratch,
        OFFSCREEN_COLOR,
        scale.text(1.0),
    );
}

/// Where an off-screen point lies, as a position on the window's edge.
///
/// A point behind the eye has no projection at all, and one in front of it but outside the
/// frustum projects outside the window. Both are answered the same way: take the clip-space
/// direction, flip it when the point is behind the eye — a point behind the camera is in the
/// opposite direction on screen from what its clip coordinates say — and push it out to the
/// unit square, which is the window's edge.
fn edge_anchor(
    point: Vec3,
    view_projection: &engine_core::math::Mat4,
    viewport: (u32, u32),
) -> (f32, f32) {
    let clip = *view_projection * point.extend(1.0);
    let (x, y) = if clip.w > 1e-6 {
        (clip.x / clip.w, clip.y / clip.w)
    } else {
        (-clip.x, -clip.y)
    };
    let longest = x.abs().max(y.abs()).max(1e-6);
    let (x, y) = (x / longest, y / longest);
    (
        (x * 0.5 + 0.5) * viewport.0 as f32,
        (1.0 - (y * 0.5 + 0.5)) * viewport.1 as f32,
    )
}

/// Fills `rect` with diagonal stripes, `spacing` apart and `thickness` wide.
///
/// Clipped to the rectangle by construction rather than by a clip rectangle, because the draw
/// list has no clipping and a stripe that ran past the edge would be a diagonal line loose on
/// the screen.
fn stripes(list: &mut UiDrawList, rect: UiRect, spacing: f32, thickness: f32, color: UiColor) {
    if rect.width <= 0.0 || rect.height <= 0.0 || spacing <= 0.0 {
        return;
    }
    let mut offset = -rect.height;
    while offset < rect.width {
        // A 45-degree stripe from the top edge to the right edge, cut off at whichever of the
        // rectangle's sides it reaches first.
        let start = if offset >= 0.0 {
            (rect.x + offset, rect.y)
        } else {
            (rect.x, rect.y - offset)
        };
        let run = (rect.width - (start.0 - rect.x)).min(rect.height - (start.1 - rect.y));
        if run > 0.0 {
            stroke(
                list,
                start,
                (start.0 + run, start.1 + run),
                thickness,
                color,
            );
        }
        offset += spacing;
    }
}

/// Draws the three axis handles of the selection's gizmo.
fn push_gizmo(list: &mut UiDrawList, editor: &Editor, scene: &EnvironmentScene) {
    let Some(screen) = editor.gizmo(scene) else {
        return;
    };
    let scale = editor.scale();
    let thickness = scale.px(HANDLE_PIXELS);
    let hovered = editor.gizmo_hover();

    // A dark line under every handle, one pixel wider. That is the whole of "readable against
    // any background": a red axis over a red rock is invisible, and a red axis with a black
    // edge is not.
    for handle in screen.handles.into_iter().flatten() {
        stroke(
            list,
            handle.from,
            handle.to,
            thickness + scale.px(2.0),
            [0.0, 0.0, 0.0, 0.65],
        );
    }
    for handle in screen.handles.into_iter().flatten() {
        let active = hovered == Some(handle.axis);
        let color = if active {
            [1.0, 0.95, 0.35, 1.0]
        } else {
            handle.axis.color()
        };
        stroke(list, handle.from, handle.to, thickness, color);
        // A square at the tip: the thing to aim at, and what makes the end of the handle a
        // target rather than a place where a line stops.
        let cap = thickness * 2.4;
        let tip = UiRect::new(handle.to.0 - cap * 0.5, handle.to.1 - cap * 0.5, cap, cap);
        list.rect(tip, color);
        list.text_at(
            (handle.to.0 + cap, handle.to.1 - cap),
            handle.axis.label(),
            color,
            scale.text(1.0),
        );
    }
    // The origin, so the gizmo has a centre even when an axis points at the eye and its handle
    // is a dot.
    let pip = thickness * 2.0;
    list.rect(
        UiRect::new(
            screen.origin.0 - pip * 0.5,
            screen.origin.1 - pip * 0.5,
            pip,
            pip,
        ),
        [1.0, 1.0, 1.0, 0.9],
    );
}

/// Draws a line as a run of squares.
///
/// The draw list has rectangles and no line primitive, deliberately: it exists to describe a
/// HUD, and a HUD is rectangles. A gizmo handle is the first thing in this project that wants
/// an arbitrary direction, and a run of `thickness`-sized quads is a line at the scale these
/// are drawn at — a handle is a couple of hundred pixels, so it is a few dozen quads.
fn stroke(list: &mut UiDrawList, from: (f32, f32), to: (f32, f32), thickness: f32, color: UiColor) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    let thickness = thickness.max(1.0);
    if length < 0.5 {
        list.rect(
            UiRect::new(
                from.0 - thickness * 0.5,
                from.1 - thickness * 0.5,
                thickness,
                thickness,
            ),
            color,
        );
        return;
    }
    // An axis-aligned line is one rectangle rather than a hundred, which is most of them: two
    // of the three handles are nearly axis-aligned whenever the camera is level.
    if dy.abs() < 0.5 {
        let left = from.0.min(to.0);
        list.rect(
            UiRect::new(left, from.1 - thickness * 0.5, length, thickness),
            color,
        );
        return;
    }
    if dx.abs() < 0.5 {
        let top = from.1.min(to.1);
        list.rect(
            UiRect::new(from.0 - thickness * 0.5, top, thickness, length),
            color,
        );
        return;
    }
    let steps = (length / (thickness * 0.7)).ceil().max(1.0);
    for step in 0..=(steps as usize) {
        let t = step as f32 / steps;
        let (x, y) = (from.0 + dx * t, from.1 + dy * t);
        list.rect(
            UiRect::new(
                x - thickness * 0.5,
                y - thickness * 0.5,
                thickness,
                thickness,
            ),
            color,
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    use engine_core::input::InputState;
    use engine_core::math::Mat4;
    use engine_environment::EnvironmentObject;
    use engine_environment::asset::PILLAR;
    use engine_environment::object::default_collision;
    use engine_presentation::ui::input::UiInput;
    use engine_render_data::{Camera, Projection};

    /// A camera at `eye`, looking at `at`.
    fn camera(eye: Vec3, at: Vec3) -> Mat4 {
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        camera.set_position(eye);
        camera
            .look_at(eye, at, Vec3::new(0.0, 1.0, 0.0))
            .expect("a valid view");
        camera.view_projection()
    }

    /// An editor with one pillar at the origin selected, seen from `eye`.
    fn selected_pillar(eye: Vec3, at: Vec3) -> (Editor, EnvironmentScene) {
        let mut scene = EnvironmentScene::with_capacity("test", 1);
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        editor.select(Selected::Object(0), false);
        // One frame, so the editor is holding the matrix the overlay draws from.
        editor.update(
            &InputState::new(),
            &UiInput::new(),
            &mut scene,
            &camera(eye, at),
            VIEWPORT,
            1.0 / 60.0,
        );
        (editor, scene)
    }

    fn painted(editor: &Editor, scene: &EnvironmentScene) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            editor,
            scene,
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        list
    }

    /// How many quads of `color` the list holds.
    fn quads_of(list: &UiDrawList, color: UiColor) -> usize {
        list.commands()
            .iter()
            .filter(|command| {
                matches!(command, engine_render_data::ui::UiCommand::Rect { color: painted, .. } if *painted == color)
            })
            .count()
    }

    #[test]
    fn a_selected_object_in_view_is_outlined_and_carries_a_gizmo() {
        let (editor, scene) = selected_pillar(Vec3::new(0.0, 6.0, 18.0), Vec3::ZERO);
        let list = painted(&editor, &scene);

        for axis in crate::gizmo::Axis::ALL {
            assert!(
                quads_of(&list, axis.color()) > 0,
                "the {} handle was not drawn",
                axis.label()
            );
        }
        assert!(
            quads_of(&list, SELECTION_COLOR) > 0,
            "the selection has no outline"
        );
        assert!(
            quads_of(&list, OFFSCREEN_COLOR) == 0,
            "an object in view was reported as off screen"
        );
    }

    #[test]
    fn a_selection_the_camera_has_turned_away_from_is_drawn_as_a_striped_silhouette() {
        // Looking away from the pillar, from the same place: it is behind the eye.
        let (editor, scene) = selected_pillar(Vec3::new(0.0, 6.0, 18.0), Vec3::new(0.0, 6.0, 90.0));
        let list = painted(&editor, &scene);

        let stripes = quads_of(&list, OFFSCREEN_COLOR);
        assert!(stripes > 4, "the silhouette has {stripes} stripes in it");
        assert!(
            list.text().contains("SELECTED"),
            "the silhouette does not say what it is: {}",
            list.text()
        );
        assert_eq!(
            quads_of(&list, SELECTION_COLOR),
            0,
            "an object behind the camera was outlined as if it were in view"
        );
    }

    #[test]
    fn the_silhouette_stays_inside_the_window_and_moves_with_the_camera() {
        // Two cameras that have both turned away from the pillar, one to each side of it, so
        // the silhouette has a side of the window to be on.
        let (left, scene) = selected_pillar(Vec3::new(0.0, 6.0, 20.0), Vec3::new(60.0, 6.0, 60.0));
        let (right, _) = selected_pillar(Vec3::new(0.0, 6.0, 20.0), Vec3::new(-60.0, 6.0, 60.0));

        let first = silhouette_of(&painted(&left, &scene));
        let second = silhouette_of(&painted(&right, &scene));
        assert!(
            (first.0 - second.0).abs() > 1.0,
            "the silhouette sits in the same place whichever way the camera faces"
        );
        for (x, y) in [first, second] {
            assert!(
                x >= 0.0 && x <= VIEWPORT.0 as f32 && y >= 0.0 && y <= VIEWPORT.1 as f32,
                "the silhouette is off the window at {x}, {y}"
            );
        }
    }

    /// Where the striped silhouette was drawn: the middle of its stripes.
    fn silhouette_of(list: &UiDrawList) -> (f32, f32) {
        let mut sum = (0.0, 0.0);
        let mut count = 0.0;
        for command in list.commands() {
            if let engine_render_data::ui::UiCommand::Rect { rect, color } = command
                && *color == OFFSCREEN_COLOR
            {
                sum = (sum.0 + rect.x, sum.1 + rect.y);
                count += 1.0;
            }
        }
        assert!(count > 0.0, "nothing striped was drawn");
        (sum.0 / count, sum.1 / count)
    }

    #[test]
    fn every_browser_row_is_inside_the_panel_and_none_overlap() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let scale = editor.scale();
        let panel = browser_rect(scale, VIEWPORT);
        let visible = browser_visible_rows(scale, VIEWPORT);
        assert!(visible > 4, "the panel holds {visible} rows");

        let mut previous_bottom = panel.y;
        for slot in 0..visible {
            let rect = browser_row_rect(scale, VIEWPORT, slot);
            assert!(
                rect.x >= panel.x && rect.right() <= panel.right(),
                "{rect:?}"
            );
            assert!(
                rect.y >= previous_bottom,
                "row {slot} overlaps the one above"
            );
            previous_bottom = rect.bottom();
        }
        // The last row stops above the filter and status lines rather than running under them.
        let status_top = panel.bottom() - scale.px(layout::STATUS_HEIGHT);
        assert!(
            previous_bottom <= status_top,
            "the list runs into the status"
        );
    }

    #[test]
    fn an_open_editor_draws_its_browser_rows() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        let text = list.text();
        assert!(text.contains("ASSETS"), "the heading is missing: {text}");
        assert!(text.contains("builtin"), "the builtin folder is missing");
        assert!(
            !text.contains("ground_plain"),
            "the builtin folder starts closed, so its assets are not drawn"
        );

        // Opened, the library is on screen.
        editor
            .browser_mut()
            .toggle(crate::browser::BrowserRow::BuiltinFolder);
        list.clear();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(
            list.text().contains("ground_plain"),
            "the first library asset is not listed"
        );
    }

    #[test]
    fn a_closed_editor_draws_nothing() {
        let editor = Editor::new();
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(list.is_empty(), "a closed editor put something on screen");
    }

    #[test]
    fn an_open_editor_draws_its_panel_and_its_keys() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_editor(
            &mut list,
            &editor,
            &EnvironmentScene::with_capacity("test", 0),
            &MenuTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        assert!(!list.is_empty());
        assert!(
            list.text().contains("SAVE"),
            "the key list is what tells an author how to save"
        );
    }

    #[test]
    fn numbers_are_written_without_allocating_a_format_string() {
        let mut out = String::new();
        write_metres(&mut out, 12.34);
        assert_eq!(out, "12.3");

        out.clear();
        write_metres(&mut out, -0.05);
        assert_eq!(out, "-0.1", "it rounds rather than truncating");

        out.clear();
        write_count(&mut out, 1_405);
        assert_eq!(out, "1405");
    }
}
