//! Test support: draws a screen and checks its layout is sound at the sizes people play at.
//!
//! "Sound" is two rules. Everything a screen draws lies inside the window, and no two of its
//! top-level panels overlap. A top-level panel is a surface that casts a shadow
//! ([`UiDrawList::shadow`]: [`super::kit::panel`] and the front-end's pages both do), so a card
//! or a button inside a panel is never mistaken for a second panel.

use engine_render_data::ui::{UiCommand, UiDrawList, UiRect, UiScale};

/// The window sizes and interface scales every screen is audited at.
pub(crate) const SIZES: [((u32, u32), f32); 9] = [
    ((1280, 720), 1.0),
    ((1280, 720), 1.5),
    ((1280, 720), 2.0),
    ((1920, 1080), 1.0),
    ((1920, 1080), 1.5),
    ((1920, 1080), 2.0),
    ((2560, 1440), 1.0),
    ((2560, 1440), 1.5),
    ((2560, 1440), 2.0),
];

/// [`SIZES`] with the scale as the type the layouts take.
pub(crate) fn sizes() -> impl Iterator<Item = ((u32, u32), UiScale)> {
    SIZES
        .into_iter()
        .map(|(viewport, scale)| (viewport, UiScale::new(scale)))
}

/// The rectangles of every shadowed surface in `list`.
pub(crate) fn panels(list: &UiDrawList) -> Vec<UiRect> {
    list.commands()
        .iter()
        .filter_map(|command| match command {
            UiCommand::Panel {
                rect,
                hollow: true,
                feather,
                stroke,
                ..
            } if *feather > 0.0 && *stroke == 0.0 => Some(*rect),
            _ => None,
        })
        .collect()
}

/// Whether `a` and `b` share more than a sliver (so panels that merely touch are fine).
pub(crate) fn overlap(a: UiRect, b: UiRect) -> bool {
    let width = a.right().min(b.right()) - a.x.max(b.x);
    let height = a.bottom().min(b.bottom()) - a.y.max(b.y);
    width > 1.5 && height > 1.5
}

/// Whether `rect` lies inside a `viewport` window, give or take a rounding pixel.
pub(crate) fn on_screen(rect: UiRect, viewport: (u32, u32)) -> bool {
    rect.x >= -1.0
        && rect.y >= -1.0
        && rect.right() <= viewport.0 as f32 + 1.0
        && rect.bottom() <= viewport.1 as f32 + 1.0
}

/// Panics, naming `what`, unless everything drawn is on screen and no two panels overlap.
pub(crate) fn assert_sound(list: &UiDrawList, viewport: (u32, u32), what: &str) {
    let panels = panels(list);
    for (index, panel) in panels.iter().enumerate() {
        assert!(
            on_screen(*panel, viewport),
            "{what}: panel {index} {panel:?} leaves the {viewport:?} window"
        );
        for (other_index, other) in panels.iter().enumerate().skip(index + 1) {
            assert!(
                !overlap(*panel, *other),
                "{what}: panels {index} {panel:?} and {other_index} {other:?} overlap"
            );
        }
    }
    for command in list.commands() {
        match command {
            UiCommand::Panel {
                rect,
                feather,
                hollow,
                ..
            } if *feather == 0.0 && !*hollow => {
                assert!(
                    on_screen(*rect, viewport),
                    "{what}: a surface {rect:?} leaves the {viewport:?} window"
                );
            }
            UiCommand::Icon { rect, .. } => {
                assert!(
                    on_screen(*rect, viewport),
                    "{what}: an icon {rect:?} leaves the {viewport:?} window"
                );
            }
            UiCommand::Label { origin, .. } => {
                assert!(
                    origin.0 >= -1.0
                        && origin.1 >= -1.0
                        && origin.0 <= viewport.0 as f32 + 1.0
                        && origin.1 <= viewport.1 as f32 + 1.0,
                    "{what}: text at {origin:?} starts outside the {viewport:?} window"
                );
            }
            _ => {}
        }
    }
}
