//! The editor's visual language: one palette, one set of measurements, one way to measure
//! text.
//!
//! Every panel, row, button and overlay reads its colours and sizes from here, so the editor
//! looks like one tool rather than a set of screens that happened to be drawn by the same
//! program. Nothing here is the game's: the game's HUD has its own theme, tuned for reading
//! a fight, and an editor tuned for reading a level wants quieter colours and denser rows.
//!
//! # The palette
//!
//! Dark, neutral and a little cool. Panels sit a step lighter than the chrome around them and
//! the fields inside them a step darker, so depth reads without a single gradient. There is
//! one accent — a restrained blue — for what is selected, focused or active, and a warm
//! amber for anything that needs attention. Everything else is a grey.

use engine_render_data::ui::{UiColor, UiScale};

/// A colour, written the way every colour in this file is.
const fn rgba(r: f32, g: f32, b: f32, a: f32) -> UiColor {
    [r, g, b, a]
}

/// The toolbar, the status bar and the gaps between panels: the frame the tool sits in.
pub const CHROME: UiColor = rgba(0.086, 0.090, 0.100, 1.0);
/// A panel's body.
pub const PANEL: UiColor = rgba(0.125, 0.130, 0.145, 1.0);
/// A panel's header strip, and a section header inside the inspector.
pub const PANEL_HEADER: UiColor = rgba(0.150, 0.157, 0.175, 1.0);
/// Something raised off a panel: a menu, a tooltip, a dialog.
pub const RAISED: UiColor = rgba(0.170, 0.177, 0.197, 1.0);
/// The inside of a text or number field.
pub const FIELD: UiColor = rgba(0.078, 0.082, 0.092, 1.0);
/// A field's edge, and the hairline between two panels.
pub const BORDER: UiColor = rgba(0.050, 0.053, 0.060, 1.0);
/// A separator drawn *on* a panel, between groups of controls.
pub const DIVIDER: UiColor = rgba(0.215, 0.225, 0.250, 1.0);
/// Body text.
pub const TEXT: UiColor = rgba(0.860, 0.870, 0.890, 1.0);
/// A heading, and the name of what is selected.
pub const TEXT_STRONG: UiColor = rgba(0.960, 0.965, 0.975, 1.0);
/// Labels and secondary information.
pub const TEXT_DIM: UiColor = rgba(0.560, 0.585, 0.625, 1.0);
/// What cannot be used right now, and placeholders.
pub const TEXT_FAINT: UiColor = rgba(0.370, 0.390, 0.430, 1.0);
/// The one accent: selected, active, focused.
pub const ACCENT: UiColor = rgba(0.290, 0.600, 0.980, 1.0);
/// The accent as a fill behind a selected row or an active tool.
pub const ACCENT_FILL: UiColor = rgba(0.290, 0.600, 0.980, 0.22);
/// The accent as a fill behind the *primary* selected row: a step stronger.
pub const ACCENT_STRONG: UiColor = rgba(0.290, 0.600, 0.980, 0.38);
/// Under the pointer.
pub const HOVER: UiColor = rgba(1.0, 1.0, 1.0, 0.055);
/// Held down.
pub const PRESSED: UiColor = rgba(0.0, 0.0, 0.0, 0.22);
/// Needs attention: unsaved work, a warning.
pub const WARNING: UiColor = rgba(0.960, 0.690, 0.270, 1.0);
/// Something failed.
pub const DANGER: UiColor = rgba(0.930, 0.380, 0.350, 1.0);
/// Something is fine: saved.
pub const SUCCESS: UiColor = rgba(0.420, 0.800, 0.480, 1.0);
/// The backing of a chip drawn over the viewport: dark enough to read against a bright sky.
pub const OVERLAY: UiColor = rgba(0.060, 0.064, 0.072, 0.78);
/// A shadow under something raised.
pub const SHADOW: UiColor = rgba(0.0, 0.0, 0.0, 0.35);
/// What dims the editor behind a dialog.
pub const SCRIM: UiColor = rgba(0.0, 0.0, 0.0, 0.45);

