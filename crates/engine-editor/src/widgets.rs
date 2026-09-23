//! The editor's drawing primitives: panels, headers, buttons, fields, checkboxes, chips,
//! tooltips and key caps.
//!
//! Every screen in the editor is built from these, which is what keeps it looking like one
//! tool. A primitive draws itself and nothing else — the rectangle it is handed is the one the
//! input code tests against, so what is drawn and what is clicked are the same shape by
//! construction.

use engine_render_data::ui::{UiColor, UiDrawList, UiRect, UiScale};

use crate::icons::{self, Icon};
use crate::theme::{self, Type};

/// How a control is drawn: which of the states a pointer can put it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    /// On: the current tool, a toggle that is set.
    pub active: bool,
    /// Under the pointer.
    pub hover: bool,
    /// Held down.
    pub pressed: bool,
    /// Cannot be used right now.
    pub disabled: bool,
}

/// A draw list and the scale it is being drawn at.
pub struct Paint<'a> {
    /// Where everything goes.
    pub list: &'a mut UiDrawList,
    /// The interface scale.
    pub scale: UiScale,
    /// The text size at that scale.
    pub ty: Type,
}

impl<'a> Paint<'a> {
    /// A painter over `list` at `scale`.
    pub fn new(list: &'a mut UiDrawList, scale: UiScale) -> Self {
        Self {
            list,
            scale,
            ty: Type::at(scale),
        }
    }

    /// A logical size in physical pixels.
    #[must_use]
    pub fn px(&self, logical: f32) -> f32 {
        self.scale.px(logical)
    }

    /// A filled rectangle.
    pub fn fill(&mut self, rect: UiRect, color: UiColor) {
        self.list.rect(rect, color);
    }

    /// A one-pixel outline, drawn inside `rect`.
    pub fn frame(&mut self, rect: UiRect, color: UiColor) {
        self.list.outline(rect, self.px(1.0).max(1.0), color);
    }

    /// Text with its top-left at `origin`.
    pub fn text(&mut self, origin: (f32, f32), text: &str, color: UiColor) {
        self.list.text_at(
            (origin.0.round(), origin.1.round()),
            text,
            color,
            self.ty.cell,
        );
    }

    /// Text starting at `x`, centred vertically in `rect`, cut to end at `right`.
    pub fn label(&mut self, rect: UiRect, x: f32, right: f32, text: &str, color: UiColor) {
        let fitted = self.ty.fit(text, right - x);
        let y = rect.y + (rect.height - self.ty.height()) * 0.5;
        self.text((x, y), &fitted, color);
    }

    /// Text centred in `rect`, cut to fit it.
    pub fn centred(&mut self, rect: UiRect, text: &str, color: UiColor) {
        let fitted = self.ty.fit(text, rect.width);
        let width = self.ty.width(&fitted);
        let y = rect.y + (rect.height - self.ty.height()) * 0.5;
        self.text((rect.x + (rect.width - width) * 0.5, y), &fitted, color);
    }

    /// Text ending at `right`, centred vertically in `rect`.
    pub fn right(&mut self, rect: UiRect, right: f32, text: &str, color: UiColor) {
        let width = self.ty.width(text);
        let y = rect.y + (rect.height - self.ty.height()) * 0.5;
        self.text((right - width, y), text, color);
    }

    /// An icon centred in `rect`.
    pub fn icon(&mut self, icon: Icon, rect: UiRect, color: UiColor) {
        icons::draw_in(self.list, icon, rect, self.ty.cell, color);
    }

    /// The square an icon is centred in when it sits at `x` in a row.
    #[must_use]
    pub fn icon_slot(&self, row: UiRect, x: f32) -> UiRect {
        let side = self.px(theme::metrics::ICON_SLOT);
        UiRect::new(x, row.y + (row.height - side) * 0.5, side, side)
    }

    /// A panel's body, with the hairline that separates it from its neighbours.
    pub fn panel(&mut self, rect: UiRect) {
        self.fill(rect, theme::PANEL);
        self.frame(rect, theme::BORDER);
    }

