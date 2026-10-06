//! The interface kit: the controls and surfaces every HUD screen is built from.
//!
//! ```text
//! panel       floating translucent surface: soft shadow, gradient, hairline, top highlight
//! button      primary · secondary · ghost · danger; hover, pressed, active, disabled
//! icon_button the same, square, an icon instead of a label
//! section     a small capitalised heading with a rule after it
//! stepper     Label ............ [ − ]  1.00  [ + ]
//! segmented   [ ← Left ][ Right → ]       one of a few, the chosen one raised
//! switch      a pill with a knob
//! anchor_grid 3 × 3 arrows around a dot: which corner an element hangs from
//! chip        a small rounded tag: an element's name over it in Edit HUD
//! ```
//!
//! Drawn from the same tokens as the dungeon map ([`super::map_style`]): one palette, one type
//! scale, one spacing scale, one set of radii. The tooltip, the frames and the editor are then
//! one design language rather than three — and a new screen is a composition of these, not a
//! new set of rectangles.
//!
//! Immediate mode: a control is drawn from its rectangle and an [`Interaction`] that says what
//! the pointer is doing to it. Whoever owns the screen decides what a press means; nothing
//! here keeps state, so nothing here can disagree with the thing it draws.

use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use super::input::UiInput;
use super::map_style::{MapStyle, TextStyle, color, radius, type_scale};

/// The kit's own accents and text styles, on top of the map's palette.
pub mod tokens {
    use engine_render_data::font::UiFont;
    use engine_render_data::ui::UiColor;

    use super::TextStyle;

    /// The selection and primary-action accent: the map's bronze catching the light.
    pub const ACCENT: UiColor = super::color::BRONZE_BRIGHT;
    /// The accent at rest.
    pub const ACCENT_DIM: UiColor = super::color::BRONZE;
    /// Guides and snapping: a cool blue that cannot be mistaken for a selection.
    pub const GUIDE: UiColor = [0.45, 0.72, 1.0, 1.0];
    /// Text on the accent.
    pub const ON_ACCENT: UiColor = [0.10, 0.08, 0.05, 1.0];
    /// A floating panel's surface, top of its gradient. Translucent: the world shows through.
    pub const GLASS_TOP: UiColor = [0.085, 0.094, 0.120, 0.93];
    /// A floating panel's surface, bottom of its gradient.
    pub const GLASS_BOTTOM: UiColor = [0.055, 0.061, 0.080, 0.93];
    /// A control's resting fill.
    pub const CONTROL: UiColor = [0.13, 0.145, 0.18, 0.92];
    /// A control under the pointer.
    pub const CONTROL_HOVER: UiColor = [0.18, 0.195, 0.24, 0.96];
    /// A control being pressed.
    pub const CONTROL_PRESSED: UiColor = [0.09, 0.10, 0.125, 0.96];
    /// A sunken track: the stepper's value well, the segmented control's channel.
    pub const WELL: UiColor = [0.035, 0.040, 0.052, 0.85];

    /// A tooltip's title.
    pub const TOOLTIP_TITLE: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 20.0,
        line: 1.3,
    };
    /// A tooltip's body: comfortably larger than anything the HUD prints in passing.
    pub const TOOLTIP_BODY: TextStyle = TextStyle {
        font: UiFont::Regular,
        size: 15.5,
        line: 1.5,
    };
    /// A tooltip's metadata: cast time, cooldown, range.
    pub const TOOLTIP_META: TextStyle = TextStyle {
        font: UiFont::Medium,
        size: 14.0,
        line: 1.45,
    };
    /// A panel's own title.
    pub const PANEL_TITLE: TextStyle = TextStyle {
        font: UiFont::SemiBold,
        size: 16.0,
        line: 1.3,
    };
}

/// The tokens at one interface scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kit {
    /// The map's tokens, which the kit draws with.
    pub style: MapStyle,
}

impl Kit {
    /// The kit at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            style: MapStyle::new(scale),
        }
    }

    /// A logical length in physical pixels.
    #[must_use]
    pub fn px(&self, logical: f32) -> f32 {
        self.style.px(logical)
    }

    /// A line of text, its capitals centred on `center_y`.
    #[allow(
        clippy::too_many_arguments,
        reason = "a line of text is this many independent facts"
    )]
    pub fn text(
        &self,
        list: &mut UiDrawList,
        x: f32,
        center_y: f32,
        text: &str,
        style: TextStyle,
        color: UiColor,
        align: TextAlign,
    ) {
        if text.is_empty() {
            return;
        }
        list.label(
            (x, self.style.centred_top(style, center_y)),
            text,
            color,
            style.font,
            self.style.size(style),
            align,
        );
    }

    /// How wide `text` is in `style`.
    #[must_use]
    pub fn width(&self, style: TextStyle, text: &str) -> f32 {
        self.style.width(style, text)
    }
}

