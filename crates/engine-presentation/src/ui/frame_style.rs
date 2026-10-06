//! The look every unit frame, the raid frames and the cast bar share with the nameplates.
//!
//! ```text
//! frame_panel   rounded near-black panel, thin dark border
//! frame_bar     dark ground, gradient fill, dark border — the nameplate's health bar
//! frame_text    Inter SemiBold with a dark edge, capitals centred on a line
//! ```
//!
//! One set of primitives, so the frames on the screen and the plates in the world are one
//! visual language: a player who has learned to read a nameplate's bar has learned to read
//! every bar on their screen. Colours come from [`PlateTheme`](crate::game_ui::PlateTheme).

use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect};

use crate::game_ui::{PlateTheme, cap_centred_top, faded, plate_text_style};

/// The panel a frame sits on: rounded, near-black, bordered.
pub fn frame_panel(list: &mut UiDrawList, rect: UiRect, theme: &PlateTheme, factor: f32) {
    let radius = (4.0 * factor).round().max(2.0);
    list.panel(rect, radius, theme.frame_panel, theme.frame_panel);
    list.panel_stroke(rect, radius, factor.max(1.0).round(), theme.frame_border);
}

/// A bar in the nameplate style: dark ground, a two-colour fill `fraction` of the way across,
/// and a border drawn over both.
pub fn frame_bar(
    list: &mut UiDrawList,
    rect: UiRect,
    fraction: f32,
    fill: (UiColor, UiColor),
    theme: &PlateTheme,
) {
    frame_health_bar(list, rect, HealthLayers::health(fraction), fill, theme);
}

/// What a health bar shows, as shares of maximum health.
///
/// ```text
/// [ health ████████████ ][ absorb ░░░░ ][ missing     ]
///                                             or, when the shield outgrows the gap,
/// [ health ████████████████████ ][░░ absorb ░░]|      <- the bright overflow edge
/// ```
///
/// One description every health bar is drawn from — the player frame, the target and its
/// target, party rows, raid cells and nameplates — so a shield looks the same on all of them
/// and a future source of absorb (or temporary health) is one more number here, not a change
/// to each bar.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HealthLayers {
    /// Current health over maximum, in `0..=1`.
    pub health: f32,
    /// Absorb over maximum. May exceed what is missing; never negative.
    pub absorb: f32,
}

impl HealthLayers {
    /// Health with no shield.
    #[must_use]
    pub fn health(fraction: f32) -> Self {
        Self {
            health: finite_fraction(fraction),
            absorb: 0.0,
        }
    }

    /// From the raw numbers. A zero maximum is an empty bar, not a division by zero.
    #[must_use]
    pub fn of(current: f32, maximum: f32, absorb: f32) -> Self {
        if !(maximum.is_finite() && maximum > 0.0) {
            return Self::default();
        }
        Self {
            health: finite_fraction(current / maximum),
            absorb: if absorb.is_finite() {
                (absorb / maximum).max(0.0)
            } else {
                0.0
            },
        }
    }

    /// Where the absorb layer sits on a bar `width` wide, starting at zero: `(start, width)`.
    ///
    /// After the health while it fits; once health and shield together pass the end, the
    /// shield is drawn back from the end instead, over the health, so its full size stays
    /// visible. `None` when there is no shield.
    #[must_use]
    pub fn absorb_span(&self, width: f32) -> Option<(f32, f32, bool)> {
        if self.absorb <= 0.0 || width <= 0.0 {
            return None;
        }
        let shield = (self.absorb.min(1.0) * width).max(1.0);
        let start = self.health * width;
        if start + shield <= width + 0.5 {
            Some((start, shield, false))
        } else {
            Some(((width - shield).max(0.0), shield, true))
        }
    }
}