    /// A panel's header strip: a title on the left, and a count beside it if there is one.
    pub fn header(&mut self, rect: UiRect, title: &str, detail: Option<&str>) {
        self.fill(rect, theme::PANEL_HEADER);
        self.fill(
            UiRect::new(
                rect.x,
                rect.bottom() - self.px(1.0),
                rect.width,
                self.px(1.0),
            ),
            theme::BORDER,
        );
        let x = rect.x + self.px(theme::metrics::PADDING + 2.0);
        self.label(rect, x, rect.right(), title, theme::TEXT_STRONG);
        if let Some(detail) = detail {
            let after = x + self.ty.width(title) + self.px(8.0);
            self.label(rect, after, rect.right(), detail, theme::TEXT_FAINT);
        }
    }

    /// A vertical hairline on a panel.
    pub fn divider_v(&mut self, x: f32, top: f32, bottom: f32) {
        self.fill(
            UiRect::new(x.round(), top, self.px(1.0).max(1.0), bottom - top),
            theme::DIVIDER,
        );
    }

    /// A horizontal hairline on a panel.
    pub fn divider_h(&mut self, left: f32, right: f32, y: f32) {
        self.fill(
            UiRect::new(left, y.round(), right - left, self.px(1.0).max(1.0)),
            theme::DIVIDER,
        );
    }

    /// The fill a control gets from its state.
    pub fn state_fill(&mut self, rect: UiRect, state: State) {
        if state.active {
            self.fill(rect, theme::ACCENT_FILL);
            self.fill(
                UiRect::new(
                    rect.x,
                    rect.bottom() - self.px(2.0),
                    rect.width,
                    self.px(2.0),
                ),
                theme::ACCENT,
            );
        }
        if state.hover && !state.disabled {
            self.fill(rect, theme::HOVER);
        }
        if state.pressed && !state.disabled {
            self.fill(rect, theme::PRESSED);
        }
    }

    /// The colour a control's icon or label is drawn in, from its state.
    #[must_use]
    pub fn ink(state: State) -> UiColor {
        if state.disabled {
            theme::TEXT_FAINT
        } else if state.active {
            theme::TEXT_STRONG
        } else if state.hover {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        }
    }

    /// A square icon button.
    pub fn button(&mut self, rect: UiRect, icon: Icon, state: State) {
        self.state_fill(rect, state);
        self.icon(icon, rect, Self::ink(state));
    }

    /// A button with an icon and a word, or only a word.
    pub fn text_button(&mut self, rect: UiRect, icon: Option<Icon>, text: &str, state: State) {
        self.state_fill(rect, state);
        let ink = Self::ink(state);
        let mut x = rect.x + self.px(6.0);
        if let Some(icon) = icon {
            let slot = self.icon_slot(rect, rect.x + self.px(4.0));
            self.icon(icon, slot, ink);
            x = slot.right() + self.px(3.0);
        }
        self.label(rect, x, rect.right() - self.px(4.0), text, ink);
    }

    /// A text field: a dark well, an optional icon, the text or a faint placeholder, and a
    /// caret and an accent edge while it has the keyboard.
    pub fn field(
        &mut self,
        rect: UiRect,
        icon: Option<Icon>,
        text: &str,
        placeholder: &str,
        focused: bool,
        hover: bool,
    ) {
        self.fill(rect, theme::FIELD);
        self.frame(
            rect,
            if focused {
                theme::ACCENT
            } else if hover {
                theme::DIVIDER
            } else {
                theme::BORDER
            },
        );
        let mut x = rect.x + self.px(5.0);
        if let Some(icon) = icon {
            let slot = self.icon_slot(rect, rect.x + self.px(2.0));
            self.icon(icon, slot, theme::TEXT_FAINT);
            x = slot.right() + self.px(2.0);
        }
        let right = rect.right() - self.px(5.0);
        if text.is_empty() && !focused {
            self.label(rect, x, right, placeholder, theme::TEXT_FAINT);
            return;
        }
        // The end of what was typed stays in view: a long filter scrolls, it does not clip
        // the caret off the end of the field.
        let room = self.ty.fits(right - x).saturating_sub(usize::from(focused));
        let shown: String = {
            let count = text.chars().count();
            text.chars().skip(count.saturating_sub(room)).collect()
        };
        self.label(rect, x, right, &shown, theme::TEXT);
        if focused {
            let caret_x = x + self.ty.width(&shown) + self.px(1.0);
            let height = self.ty.height() + self.px(2.0);
            self.fill(
                UiRect::new(
                    caret_x,
                    rect.y + (rect.height - height) * 0.5,
                    self.px(1.0).max(1.0),
                    height,
                ),
                theme::ACCENT,
            );
        }
    }