/// What the pointer is doing to a control, and what state the control is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interaction {
    /// The pointer is over it.
    pub hovered: bool,
    /// The button is held down over it.
    pub pressed: bool,
    /// It is on: the selected segment, a toggled button.
    pub active: bool,
    /// It does nothing and says so.
    pub disabled: bool,
}

impl Interaction {
    /// The pointer's relation to `rect` this frame.
    #[must_use]
    pub fn at(rect: UiRect, input: &UiInput) -> Self {
        let hovered = rect.contains(input.pointer.0, input.pointer.1);
        Self {
            hovered,
            pressed: hovered && input.pointer_down,
            active: false,
            disabled: false,
        }
    }

    /// The same, on or off.
    #[must_use]
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// The same, usable or not. A disabled control is neither hovered nor pressed.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        if disabled {
            self.hovered = false;
            self.pressed = false;
        }
        self
    }

    /// Whether `rect` was clicked this frame: pressed inside it.
    #[must_use]
    pub fn clicked(rect: UiRect, input: &UiInput) -> bool {
        input.pointer_pressed && rect.contains(input.pointer.0, input.pointer.1)
    }
}

/// How loudly a button asks to be pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    /// The one action a screen is for: filled with the accent.
    Primary,
    /// An ordinary control.
    Secondary,
    /// A quiet one: no surface until the pointer is on it.
    Ghost,
    /// One that throws something away.
    Danger,
}

/// A floating panel: the surface a tooltip or a tool window sits on.
pub fn panel(list: &mut UiDrawList, kit: &Kit, rect: UiRect, border: UiColor) {
    let corner = kit.px(radius::CARD);
    list.shadow(
        rect,
        corner,
        kit.px(18.0),
        color::alpha(color::SHADOW, 0.85),
    );
    list.panel(rect, corner, tokens::GLASS_TOP, tokens::GLASS_BOTTOM);
    // A highlight along the top edge: the panel catches light, it is not a hole in the screen.
    list.panel(
        UiRect::new(
            rect.x + corner,
            rect.y + 1.0,
            rect.width - corner * 2.0,
            1.0,
        ),
        0.0,
        [1.0, 1.0, 1.0, 0.06],
        [1.0, 1.0, 1.0, 0.06],
    );
    list.panel_stroke(rect, corner, kit.px(1.0).max(1.0), border);
}

/// A button: an optional icon and a label, centred together.
#[allow(
    clippy::too_many_arguments,
    reason = "a button is this many independent facts"
)]
pub fn button(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    label: &str,
    icon: Option<(UiIcon, f32)>,
    variant: ButtonVariant,
    state: Interaction,
) {
    let corner = kit.px(radius::CONTROL);
    let alpha = if state.disabled { 0.38 } else { 1.0 };
    let (fill_top, fill_bottom, stroke, text) = button_colors(variant, state);
    if fill_top[3] > 0.0 {
        list.panel(
            rect,
            corner,
            color::alpha(fill_top, alpha),
            color::alpha(fill_bottom, alpha),
        );
    }
    if stroke[3] > 0.0 {
        list.panel_stroke(rect, corner, 1.0, color::alpha(stroke, alpha));
    }
    let text = color::alpha(text, alpha);
    let style = type_scale::UI;
    let icon_edge = kit.px(16.0);
    let gap = if label.is_empty() { 0.0 } else { kit.px(6.0) };
    let label_width = kit.width(style, label);
    let content = label_width + icon.map_or(0.0, |_| icon_edge + gap);
    let mut x = (rect.x + (rect.width - content) * 0.5).round();
    let centre = rect.y + rect.height * 0.5;
    if let Some((icon, rotation)) = icon {
        list.icon_rotated(
            (x + icon_edge * 0.5, centre),
            icon_edge,
            icon,
            rotation,
            text,
        );
        x += icon_edge + gap;
    }
    kit.text(list, x, centre, label, style, text, TextAlign::Left);
}