/// The three axis colours, quieter than pure red, green and blue so the gizmo reads as part
/// of the tool and not as a warning.
pub const AXIS_X: UiColor = rgba(0.920, 0.330, 0.330, 1.0);
/// See [`AXIS_X`].
pub const AXIS_Y: UiColor = rgba(0.450, 0.830, 0.360, 1.0);
/// See [`AXIS_X`].
pub const AXIS_Z: UiColor = rgba(0.330, 0.560, 0.980, 1.0);
/// A handle that is hovered or being dragged.
pub const HANDLE_ACTIVE: UiColor = rgba(1.0, 0.880, 0.360, 1.0);

/// What each kind of thing is tinted, in the browser and the hierarchy.
pub mod kind {
    use super::rgba;
    use engine_render_data::ui::UiColor;

    /// A folder.
    pub const FOLDER: UiColor = rgba(0.820, 0.700, 0.450, 1.0);
    /// A model: something that can be placed.
    pub const MODEL: UiColor = rgba(0.620, 0.760, 0.930, 1.0);
    /// A material.
    pub const MATERIAL: UiColor = rgba(0.790, 0.580, 0.880, 1.0);
    /// A loose texture.
    pub const TEXTURE: UiColor = rgba(0.520, 0.800, 0.720, 1.0);
    /// A level.
    pub const SCENE: UiColor = rgba(0.950, 0.700, 0.370, 1.0);
    /// Water.
    pub const WATER: UiColor = rgba(0.360, 0.660, 0.960, 1.0);
    /// A spawn point.
    pub const SPAWN: UiColor = rgba(0.460, 0.860, 0.520, 1.0);
    /// A light.
    pub const LIGHT: UiColor = rgba(1.000, 0.820, 0.360, 1.0);
    /// Rocks and cliffs.
    pub const STONE: UiColor = rgba(0.700, 0.700, 0.720, 1.0);
    /// Trees and bushes.
    pub const PLANT: UiColor = rgba(0.520, 0.800, 0.450, 1.0);
    /// Monsters: bone against the crypt's violet.
    pub const MONSTER: UiColor = rgba(0.880, 0.520, 0.560, 1.0);
}

/// Measurements, in logical pixels. Multiplied by the interface scale where they are used.
pub mod metrics {
    /// The main toolbar along the top.
    pub const TOOLBAR_HEIGHT: f32 = 34.0;
    /// The status bar along the bottom.
    pub const STATUS_HEIGHT: f32 = 22.0;
    /// A panel's header strip.
    pub const HEADER_HEIGHT: f32 = 24.0;
    /// One row of a list: an asset, a hierarchy entry, a menu item.
    pub const ROW_HEIGHT: f32 = 20.0;
    /// A search field.
    pub const FIELD_HEIGHT: f32 = 20.0;
    /// A toolbar button.
    pub const BUTTON: f32 = 26.0;
    /// Padding inside a panel.
    pub const PADDING: f32 = 6.0;
    /// How far one level of a tree indents.
    pub const INDENT: f32 = 12.0;
    /// The space an icon takes in a row, gap included.
    pub const ICON_SLOT: f32 = 16.0;
    /// A grid tile in the content browser.
    pub const TILE: f32 = 76.0;
    /// How wide the hit zone of a panel splitter is.
    pub const SPLITTER: f32 = 6.0;
    /// The narrowest the browser may be dragged.
    pub const LEFT_MIN: f32 = 180.0;
    /// The narrowest the hierarchy and inspector column may be dragged.
    pub const RIGHT_MIN: f32 = 230.0;
    /// The width of the browser the editor opens with.
    pub const LEFT_DEFAULT: f32 = 250.0;
    /// The width of the right-hand column the editor opens with.
    pub const RIGHT_DEFAULT: f32 = 300.0;
    /// The narrowest the viewport is allowed to become.
    pub const VIEWPORT_MIN: f32 = 360.0;
    /// How long the pointer rests on a control before its tooltip appears, in seconds.
    pub const TOOLTIP_DELAY: f32 = 0.45;
}

/// The font's cell: five columns and seven rows, one column of gap between glyphs.
const GLYPH_COLUMNS: f32 = 5.0;
/// See [`GLYPH_COLUMNS`].
const GLYPH_ROWS: f32 = 7.0;
/// See [`GLYPH_COLUMNS`].
const GLYPH_ADVANCE: f32 = 6.0;