fn finite_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// [`frame_bar`] with every layer of [`HealthLayers`]: the fill, then the absorb in pale
/// silver, then the bright edge when the shield has run off the end, then the border.
pub fn frame_health_bar(
    list: &mut UiDrawList,
    rect: UiRect,
    layers: HealthLayers,
    fill: (UiColor, UiColor),
    theme: &PlateTheme,
) {
    let border = 1.0;
    let inner = rect.inset(border);
    list.rect(inner, theme.bar_background);
    if layers.health > 0.0 {
        let width = (inner.width * layers.health).round().max(1.0);
        list.panel(
            UiRect::new(inner.x, inner.y, width, inner.height),
            0.0,
            fill.0,
            fill.1,
        );
    }
    push_absorb_layer(list, inner, layers, theme, 1.0);
    list.outline(rect, border, theme.border);
}

/// Draws only the absorb layer of `layers` inside `inner`, at `alpha`.
///
/// For a bar that draws its own fill — the raid cell, the nameplate — and wants the shield in
/// the same place and the same colours as everywhere else.
pub fn push_absorb_layer(
    list: &mut UiDrawList,
    inner: UiRect,
    layers: HealthLayers,
    theme: &PlateTheme,
    alpha: f32,
) {
    let Some((start, width, overflow)) = layers.absorb_span(inner.width) else {
        return;
    };
    let shield = UiRect::new(
        (inner.x + start).round(),
        inner.y,
        width.round().max(1.0),
        inner.height,
    );
    // A translucent sheet first, so the health under an overflowing shield still shows.
    list.panel(
        shield,
        0.0,
        faded(theme.absorb_top, 0.80 * alpha),
        faded(theme.absorb_bottom, 0.80 * alpha),
    );
    // **The shield's texture**: bright diagonal hatching across the sheet, the genre's mark
    // for "this health is not yours yet". Drawn as a staircase of short runs so it needs no
    // texture and no clip — the bar may already be inside one.
    push_hatching(list, shield, faded([1.0, 1.0, 1.0, 0.28], alpha));
    // A highlight along its top and a shade along its bottom, so the layer reads as a sheet
    // of glass laid over the bar rather than a second colour of health.
    list.rect(
        UiRect::new(shield.x, shield.y, shield.width, 1.0),
        faded(theme.absorb_edge, 0.85 * alpha),
    );
    list.rect(
        UiRect::new(shield.x, shield.bottom() - 1.0, shield.width, 1.0),
        faded([0.0, 0.0, 0.0, 0.35], alpha),
    );
    // Where the shield starts, a bright seam: the boundary between health and shield.
    list.rect(
        UiRect::new(shield.x, shield.y, 1.0, shield.height),
        faded(theme.absorb_edge, 0.9 * alpha),
    );
    if overflow {
        // More shield than missing health: a glowing edge at the end says "and then some".
        let edge = (inner.height * 0.18).clamp(2.0, 4.0).round();
        let end = UiRect::new(inner.right() - edge, inner.y, edge, inner.height);
        list.glow(end, 0.0, edge * 2.0, faded(theme.absorb_edge, 0.55 * alpha));
        list.rect(end, faded(theme.absorb_edge, alpha));
    }
}

/// Diagonal hatching inside `rect`: stripes rising to the right, every few pixels.
///
/// Each stripe is a stack of short horizontal runs, one per band of rows, stepped across as
/// it climbs — a 45° line at the bars' sizes for a handful of rectangles, and every run is
/// cut to `rect` so nothing spills over the border.
fn push_hatching(list: &mut UiDrawList, rect: UiRect, color: UiColor) {
    if rect.width < 2.0 || rect.height < 2.0 {
        return;
    }
    let pitch = (rect.height * 0.55).clamp(5.0, 10.0).round();
    let thickness = (pitch * 0.4).round().max(1.0);
    let band = (rect.height / 6.0).round().clamp(1.0, 3.0);
    let bands = (rect.height / band).ceil() as usize;
    // Stripes start far enough left that the ones that climb in from outside cover the
    // left-hand corner too.
    let first = -(rect.height / pitch).ceil() as i32 - 1;
    let last = (rect.width / pitch).ceil() as i32 + 1;
    for stripe in first..=last {
        for index in 0..bands {
            let top = rect.y + index as f32 * band;
            let height = band.min(rect.bottom() - top);
            // Rising to the right: the lower the band, the further left the run.
            let rise = rect.bottom() - (top + height * 0.5);
            let left = rect.x + stripe as f32 * pitch + rise;
            let x0 = left.max(rect.x);
            let x1 = (left + thickness).min(rect.right());
            if x1 - x0 >= 0.5 && height > 0.0 {
                list.rect(UiRect::new(x0, top, x1 - x0, height), color);
            }
        }
    }
}

