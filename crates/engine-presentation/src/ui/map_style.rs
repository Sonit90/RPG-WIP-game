//! The dungeon map's design system: colours, type, spacing, radii, elevation and motion, in
//! one place.
//!
//! Every surface, label and control of the minimap, the full-screen map and its encounter
//! journal is drawn from these tokens. Sizes are **logical** pixels; [`MapStyle`] converts them
//! once per scale change, the same arrangement `HudMetrics` uses for the HUD, so a layout never
//! scales a number twice or forgets to.
//!
//! ```text
//! surfaces   base 0 (frame) < 1 (panels) < 2 (cards) < 3 (hover/raised)
//! text       primary > secondary > muted;  display face only for titles
//! accents    bronze (structure, selection) · ember (damage) · frost (control)
//!            teal (counterplay) · crimson (boss, danger) · verdant (cleared, healing)
//! spacing    4 · 8 · 12 · 16 · 24 · 32
//! radii      4 (chips) · 6 (buttons, icons) · 10 (cards, canvas) · 14 (the window)
//! motion     fast 120 ms (hover) · base 180 ms (content) — none with reduced motion
//! ```

use engine_render_data::font::UiFont;
use engine_render_data::ui::{UiColor, UiScale};

/// Colour tokens.
pub mod color {
    use engine_render_data::ui::UiColor;

    /// The wash over the game behind the full-screen map.
    pub const SCRIM: UiColor = [0.012, 0.014, 0.020, 0.82];
    /// The window, top of its gradient.
    pub const SURFACE_0_TOP: UiColor = [0.078, 0.086, 0.110, 1.0];
    /// The window, bottom of its gradient.
    pub const SURFACE_0_BOTTOM: UiColor = [0.050, 0.056, 0.074, 1.0];
    /// Panels inside the window: the journal, the control groups.
    pub const SURFACE_1: UiColor = [0.068, 0.076, 0.098, 1.0];
    /// Cards: a monster, an ability, a loot row.
    pub const SURFACE_2: UiColor = [0.094, 0.104, 0.132, 1.0];
    /// A card or button under the pointer.
    pub const SURFACE_3: UiColor = [0.125, 0.137, 0.172, 1.0];
    /// The ground beyond the level: the picture's own surround, so the two meet seamlessly.
    pub const MAP_GROUND: UiColor = [0.067, 0.075, 0.098, 1.0];
    /// Hairline separators and card edges.
    pub const BORDER: UiColor = [1.0, 1.0, 1.0, 0.07];
    /// A stronger edge: the window, the canvas.
    pub const BORDER_STRONG: UiColor = [0.690, 0.553, 0.341, 0.32];
    /// Drop shadows.
    pub const SHADOW: UiColor = [0.0, 0.0, 0.0, 0.55];

    /// Aged bronze: structure, selection, the progress bar.
    pub const BRONZE: UiColor = [0.690, 0.553, 0.341, 1.0];
    /// Bronze catching the light: headings, active states.
    pub const BRONZE_BRIGHT: UiColor = [0.867, 0.733, 0.502, 1.0];
    /// Bronze in shadow.
    pub const BRONZE_DIM: UiColor = [0.369, 0.294, 0.184, 1.0];

    /// Text that is read.
    pub const TEXT_PRIMARY: UiColor = [0.925, 0.906, 0.867, 1.0];
    /// Supporting text.
    pub const TEXT_SECONDARY: UiColor = [0.690, 0.678, 0.655, 1.0];
    /// Metadata and hints.
    pub const TEXT_MUTED: UiColor = [0.478, 0.475, 0.478, 1.0];
    /// The display face's titles: a warm parchment white.
    pub const TEXT_TITLE: UiColor = [0.929, 0.867, 0.741, 1.0];