/// The fill, the edge and the text of a button in a state.
fn button_colors(
    variant: ButtonVariant,
    state: Interaction,
) -> (UiColor, UiColor, UiColor, UiColor) {
    let clear = [0.0; 4];
    match variant {
        ButtonVariant::Primary => {
            let (top, bottom) = if state.pressed {
                (tokens::ACCENT_DIM, color::BRONZE_DIM)
            } else if state.hovered {
                ([0.95, 0.83, 0.62, 1.0], tokens::ACCENT)
            } else {
                (tokens::ACCENT, tokens::ACCENT_DIM)
            };
            (top, bottom, [1.0, 0.95, 0.85, 0.35], tokens::ON_ACCENT)
        }
        ButtonVariant::Secondary | ButtonVariant::Ghost | ButtonVariant::Danger => {
            let ghost = variant != ButtonVariant::Secondary;
            let fill = if state.pressed {
                tokens::CONTROL_PRESSED
            } else if state.active {
                color::alpha(tokens::ACCENT, 0.20)
            } else if state.hovered {
                tokens::CONTROL_HOVER
            } else if ghost {
                clear
            } else {
                tokens::CONTROL
            };
            let stroke = if state.active {
                color::alpha(tokens::ACCENT, 0.75)
            } else if state.hovered {
                color::alpha([1.0, 1.0, 1.0, 1.0], 0.16)
            } else if ghost {
                clear
            } else {
                color::BORDER
            };
            let text = match variant {
                ButtonVariant::Danger => color::CRIMSON,
                _ if state.active => tokens::ACCENT,
                _ if state.hovered => color::TEXT_PRIMARY,
                _ => [0.86, 0.85, 0.82, 1.0],
            };
            (fill, fill, stroke, text)
        }
    }
}

/// A square button with an icon in it.
pub fn icon_button(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    icon: UiIcon,
    rotation: f32,
    variant: ButtonVariant,
    state: Interaction,
) {
    button(list, kit, rect, "", Some((icon, rotation)), variant, state);
}

/// A section heading across `width` at `(x, top)`: small capitals and a rule after them.
/// Returns its height.
pub fn section(list: &mut UiDrawList, kit: &Kit, x: f32, top: f32, width: f32, title: &str) -> f32 {
    let style = type_scale::LABEL;
    let height = kit.px(22.0);
    let centre = top + height * 0.5;
    kit.text(
        list,
        x,
        centre,
        title,
        style,
        color::TEXT_MUTED,
        TextAlign::Left,
    );
    let start = x + kit.width(style, title) + kit.px(10.0);
    if start < x + width {
        list.rect(
            UiRect::new(
                start.round(),
                centre.round(),
                (x + width - start).round(),
                1.0,
            ),
            color::BORDER,
        );
    }
    height
}

/// Where a stepper's parts are inside `rect`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepperParts {
    /// The decrease button.
    pub minus: UiRect,
    /// The value between them.
    pub value: UiRect,
    /// The increase button.
    pub plus: UiRect,
}

/// Lays a stepper out: the label on the left, `[ − ] value [ + ]` on the right.
#[must_use]
pub fn stepper_parts(kit: &Kit, rect: UiRect) -> StepperParts {
    let button = rect.height;
    let value = kit.px(64.0);
    let right = rect.right();
    StepperParts {
        plus: UiRect::new(right - button, rect.y, button, rect.height),
        value: UiRect::new(right - button - value, rect.y, value, rect.height),
        minus: UiRect::new(right - button * 2.0 - value, rect.y, button, rect.height),
    }
}

/// `Label ......... [ − ]  value  [ + ]`.
#[allow(
    clippy::too_many_arguments,
    reason = "a stepper is this many independent facts"
)]
pub fn stepper(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    label: &str,
    value: &str,
    minus: Interaction,
    plus: Interaction,
) {
    let parts = stepper_parts(kit, rect);
    let centre = rect.y + rect.height * 0.5;
    kit.text(
        list,
        rect.x,
        centre,
        label,
        type_scale::UI,
        color::TEXT_SECONDARY,
        TextAlign::Left,
    );
    let well = UiRect::new(
        parts.minus.x,
        rect.y,
        parts.plus.right() - parts.minus.x,
        rect.height,
    );
    let corner = kit.px(radius::CONTROL);
    list.panel(well, corner, tokens::WELL, tokens::WELL);
    list.panel_stroke(well, corner, 1.0, color::BORDER);
    icon_button(
        list,
        kit,
        parts.minus.inset(kit.px(2.0)),
        UiIcon::ControlMinus,
        0.0,
        ButtonVariant::Ghost,
        minus,
    );
    icon_button(
        list,
        kit,
        parts.plus.inset(kit.px(2.0)),
        UiIcon::ControlPlus,
        0.0,
        ButtonVariant::Ghost,
        plus,
    );
    kit.text(
        list,
        parts.value.x + parts.value.width * 0.5,
        centre,
        value,
        type_scale::UI,
        color::TEXT_PRIMARY,
        TextAlign::Center,
    );
}