/// A flat colour as a bar fill: itself at the top, a darker shade at the bottom.
///
/// The frames' resource bars have one colour each in the theme; this gives them the same
/// slight depth the health bars have without a second colour per resource.
#[must_use]
pub fn shaded(color: UiColor) -> (UiColor, UiColor) {
    let dark = [color[0] * 0.68, color[1] * 0.68, color[2] * 0.68, color[3]];
    (color, dark)
}

/// A friendly unit's health fill at `fraction`: green, turning orange when it is low.
#[must_use]
pub fn friendly_fill(theme: &PlateTheme, fraction: f32) -> (UiColor, UiColor) {
    if fraction <= theme.low_health.max(0.25) {
        (theme.friendly_low_top, theme.friendly_low_bottom)
    } else {
        (theme.friendly_top, theme.friendly_bottom)
    }
}

/// A hostile unit's health fill at `fraction`: the nameplate's red, orange-red when low.
#[must_use]
pub fn hostile_fill(theme: &PlateTheme, fraction: f32) -> (UiColor, UiColor) {
    if fraction <= theme.low_health {
        (theme.health_low_top, theme.health_low_bottom)
    } else {
        (theme.health_top, theme.health_bottom)
    }
}

/// A line of text in the frames' face, its capitals centred on `center_y`.
#[allow(
    clippy::too_many_arguments,
    reason = "a styled line is this many independent facts"
)]
pub fn frame_text(
    list: &mut UiDrawList,
    x: f32,
    center_y: f32,
    text: &str,
    color: UiColor,
    size: f32,
    align: TextAlign,
    theme: &PlateTheme,
) {
    frame_text_faded(list, x, center_y, text, color, size, align, theme, 1.0);
}

/// [`frame_text`] at an opacity, edge and all.
#[allow(
    clippy::too_many_arguments,
    reason = "a styled line is this many independent facts"
)]
pub fn frame_text_faded(
    list: &mut UiDrawList,
    x: f32,
    center_y: f32,
    text: &str,
    color: UiColor,
    size: f32,
    align: TextAlign,
    theme: &PlateTheme,
    alpha: f32,
) {
    if text.is_empty() || size.is_nan() || size <= 0.0 {
        return;
    }
    list.styled_label(
        (x, cap_centred_top(UiFont::SemiBold, size, center_y)),
        text,
        faded(color, alpha),
        UiFont::SemiBold,
        size,
        align,
        plate_text_style(faded(theme.text_outline, alpha), size, 0.0),
    );
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::ui::UiCommand;

    #[test]
    fn a_bar_is_ground_fill_and_border_and_its_fill_follows_the_fraction() {
        let theme = PlateTheme::default();
        let mut list = UiDrawList::new();
        let rect = UiRect::new(10.0, 10.0, 102.0, 16.0);
        frame_bar(&mut list, rect, 0.5, friendly_fill(&theme, 0.5), &theme);
        let fill = list
            .commands()
            .iter()
            .find_map(|command| match command {
                UiCommand::Panel { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a fill");
        assert_eq!(fill.width, 50.0);
        assert!(
            list.commands()
                .iter()
                .any(|command| matches!(command, UiCommand::Outline { .. }))
        );
    }

    #[test]
    fn low_health_changes_colour_for_friend_and_foe() {
        let theme = PlateTheme::default();
        assert_ne!(friendly_fill(&theme, 1.0), friendly_fill(&theme, 0.1));
        assert_ne!(hostile_fill(&theme, 1.0), hostile_fill(&theme, 0.1));
    }
}