    /// Damage.
    pub const EMBER: UiColor = [0.894, 0.482, 0.345, 1.0];
    /// Crowd control: slows, charges.
    pub const FROST: UiColor = [0.475, 0.710, 0.918, 1.0];
    /// Counterplay: what the party does about it.
    pub const TEAL: UiColor = [0.451, 0.812, 0.788, 1.0];
    /// The boss, danger, a fight in progress.
    pub const CRIMSON: UiColor = [0.867, 0.325, 0.290, 1.0];
    /// Cleared, healing.
    pub const VERDANT: UiColor = [0.502, 0.788, 0.549, 1.0];
    /// Interruptible casts.
    pub const AMBER: UiColor = [0.929, 0.769, 0.380, 1.0];
    /// Arcane damage.
    pub const ARCANE: UiColor = [0.718, 0.584, 0.945, 1.0];

    /// The same colour at another opacity.
    #[must_use]
    pub fn alpha(color: UiColor, alpha: f32) -> UiColor {
        [color[0], color[1], color[2], color[3] * alpha]
    }
}

/// A run of text's treatment: face, size in logical pixels, and line height as a multiple.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// The face.
    pub font: UiFont,
    /// Pixels per em, logical.
    pub size: f32,
    /// Baseline to baseline, as a multiple of the size.
    pub line: f32,
}

/// The type scale. Nothing on the map is set smaller than [`type_scale::LABEL`].
pub mod type_scale {
    use super::TextStyle;
    use engine_render_data::font::UiFont;

    /// The dungeon's title.
    pub const TITLE: TextStyle = TextStyle {
        font: UiFont::Display,
        size: 28.0,
        line: 1.2,
    };
    /// A selected encounter's title in the journal.
    pub const ENCOUNTER: TextStyle = TextStyle {
        font: UiFont::Display,
        size: 22.0,
        line: 1.25,
    };
    /// A monster or boss name.
    pub const NAME: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 17.0,
        line: 1.3,
    };
    /// Section headings and card titles.
    pub const HEADING: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 15.0,
        line: 1.35,
    };
    /// An ability's name.
    pub const ABILITY: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 15.0,
        line: 1.3,
    };
    /// Descriptions and explanations.
    pub const BODY: TextStyle = TextStyle {
        font: UiFont::Regular,
        size: 14.0,
        line: 1.5,
    };
    /// Secondary labels: buttons, chips, the help strip.
    pub const UI: TextStyle = TextStyle {
        font: UiFont::Medium,
        size: 13.0,
        line: 1.4,
    };
    /// Metadata: cast, cooldown, range, level.
    pub const META: TextStyle = TextStyle {
        font: UiFont::Medium,
        size: 12.5,
        line: 1.4,
    };
    /// Small section labels, the only uppercase text.
    pub const LABEL: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 11.5,
        line: 1.4,
    };
    /// A marker's pack count.
    pub const BADGE: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 11.5,
        line: 1.2,
    };
}

/// The spacing scale, logical pixels.
pub mod space {
    /// Hairline gap.
    pub const XS: f32 = 4.0;
    /// Tight.
    pub const SM: f32 = 8.0;
    /// Between related items.
    pub const MD: f32 = 12.0;
    /// Inside a card.
    pub const LG: f32 = 16.0;
    /// Between groups; panel padding.
    pub const XL: f32 = 24.0;
    /// Around the window.
    pub const XXL: f32 = 32.0;
}

/// Corner radii, logical pixels.
pub mod radius {
    /// Chips and badges.
    pub const CHIP: f32 = 4.0;
    /// Buttons and icon frames.
    pub const CONTROL: f32 = 6.0;
    /// Cards and the canvas.
    pub const CARD: f32 = 10.0;
    /// The window.
    pub const WINDOW: f32 = 14.0;
}

/// Motion, in seconds.
pub mod motion {
    /// Hover feedback.
    pub const FAST: f32 = 0.12;
    /// Content changing: a new selection, a card opening.
    pub const BASE: f32 = 0.18;
    /// How long the pointer rests before a tooltip appears.
    pub const TOOLTIP_DELAY: f32 = 0.35;
}

/// The tokens at one interface scale: what layout and drawing actually use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapStyle {
    /// The scale these were derived with.
    pub scale: UiScale,
}