/// One option of a segmented control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment<'a> {
    /// Its words.
    pub label: &'a str,
    /// Its icon and how far the icon is turned, in radians.
    pub icon: Option<(UiIcon, f32)>,
}

/// Where option `index` of `count` is inside a segmented control in `rect`.
#[must_use]
pub fn segment_rect(rect: UiRect, count: usize, index: usize) -> UiRect {
    let count = count.max(1) as f32;
    let width = rect.width / count;
    UiRect::new(
        (rect.x + width * index as f32).round(),
        rect.y,
        width.round(),
        rect.height,
    )
}

/// A row of mutually exclusive options in a sunken channel; the chosen one raised.
///
/// `states[i]` is option `i`'s interaction; `active` marks the chosen one.
pub fn segmented(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    options: &[Segment<'_>],
    states: &[Interaction],
) {
    let corner = kit.px(radius::CONTROL);
    list.panel(rect, corner, tokens::WELL, tokens::WELL);
    list.panel_stroke(rect, corner, 1.0, color::BORDER);
    for (index, option) in options.iter().enumerate() {
        let cell = segment_rect(rect, options.len(), index).inset(kit.px(2.0));
        let state = states.get(index).copied().unwrap_or_default();
        let variant = ButtonVariant::Ghost;
        button(list, kit, cell, option.label, option.icon, variant, state);
    }
}

/// A switch: a pill track and a knob, on the right when on.
pub fn switch(list: &mut UiDrawList, kit: &Kit, rect: UiRect, on: bool, state: Interaction) {
    let track = UiRect::new(
        rect.right() - kit.px(38.0),
        (rect.y + (rect.height - kit.px(20.0)) * 0.5).round(),
        kit.px(38.0),
        kit.px(20.0),
    );
    let pill = track.height * 0.5;
    let (fill, stroke) = if on {
        (
            color::alpha(tokens::ACCENT, 0.85),
            color::alpha(tokens::ACCENT, 0.9),
        )
    } else if state.hovered {
        (tokens::CONTROL_HOVER, color::alpha([1.0; 4], 0.16))
    } else {
        (tokens::WELL, color::BORDER)
    };
    list.panel(track, pill, fill, fill);
    list.panel_stroke(track, pill, 1.0, stroke);
    let knob = track.height - kit.px(6.0);
    let x = if on {
        track.right() - kit.px(3.0) - knob
    } else {
        track.x + kit.px(3.0)
    };
    list.panel(
        UiRect::new(x, track.y + kit.px(3.0), knob, knob),
        knob * 0.5,
        if on {
            tokens::ON_ACCENT
        } else {
            [0.82, 0.82, 0.80, 1.0]
        },
        if on {
            tokens::ON_ACCENT
        } else {
            [0.66, 0.66, 0.64, 1.0]
        },
    );
}

/// Where cell `index` (row-major, 0 is top-left) of a 3 × 3 anchor grid in `rect` is.
#[must_use]
pub fn anchor_cell(rect: UiRect, index: usize) -> UiRect {
    let (column, row) = ((index % 3) as f32, (index / 3) as f32);
    let (width, height) = (rect.width / 3.0, rect.height / 3.0);
    UiRect::new(
        (rect.x + column * width).round(),
        (rect.y + row * height).round(),
        width.round(),
        height.round(),
    )
}

/// The direction the arrow in anchor cell `index` points, in radians from pointing right; the
/// centre cell has none.
#[must_use]
pub fn anchor_arrow(index: usize) -> Option<f32> {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
    Some(match index {
        0 => -PI + FRAC_PI_4,
        1 => -FRAC_PI_2,
        2 => -FRAC_PI_4,
        3 => PI,
        5 => 0.0,
        6 => PI - FRAC_PI_4,
        7 => FRAC_PI_2,
        8 => FRAC_PI_4,
        _ => return None,
    })
}

/// A 3 × 3 grid of arrows around a dot: which corner, edge or middle something hangs from.
///
/// `states[i]` is cell `i`'s interaction; `active` marks the chosen one.
pub fn anchor_grid(list: &mut UiDrawList, kit: &Kit, rect: UiRect, states: &[Interaction; 9]) {
    let corner = kit.px(radius::CONTROL);
    list.panel(rect, corner, tokens::WELL, tokens::WELL);
    list.panel_stroke(rect, corner, 1.0, color::BORDER);
    for (index, state) in states.iter().enumerate() {
        let cell = anchor_cell(rect, index).inset(kit.px(2.0));
        if state.active || state.hovered {
            let fill = if state.active {
                color::alpha(tokens::ACCENT, 0.22)
            } else {
                tokens::CONTROL_HOVER
            };
            list.panel(cell, kit.px(4.0), fill, fill);
            if state.active {
                list.panel_stroke(cell, kit.px(4.0), 1.0, color::alpha(tokens::ACCENT, 0.8));
            }
        }
        let tint = if state.active {
            tokens::ACCENT
        } else if state.hovered {
            color::TEXT_PRIMARY
        } else {
            color::TEXT_MUTED
        };
        let centre = (cell.x + cell.width * 0.5, cell.y + cell.height * 0.5);
        match anchor_arrow(index) {
            Some(angle) => {
                let edge = (cell.width.min(cell.height) * 0.62).round();
                list.icon_rotated(centre, edge, UiIcon::ControlArrow, angle, tint);
            }
            None => list.disc(centre, kit.px(4.0), tint),
        }
    }
}

/// A small rounded tag with `text` in it, its left edge at `x` and its top at `top`. Returns
/// its rectangle.
pub fn chip(
    list: &mut UiDrawList,
    kit: &Kit,
    x: f32,
    top: f32,
    text: &str,
    foreground: UiColor,
    background: UiColor,
) -> UiRect {
    let style = type_scale::BADGE;
    let pad = kit.px(7.0);
    let rect = UiRect::new(
        x.round(),
        top.round(),
        (kit.width(style, text) + pad * 2.0).round(),
        kit.px(20.0),
    );
    list.panel(rect, kit.px(radius::CHIP), background, background);
    kit.text(
        list,
        rect.x + pad,
        rect.y + rect.height * 0.5,
        text,
        style,
        foreground,
        TextAlign::Left,
    );
    rect
}

/// The face every kit label is set in, for a caller measuring one.
pub const LABEL_FONT: UiFont = UiFont::Medium;

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render_data::ui::UiCommand;

    fn input_at(pointer: (f32, f32), down: bool, pressed: bool) -> UiInput {
        UiInput {
            pointer,
            pointer_down: down,
            pointer_pressed: pressed,
            ..UiInput::new()
        }
    }

    #[test]
    fn an_interaction_reads_hover_press_and_click_from_the_pointer() {
        let rect = UiRect::new(10.0, 10.0, 100.0, 30.0);
        let state = Interaction::at(rect, &input_at((20.0, 20.0), true, true));
        assert!(state.hovered && state.pressed);
        assert!(Interaction::clicked(
            rect,
            &input_at((20.0, 20.0), true, true)
        ));
        assert!(!Interaction::clicked(
            rect,
            &input_at((200.0, 20.0), true, true)
        ));
        let disabled = state.disabled(true);
        assert!(!disabled.hovered && !disabled.pressed && disabled.disabled);
    }

    #[test]
    fn a_stepper_puts_its_buttons_on_the_right_and_its_value_between_them() {
        let kit = Kit::new(UiScale::default());
        let rect = UiRect::new(0.0, 0.0, 280.0, 28.0);
        let parts = stepper_parts(&kit, rect);
        assert!((parts.plus.right() - rect.right()).abs() < 0.5);
        assert!(parts.minus.right() <= parts.value.x + 0.5);
        assert!(parts.value.right() <= parts.plus.x + 0.5);
    }

    #[test]
    fn segments_tile_their_control_and_anchor_cells_tile_the_grid() {
        let rect = UiRect::new(0.0, 0.0, 300.0, 30.0);
        let first = segment_rect(rect, 3, 0);
        let last = segment_rect(rect, 3, 2);
        assert!(first.x.abs() < 0.5 && (last.right() - 300.0).abs() < 1.0);
        let grid = UiRect::new(0.0, 0.0, 90.0, 90.0);
        assert_eq!(anchor_cell(grid, 4), UiRect::new(30.0, 30.0, 30.0, 30.0));
        assert!(anchor_arrow(4).is_none() && anchor_arrow(0).is_some());
    }

    #[test]
    fn every_control_draws_its_words_in_the_proportional_face() {
        let kit = Kit::new(UiScale::default());
        let mut list = UiDrawList::new();
        let rect = UiRect::new(0.0, 0.0, 200.0, 32.0);
        button(
            &mut list,
            &kit,
            rect,
            "Done",
            None,
            ButtonVariant::Primary,
            Interaction::default(),
        );
        stepper(
            &mut list,
            &kit,
            rect,
            "Scale",
            "1.00",
            Interaction::default(),
            Interaction::default(),
        );
        assert!(
            list.commands()
                .iter()
                .all(|command| !matches!(command, UiCommand::Text { .. })),
            "a kit control used the bitmap font"
        );
        assert!(
            list.commands()
                .iter()
                .any(|command| matches!(command, UiCommand::Label { .. }))
        );
    }
}
