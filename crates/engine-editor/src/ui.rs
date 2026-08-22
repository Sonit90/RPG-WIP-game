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
use engine_presentation::ui::paint::{MenuTheme, push_widget};
use engine_presentation::ui::screen::UiScreen;
use engine_render_data::ui::{UiDrawList, UiRect, UiScale};

use crate::Editor;

/// Layout constants, in logical pixels.
mod layout {
    /// Width of the palette panel.
    pub const PANEL_WIDTH: f32 = 168.0;
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
    /// Height reserved under the palette for the status lines.
    pub const STATUS_HEIGHT: f32 = 46.0;
}

/// Where the palette panel sits, in physical pixels.
#[must_use]
pub fn panel_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    let rows = engine_environment::asset::LIBRARY.len() as f32;
    let height = scale.px(layout::PADDING * 2.0
        + layout::HEADING
        + rows * layout::ROW_HEIGHT
        + (rows - 1.0) * layout::ROW_GAP
        + layout::STATUS_HEIGHT);
    let width = scale.px(layout::PANEL_WIDTH);
    let margin = scale.px(layout::MARGIN);
    // Clamped so the palette is reachable on a short window rather than hanging off the
    // bottom of it — the same rule the HUD's own panels follow.
    let height = height.min((viewport.1 as f32 - margin * 2.0).max(scale.px(120.0)));
    UiRect::new(margin, margin, width, height)
}

/// Puts every palette button where it belongs for this scale and viewport.
pub fn layout_palette(palette: &mut UiScreen<usize>, scale: UiScale, viewport: (u32, u32)) {
    let panel = panel_rect(scale, viewport);
    let padding = scale.px(layout::PADDING);
    let row = scale.px(layout::ROW_HEIGHT);
    let gap = scale.px(layout::ROW_GAP);
    let top = panel.y + padding + scale.px(layout::HEADING);
    let width = panel.width - padding * 2.0;

    for index in 0..engine_environment::asset::LIBRARY.len() {
        let y = top + index as f32 * (row + gap);
        palette.set_rect(index, UiRect::new(panel.x + padding, y, width, row));
    }
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
    list.text_at(
        (panel.x + padding, panel.y + padding),
        "PLACE",
        theme.title,
        text,
    );

    for (index, widget) in editor.palette().widgets() {
        push_widget(list, widget, theme, scale);
        // The chosen asset is marked rather than merely hovered, because the palette has
        // sixteen entries and "which one will the next click place" must be readable while
        // the pointer is out in the world.
        if index == asset_index(editor) {
            list.outline(widget.rect(), border, theme.title);
        }
    }

    // Under the palette: what just happened, and what is selected.
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
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5),
        editor.status(),
        theme.text,
        text,
    );

    scratch.clear();
    match editor
        .selection()
        .and_then(|index| scene.objects.get(index))
    {
        Some(object) => {
            scratch.push_str(object.asset.name());
            scratch.push_str(" @ ");
            write_metres(scratch, object.position().x);
            scratch.push(' ');
            write_metres(scratch, object.position().z);
        }
        None => scratch.push_str("nothing selected"),
    }
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5 + text * 10.0),
        scratch,
        theme.text_disabled,
        text,
    );

    // The object count, so an author can see a scene growing past what they meant to build.
    scratch.clear();
    scratch.push_str("objects ");
    write_count(scratch, scene.len());
    list.text_at(
        (panel.x + padding, status_top + padding * 0.5 + text * 20.0),
        scratch,
        theme.text_disabled,
        text,
    );

    // The key list, bottom right, where a HUD is not.
    let keys = [
        "LMB PLACE / SELECT   DRAG MOVE",
        "R YAW   T SIZE   X DELETE   C COPY",
        "Z UNDO   G GRID   TAB NEXT ASSET",
        "CTRL+S SAVE   F6 BACK TO PLAYING",
    ];
    let line = text * 12.0;
    let bottom = viewport.1 as f32 - scale.px(layout::MARGIN);
    for (index, key) in keys.iter().enumerate() {
        let y = bottom - line * (keys.len() - index) as f32;
        list.text_aligned(
            (viewport.0 as f32 - scale.px(layout::MARGIN), y),
            key,
            theme.text_disabled,
            text,
            engine_render_data::ui::TextAlign::Right,
        );
    }
}

/// Which palette row the editor's asset is.
fn asset_index(editor: &Editor) -> usize {
    engine_environment::asset::LIBRARY
        .iter()
        .position(|asset| *asset == editor.asset())
        .unwrap_or(0)
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
    fn every_palette_row_is_inside_the_panel_and_none_overlap() {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test", VIEWPORT);
        let panel = panel_rect(editor.scale(), VIEWPORT);

        let mut previous_bottom = panel.y;
        for index in 0..engine_environment::asset::LIBRARY.len() {
            let Some(widget) = editor.palette().widget(index) else {
                panic!("row {index} exists");
            };
            let rect = widget.rect();
            assert!(
                rect.x >= panel.x && rect.right() <= panel.right(),
                "{rect:?}"
            );
            assert!(
                rect.y >= previous_bottom,
                "row {index} overlaps the one above"
            );
            previous_bottom = rect.bottom();
        }
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