/// How the editor measures and fits text at one interface scale.
///
/// The font is a 5x7 bitmap drawn at whole-number sizes, so text is measured exactly rather
/// than estimated: a label is truncated where it actually runs out of room, and nothing ever
/// spills out of a panel onto the level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Type {
    /// Physical pixels per font cell.
    pub cell: f32,
}

impl Type {
    /// The editor's one text size at `scale`.
    ///
    /// One size, deliberately: hierarchy in this interface comes from colour, weight of the
    /// strip behind a heading and position, not from shouting.
    #[must_use]
    pub fn at(scale: UiScale) -> Self {
        Self {
            cell: scale.text(1.0),
        }
    }

    /// How tall a line of text is, in pixels.
    #[must_use]
    pub fn height(self) -> f32 {
        GLYPH_ROWS * self.cell
    }

    /// How wide `text` is, in pixels.
    #[must_use]
    pub fn width(self, text: &str) -> f32 {
        let count = text.chars().count() as f32;
        if count == 0.0 {
            return 0.0;
        }
        (count * GLYPH_ADVANCE - (GLYPH_ADVANCE - GLYPH_COLUMNS)) * self.cell
    }

    /// How many characters fit in `width` pixels.
    #[must_use]
    pub fn fits(self, width: f32) -> usize {
        ((width / self.cell + (GLYPH_ADVANCE - GLYPH_COLUMNS)) / GLYPH_ADVANCE)
            .floor()
            .max(0.0) as usize
    }

    /// `text`, cut to fit `width` with `..` at the end when it does not.
    #[must_use]
    pub fn fit<'a>(self, text: &'a str, width: f32) -> std::borrow::Cow<'a, str> {
        let room = self.fits(width);
        let count = text.chars().count();
        if count <= room {
            return std::borrow::Cow::Borrowed(text);
        }
        if room <= 2 {
            return std::borrow::Cow::Owned(text.chars().take(room).collect());
        }
        let mut cut: String = text.chars().take(room - 2).collect();
        cut.push_str("..");
        std::borrow::Cow::Owned(cut)
    }
}

/// A name as a person reads it: `rock_small` becomes `Rock Small`, and a path keeps only its
/// last part.
///
/// The font draws capitals, so the case is for anything that one day draws lowercase — and
/// for the tests, which compare what a person would type.
#[must_use]
pub fn display_name(name: &str) -> String {
    let leaf = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut out = String::with_capacity(leaf.len());
    let mut start = true;
    for character in leaf.chars() {
        if character == '_' || character == '-' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            start = true;
            continue;
        }
        if start {
            out.extend(character.to_uppercase());
            start = false;
        } else {
            out.push(character);
        }
    }
    out.trim_end().to_owned()
}

/// The factor the editor lays itself out at: the player's interface scale, or more on a tall
/// window, so a 4K monitor does not get a 1080p-sized tool in one corner of it.
#[must_use]
pub fn scale_for(settings: UiScale, viewport: (u32, u32)) -> UiScale {
    let by_height = (viewport.1 as f32 / 1080.0 * 4.0).round() / 4.0;
    UiScale::new(settings.factor().max(by_height).max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_words() {
        assert_eq!(display_name("rock_small"), "Rock Small");
        assert_eq!(
            display_name("generated/environment/caves/cave_tunnel_straight"),
            "Cave Tunnel Straight"
        );
        assert_eq!(display_name("firemage.backup"), "Firemage.backup");
    }

    #[test]
    fn text_is_measured_and_fitted_exactly() {
        let ty = Type { cell: 2.0 };
        assert_eq!(ty.width("AB"), 22.0);
        assert_eq!(ty.fits(22.0), 2);
        assert_eq!(ty.fit("ABCDEFGH", ty.width("ABCDE")), "ABC..");
        assert_eq!(ty.fit("ABC", 1000.0), "ABC");
    }

    #[test]
    fn a_tall_window_gets_a_bigger_tool() {
        let one = UiScale::new(1.0);
        assert_eq!(scale_for(one, (1920, 1080)).factor(), 1.0);
        assert_eq!(scale_for(one, (3840, 2160)).factor(), 2.0);
        assert_eq!(scale_for(one, (1280, 720)).factor(), 1.0);
    }
}