    /// A checkbox.
    pub fn checkbox(&mut self, rect: UiRect, checked: bool, hover: bool) {
        let side = self.px(12.0);
        let bx = UiRect::new(rect.x, rect.y + (rect.height - side) * 0.5, side, side);
        self.fill(bx, if checked { theme::ACCENT } else { theme::FIELD });
        self.frame(
            bx,
            if hover {
                theme::TEXT_DIM
            } else if checked {
                theme::ACCENT
            } else {
                theme::DIVIDER
            },
        );
        if checked {
            self.icon(crate::icons::Icon::Check, bx, theme::TEXT_STRONG);
        }
    }

    /// A drop shadow under something raised.
    pub fn shadow(&mut self, rect: UiRect) {
        let offset = self.px(3.0);
        self.fill(
            UiRect::new(rect.x + offset, rect.y + offset, rect.width, rect.height),
            theme::SHADOW,
        );
    }

    /// A small dark chip over the viewport.
    pub fn chip(&mut self, rect: UiRect) {
        self.fill(rect, theme::OVERLAY);
    }

    /// How wide a chip holding `text` (and an icon, if there is one) is.
    #[must_use]
    pub fn chip_width(&self, text: &str, icon: bool) -> f32 {
        self.ty.width(text)
            + self.px(12.0)
            + if icon {
                self.px(theme::metrics::ICON_SLOT + 2.0)
            } else {
                0.0
            }
    }

    /// A chip with an optional icon and a line of text, `rect` wide.
    pub fn chip_text(
        &mut self,
        rect: UiRect,
        icon: Option<(Icon, UiColor)>,
        text: &str,
        color: UiColor,
    ) {
        self.chip(rect);
        let mut x = rect.x + self.px(6.0);
        if let Some((icon, tint)) = icon {
            let slot = self.icon_slot(rect, rect.x + self.px(3.0));
            self.icon(icon, slot, tint);
            x = slot.right() + self.px(2.0);
        }
        self.label(rect, x, rect.right(), text, color);
    }

    /// A key cap: `CTRL`, `S`. Returns how wide it was.
    pub fn key(&mut self, origin: (f32, f32), key: &str, height: f32) -> f32 {
        let width = self.ty.width(key) + self.px(8.0);
        let rect = UiRect::new(origin.0, origin.1, width, height);
        self.fill(rect, theme::FIELD);
        self.frame(rect, theme::DIVIDER);
        self.centred(rect, key, theme::TEXT);
        width
    }

    /// A tooltip beside `pointer`: a title and, faintly, the key that does the same thing.
    /// Kept inside the window.
    pub fn tooltip(
        &mut self,
        pointer: (f32, f32),
        title: &str,
        shortcut: &str,
        window: (u32, u32),
    ) {
        let pad = self.px(6.0);
        let gap = self.px(10.0);
        let width = self.ty.width(title)
            + if shortcut.is_empty() {
                0.0
            } else {
                gap + self.ty.width(shortcut)
            }
            + pad * 2.0;
        let height = self.ty.height() + pad * 2.0;
        let x = (pointer.0 + self.px(12.0)).min(window.0 as f32 - width - pad);
        let mut y = pointer.1 + self.px(18.0);
        if y + height > window.1 as f32 {
            y = pointer.1 - height - self.px(6.0);
        }
        let rect = UiRect::new(x.max(0.0), y.max(0.0), width, height);
        self.shadow(rect);
        self.fill(rect, theme::RAISED);
        self.frame(rect, theme::DIVIDER);
        self.text((rect.x + pad, rect.y + pad), title, theme::TEXT_STRONG);
        if !shortcut.is_empty() {
            self.text(
                (rect.x + pad + self.ty.width(title) + gap, rect.y + pad),
                shortcut,
                theme::TEXT_DIM,
            );
        }
    }
}