impl MapStyle {
    /// The tokens at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self { scale }
    }

    /// A logical length in physical pixels, rounded to a whole pixel.
    #[must_use]
    pub fn px(&self, logical: f32) -> f32 {
        self.scale.px(logical)
    }

    /// A text style's size in physical pixels. Not rounded: the fonts are distance fields and
    /// draw cleanly at any size.
    #[must_use]
    pub fn size(&self, style: TextStyle) -> f32 {
        style.size * self.scale.factor()
    }

    /// A text style's line height in physical pixels.
    #[must_use]
    pub fn line(&self, style: TextStyle) -> f32 {
        (style.size * style.line * self.scale.factor()).round()
    }

    /// How wide `text` is in `style`, physical pixels.
    #[must_use]
    pub fn width(&self, style: TextStyle, text: &str) -> f32 {
        style.font.width(text, self.size(style))
    }

    /// `text` wrapped to `width` physical pixels in `style`.
    #[must_use]
    pub fn wrap(&self, style: TextStyle, text: &str, width: f32) -> Vec<String> {
        style.font.wrap(text, self.size(style), width)
    }

    /// `text` cut with an ellipsis to fit `width` physical pixels in `style`.
    #[must_use]
    pub fn fit(&self, style: TextStyle, text: &str, width: f32) -> String {
        style.font.fit(text, self.size(style), width)
    }

    /// The top of a line box that centres `style`'s capitals on `center_y`.
    #[must_use]
    pub fn centred_top(&self, style: TextStyle, center_y: f32) -> f32 {
        let size = self.size(style);
        // Cap height is about 0.72 em in both faces; centring the caps, not the line box, is
        // what makes a label sit in the middle of its button.
        center_y - style.font.ascent(size) + size * 0.36
    }
}

/// `text` in title case: "GATE WARDENS" becomes "Gate Wardens", "MARK OF THE HOST" becomes
/// "Mark of the Host". Names in the game's tables are stored upper-case for the bitmap font;
/// this is how they are shown in the proportional one.
#[must_use]
pub fn title_case(text: &str) -> String {
    const SMALL: [&str; 9] = ["of", "the", "a", "an", "and", "in", "on", "to", "for"];
    let mut out = String::with_capacity(text.len());
    let mut first_word = true;
    for (index, word) in text.split(' ').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let lower = word.to_lowercase();
        if !first_word && SMALL.contains(&lower.as_str()) {
            out.push_str(&lower);
        } else {
            // Capitalize after the start and after a hyphen or an apostrophe-free boundary.
            let mut capitalize = true;
            for character in lower.chars() {
                if capitalize && character.is_alphabetic() {
                    out.extend(character.to_uppercase());
                    capitalize = false;
                } else {
                    out.push(character);
                }
                if character == '-' || character == '(' {
                    capitalize = true;
                }
            }
        }
        // A dash or a colon starts a new title: "Phase 2 - The Ash".
        if !word.is_empty() {
            first_word = word == "-" || word.ends_with(':');
        }
    }
    out
}

/// A colour for an ability's category accent.
#[must_use]
pub fn blend(a: UiColor, b: UiColor, t: f32) -> UiColor {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_shown_in_title_case_with_small_words_lowered() {
        assert_eq!(title_case("GATE WARDENS"), "Gate Wardens");
        assert_eq!(title_case("MARK OF THE HOST"), "Mark of the Host");
        assert_eq!(title_case("THE HOLLOW STEWARD"), "The Hollow Steward");
        assert_eq!(title_case("PHASE 2 - THE ASH"), "Phase 2 - The Ash");
        assert_eq!(title_case("MULTI-SHOT"), "Multi-Shot");
    }

    #[test]
    fn nothing_is_set_below_the_label_size_and_sizes_scale() {
        for style in [
            type_scale::TITLE,
            type_scale::ENCOUNTER,
            type_scale::NAME,
            type_scale::HEADING,
            type_scale::ABILITY,
            type_scale::BODY,
            type_scale::UI,
            type_scale::META,
            type_scale::BADGE,
        ] {
            assert!(style.size >= type_scale::LABEL.size, "{style:?}");
        }
        let large = MapStyle::new(UiScale::new(2.0));
        assert!((large.size(type_scale::BODY) - 28.0).abs() < 1e-4);
    }
}
