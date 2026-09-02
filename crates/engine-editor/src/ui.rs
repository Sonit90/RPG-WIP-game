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
//!                              LMB place / select · drag move · R yaw · T size
//!                              X delete · C copy · Z undo · G grid · CTRL+S save · F6 play
//! ```
//!
//! It paints into the same [`UiDrawList`] the HUD and the menus use, so the whole interface
//! is still one batch and one draw call — see `engine_presentation::ui::paint`.

use engine_environment::EnvironmentScene;
use engine_presentation::ui::paint::MenuTheme;
use engine_render_data::ui::{UiDrawList, UiRect, UiScale};

use crate::browser::BrowserRow;
use crate::{Editor, Selected, hierarchy_len, hierarchy_row_at};

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
    (((panel.height - padding * 2.0 - heading) / row).floor().max(0.0)) as usize
}

/// Where one hierarchy row sits, counting from the top of the visible window.
#[must_use]
pub fn hierarchy_row_rect(scale: UiScale, viewport: (u32, u32), slot: usize) -> UiRect {
    let panel = hierarchy_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let row = scale.px(layout::ROW_HEIGHT);
    let gap = scale.px(layout::ROW_GAP);
    let top = panel.y + padding + scale.px(layout::HEADING) + slot as f32 * (row + gap);
    UiRect::new(
        panel.x + padding,
        top,
        panel.width - padding * 2.0,
        row,
    )
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
    let keys = [
        "LMB PLACE / SELECT   DRAG MOVE   CTRL+LMB ADD",
        "ARROWS NUDGE   SHIFT+ARROWS UP/DOWN   L LOCAL",
        "R YAW   T SIZE   0 RESET   X DELETE   C COPY",
        "G GRID   SHIFT+G ROTATION STEP   CTRL+A ALL",
        "V POUR WATER   B SCULPT THE GROUND   Z UNDO",
        "TAB NEXT ASSET",
        "F FIND   F2 RESCAN ASSETS",
        "CTRL+S SAVE   F6 BACK TO PLAYING",
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
    scratch.push_str("GRID ");
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

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;

    const VIEWPORT: (u32, u32) = (1920, 1080);

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
            assert!(rect.y >= previous_bottom, "row {slot} overlaps the one above");
            previous_bottom = rect.bottom();
        }
        // The last row stops above the filter and status lines rather than running under them.
        let status_top = panel.bottom() - scale.px(layout::STATUS_HEIGHT);
        assert!(previous_bottom <= status_top, "the list runs into the status");
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
        editor.browser_mut().toggle(crate::browser::BrowserRow::BuiltinFolder);
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
