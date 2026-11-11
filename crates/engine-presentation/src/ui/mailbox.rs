//! The mailbox: the letters a character has waiting, and a way to take what is in them.
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────────────────┐
//! │ Mailbox                                                   3 letters · 2 new   │
//! ├───────────────────────────────┬───────────────────────────────────────────────┤
//! │ ● [icon] Dungeon Chest  2 min │ Your bags were full                           │
//! │          Your bags were full  │ From Dungeon Chest · 2 min ago                │
//! │   [icon] Dungeon Chest  3 day │ ───────────────────────────────────────────── │
//! │          Left in the chest    │ Your bags were full when you opened the       │
//! │                               │ chest, so we sent this along.                 │
//! │                               │ ATTACHMENT ─────────────────────────────────  │
//! │                               │ [icon]  Ember-Wrought Band                    │
//! │ [        TAKE ALL        ]    │ [   TAKE   ]                                  │
//! └───────────────────────────────┴───────────────────────────────────────────────┘
//! ```
//!
//! # Drawn like the sheet, and as ignorant
//!
//! The same kit tokens as [`super::character_menu`], the same slots, the same item tooltip
//! docked beside what the pointer is over. And like the sheet this owns no items: it reads the
//! [`Mailbox`] the application hands it and answers with a [`MailboxOutcome`] - a *request*.
//! Offline the application turns it into a [`engine_game::PlayerCommand`]; online into a call
//! to the API. Either way the window is redrawn from the next frame's mailbox, which is the
//! mailbox as it actually is.
//!
//! # What it does keep
//!
//! Which letter is open, how far the list is scrolled, and the line of feedback ("Your bags
//! are full.") that fades after a few seconds. All three are questions about the pointer, which
//! gameplay has no opinion about.

use engine_game::character::{ItemStack, MailItem, MailReason, Mailbox, item};
use engine_render_data::ui::{TextAlign, UiClip, UiColor, UiDrawList, UiRect, UiScale};

use super::character_menu::{icon_for_item, quality_color};
use super::input::UiInput;
use super::kit::{self, ButtonVariant, Interaction, Kit, tokens};
use super::map_style::{blend, color, push_title_case, radius, type_scale};
use super::tooltip::{ItemContext, ItemPlace, ItemTooltip};
use crate::game_ui::write_integer;
use crate::snapshot::CharacterView;

/// How long the line of feedback stays up, in seconds.
pub const NOTICE_SECONDS: f32 = 3.5;

/// What the bag-full refusal says. One string, so offline and online agree.
pub const BAG_FULL_NOTICE: &str = "Your bags are full.";

/// Layout of the panel, in design pixels.
///
/// Fits inside 1280x720 at an interface scale of one, which a test asserts; a larger scale
/// shrinks it to fit, as the sheet does.
mod layout {
    /// Width of the whole panel.
    pub const PANEL_WIDTH: f32 = 960.0;
    /// Height of the whole panel.
    pub const PANEL_HEIGHT: f32 = 625.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 20.0;
    /// Height of the heading strip.
    pub const HEADER: f32 = 60.0;
    /// Height of the hint line along the bottom.
    pub const FOOTER: f32 = 30.0;
    /// Width of the letter list on the left.
    pub const LIST_WIDTH: f32 = 380.0;
    /// Gap between the list and the open letter.
    pub const COLUMN_GAP: f32 = 16.0;
    /// Height of one row of the list.
    pub const ROW: f32 = 72.0;
    /// Gap between two rows.
    pub const ROW_GAP: f32 = 6.0;
    /// Padding inside a card.
    pub const CARD_PAD: f32 = 8.0;
    /// Height of the TAKE ALL button under the list.
    pub const BUTTON: f32 = 40.0;
    /// Gap between the list card and the button under it.
    pub const BUTTON_GAP: f32 = 10.0;
    /// Edge of the attachment slot in the open letter.
    pub const ATTACHMENT: f32 = 72.0;
    /// Edge of the small item slot in a row.
    pub const ROW_ICON: f32 = 52.0;
    /// Width of the scroll bar.
    pub const SCROLLBAR: f32 = 6.0;
}

/// What the window decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxOutcome {
    /// Nothing happened.
    Idle,
    /// The player asked for letter `id`'s item.
    Take {
        /// The letter.
        id: u64,
    },
    /// The player asked for every letter's item.
    TakeAll,
    /// The player opened an unread letter.
    MarkRead {
        /// The letter.
        id: u64,
    },
    /// The window was closed.
    Close,
}

/// The mailbox window: whether it is open, which letter is open and how the list is scrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct MailboxMenu {
    open: bool,
    scale: UiScale,
    selected: Option<u64>,
    /// The letter a read was last requested for, so a letter is asked about once per opening.
    read_requested: Option<u64>,
    hovered_row: Option<u64>,
    hovered_icon: Option<u64>,
    attachment_hovered: bool,
    scroll: f32,
    notice: Option<f32>,
}

impl Default for MailboxMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl MailboxMenu {
    /// Closed.
    #[must_use]
    pub fn new() -> Self {
        Self::with_scale(UiScale::default())
    }

    /// Closed, laid out at `scale`.
    #[must_use]
    pub fn with_scale(scale: UiScale) -> Self {
        Self {
            open: false,
            scale,
            selected: None,
            read_requested: None,
            hovered_row: None,
            hovered_icon: None,
            attachment_hovered: false,
            scroll: 0.0,
            notice: None,
        }
    }

    /// Lays the window out at a new scale from here on.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.scale = scale;
    }

    /// The scale for a window of `viewport` pixels: the configured one, reduced until the whole
    /// panel fits.
    #[must_use]
    pub fn scale_for(&self, viewport: (u32, u32)) -> UiScale {
        let fit =
            (viewport.0 as f32 / layout::PANEL_WIDTH).min(viewport.1 as f32 / layout::PANEL_HEIGHT);
        if fit >= self.scale.factor() || fit <= 0.0 {
            self.scale
        } else {
            UiScale::new(fit.max(UiScale::MIN))
        }
    }

    /// Whether the window is on screen.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
        self.selected = None;
        self.read_requested = None;
        self.hovered_row = None;
        self.hovered_icon = None;
        self.attachment_hovered = false;
        self.scroll = 0.0;
        self.notice = None;
    }

    /// Closes it.
    pub fn close(&mut self) {
        self.open = false;
        self.hovered_row = None;
        self.hovered_icon = None;
        self.attachment_hovered = false;
    }

    /// The letter that is open.
    #[must_use]
    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    /// Says `BAG_FULL_NOTICE` for a few seconds.
    pub fn notify_bag_full(&mut self) {
        self.notice = Some(0.0);
    }

    /// Whether the feedback line is up.
    #[must_use]
    pub fn has_notice(&self) -> bool {
        self.notice.is_some()
    }

    /// Ages the feedback line by `delta` real seconds.
    pub fn advance(&mut self, delta: f32) {
        if let Some(age) = self.notice.as_mut() {
            *age += delta;
            if *age >= NOTICE_SECONDS {
                self.notice = None;
            }
        }
    }

    // ---- geometry ---------------------------------------------------------------------

    fn kit(&self, viewport: (u32, u32)) -> Kit {
        Kit::new(self.scale_for(viewport))
    }

    /// The panel, centred on the viewport.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let (width, height) = (kit.px(layout::PANEL_WIDTH), kit.px(layout::PANEL_HEIGHT));
        UiRect::new(
            ((viewport.0 as f32 - width) * 0.5).max(0.0).round(),
            ((viewport.1 as f32 - height) * 0.5).max(0.0).round(),
            width,
            height,
        )
    }

    fn body_top(&self, viewport: (u32, u32)) -> f32 {
        self.panel_rect(viewport).y + self.kit(viewport).px(layout::HEADER)
    }

    fn body_bottom(&self, viewport: (u32, u32)) -> f32 {
        let kit = self.kit(viewport);
        self.panel_rect(viewport).bottom() - kit.px(layout::PADDING) - kit.px(layout::FOOTER)
    }

    /// The card the letters are listed in.
    #[must_use]
    pub fn list_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let panel = self.panel_rect(viewport);
        let top = self.body_top(viewport);
        UiRect::new(
            panel.x + kit.px(layout::PADDING),
            top,
            kit.px(layout::LIST_WIDTH),
            self.body_bottom(viewport) - top - kit.px(layout::BUTTON) - kit.px(layout::BUTTON_GAP),
        )
    }

    /// The area rows scroll inside.
    fn list_viewport(&self, viewport: (u32, u32)) -> UiRect {
        self.list_rect(viewport)
            .inset(self.kit(viewport).px(layout::CARD_PAD))
    }

    /// The TAKE ALL button, under the list.
    #[must_use]
    pub fn take_all_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let list = self.list_rect(viewport);
        UiRect::new(
            list.x,
            list.bottom() + kit.px(layout::BUTTON_GAP),
            list.width,
            kit.px(layout::BUTTON),
        )
    }

    /// The card the open letter is drawn in.
    #[must_use]
    pub fn detail_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let panel = self.panel_rect(viewport);
        let list = self.list_rect(viewport);
        let left = list.right() + kit.px(layout::COLUMN_GAP);
        let top = self.body_top(viewport);
        UiRect::new(
            left,
            top,
            panel.right() - kit.px(layout::PADDING) - left,
            self.body_bottom(viewport) - top,
        )
    }

    /// The TAKE button in the open letter.
    #[must_use]
    pub fn take_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let detail = self.detail_rect(viewport).inset(kit.px(20.0));
        UiRect::new(
            detail.x,
            detail.bottom() - kit.px(layout::BUTTON),
            kit.px(180.0),
            kit.px(layout::BUTTON),
        )
    }

    /// The Close button, bottom right of the panel.
    #[must_use]
    pub fn close_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let panel = self.panel_rect(viewport);
        let (width, height) = (kit.px(84.0), kit.px(26.0));
        UiRect::new(
            panel.right() - kit.px(layout::PADDING) - width,
            panel.bottom() - kit.px(layout::PADDING) - height * 0.5 - kit.px(layout::FOOTER) * 0.25
                + kit.px(2.0),
            width,
            height,
        )
    }

    fn row_pitch(&self, viewport: (u32, u32)) -> f32 {
        let kit = self.kit(viewport);
        kit.px(layout::ROW) + kit.px(layout::ROW_GAP)
    }

    /// The largest scroll offset for `count` letters.
    fn max_scroll(&self, count: usize, viewport: (u32, u32)) -> f32 {
        let kit = self.kit(viewport);
        let content = count as f32 * self.row_pitch(viewport) - kit.px(layout::ROW_GAP);
        (content - self.list_viewport(viewport).height).max(0.0)
    }

    /// Where row `index` (0 is the newest letter) is, scroll applied.
    #[must_use]
    pub fn row_rect(&self, index: usize, count: usize, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let area = self.list_viewport(viewport);
        let bar = if self.max_scroll(count, viewport) > 0.0 {
            kit.px(layout::SCROLLBAR) + kit.px(6.0)
        } else {
            0.0
        };
        UiRect::new(
            area.x,
            area.y + index as f32 * self.row_pitch(viewport) - self.scroll,
            area.width - bar,
            kit.px(layout::ROW),
        )
    }

    /// The small item slot inside a row.
    fn row_icon_rect(&self, row: UiRect, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let edge = kit.px(layout::ROW_ICON);
        UiRect::new(
            row.x + kit.px(22.0),
            row.y + (row.height - edge) * 0.5,
            edge,
            edge,
        )
    }

    /// The attachment slot in the open letter.
    #[must_use]
    pub fn attachment_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit(viewport);
        let detail = self.detail_rect(viewport).inset(kit.px(20.0));
        let edge = kit.px(layout::ATTACHMENT);
        UiRect::new(detail.x, detail.y + kit.px(188.0), edge, edge)
    }

    // ---- input ------------------------------------------------------------------------

    /// Reads the frame's pointer and wheel and reports what the player asked for.
    ///
    /// `wheel` is in notches, positive away from the player. `character` is the bag the item
    /// would go to: a full one is answered here with the feedback line rather than by a request
    /// the server is certain to refuse.
    pub fn update(
        &mut self,
        input: &UiInput,
        wheel: f32,
        mailbox: &Mailbox,
        character: &CharacterView,
        viewport: (u32, u32),
    ) -> MailboxOutcome {
        if !self.open {
            return MailboxOutcome::Idle;
        }
        if input.cancel {
            self.close();
            return MailboxOutcome::Close;
        }

        let letters = mailbox.letters();
        let count = letters.len();
        // The open letter is gone (taken): open the newest one instead.
        if self.selected.is_none_or(|id| mailbox.get(id).is_none()) {
            self.selected = letters.last().map(|letter| letter.id);
        }

        let (x, y) = input.pointer;
        let panel = self.panel_rect(viewport);
        let area = self.list_viewport(viewport);

        if wheel != 0.0 && self.list_rect(viewport).contains(x, y) {
            self.scroll -= wheel * self.row_pitch(viewport);
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll(count, viewport));

        // What the pointer is over.
        self.hovered_row = None;
        self.hovered_icon = None;
        for (index, letter) in letters.iter().rev().enumerate() {
            let row = self.row_rect(index, count, viewport);
            if row.bottom() < area.y || row.y > area.bottom() {
                continue;
            }
            if area.contains(x, y) && row.contains(x, y) {
                self.hovered_row = Some(letter.id);
                if self.row_icon_rect(row, viewport).contains(x, y) {
                    self.hovered_icon = Some(letter.id);
                }
            }
        }
        self.attachment_hovered =
            self.selected.is_some() && self.attachment_rect(viewport).contains(x, y);

        if input.pointer_pressed {
            if let Some(id) = self.hovered_row {
                self.selected = Some(id);
            } else if self.close_rect(viewport).contains(x, y) {
                self.close();
                return MailboxOutcome::Close;
            } else if self.take_rect(viewport).contains(x, y)
                && let Some(id) = self.selected
            {
                if bag_is_full(character) {
                    self.notify_bag_full();
                } else {
                    return MailboxOutcome::Take { id };
                }
            } else if self.take_all_rect(viewport).contains(x, y) && count > 0 {
                if bag_is_full(character) {
                    self.notify_bag_full();
                } else {
                    return MailboxOutcome::TakeAll;
                }
            } else if !panel.contains(x, y) {
                self.close();
                return MailboxOutcome::Close;
            }
        }

        // Opening an unread letter is reading it. Asked once per letter per opening.
        if let Some(id) = self.selected
            && self.read_requested != Some(id)
            && mailbox.get(id).is_some_and(|letter| !letter.read)
        {
            self.read_requested = Some(id);
            return MailboxOutcome::MarkRead { id };
        }
        MailboxOutcome::Idle
    }
}

/// Whether the bag has no free cell.
fn bag_is_full(character: &CharacterView) -> bool {
    character.inventory.iter().all(Option::is_some)
}

/// The window's subject line for a letter.
#[must_use]
pub fn subject(reason: MailReason) -> &'static str {
    match reason {
        MailReason::BagFull => "Your bags were full",
        MailReason::Unclaimed => "Left in the chest",
        MailReason::SlotRemoved => "Its slot was removed",
        MailReason::DungeonDrop => "Dropped in the dungeon",
    }
}

/// The body of a letter: why it was sent.
#[must_use]
pub fn body(reason: MailReason) -> &'static str {
    match reason {
        MailReason::BagFull => {
            "Your bags were full when you opened the chest, so we sent this along. Make some \
             room and take it whenever you like."
        }
        MailReason::Unclaimed => {
            "You did not take this from the chest before the dungeon closed, so we sent it to \
             you. Nothing is ever lost - take it whenever you are ready."
        }
        MailReason::SlotRemoved => {
            "This was worn in an equipment slot that no longer exists, and your bags were full.              Make some room and take it whenever you like."
        }
        MailReason::DungeonDrop => {
            "A monster dropped this while you were in the dungeon. Take it whenever you are \
             ready."
        }
    }
}

/// How long ago `then` was at `now`, both unix seconds: `Just now`, `2 min ago`, `3 days ago`.
pub fn write_relative_time(out: &mut String, now: f64, then: f64) {
    let seconds = (now - then).max(0.0);
    if seconds < 45.0 {
        out.push_str("Just now");
        return;
    }
    let (amount, unit) = if seconds < 3_600.0 {
        ((seconds / 60.0).round().max(1.0), "min")
    } else if seconds < 86_400.0 {
        ((seconds / 3_600.0).round(), "hr")
    } else {
        ((seconds / 86_400.0).floor(), "day")
    };
    write_integer(out, amount as i64);
    out.push(' ');
    out.push_str(unit);
    if unit == "day" && amount as i64 != 1 {
        out.push('s');
    }
    out.push_str(" ago");
}

/// Draws the mailbox, if it is open.
///
/// `now` is the current unix time in seconds, for the "2 min ago" lines. `tooltip` is the
/// caller's, so hovering an item rewrites strings that already exist.
#[allow(
    clippy::too_many_arguments,
    reason = "a window is drawn from this many independent facts"
)]
pub fn push_mailbox(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    mailbox: &Mailbox,
    character: &CharacterView,
    pointer: (f32, f32),
    now: f64,
    tooltip: &mut ItemTooltip,
    viewport: (u32, u32),
) {
    if !menu.is_open() {
        return;
    }
    let input = &UiInput::new().with_pointer(pointer.0, pointer.1);
    let kit = menu.kit(viewport);
    let panel = menu.panel_rect(viewport);
    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        color::SCRIM,
    );
    kit::panel(list, &kit, panel, color::BORDER_STRONG);

    push_header(list, menu, &kit, mailbox, viewport);
    push_list(list, menu, &kit, mailbox, now, viewport);
    push_detail(list, menu, &kit, mailbox, character, input, now, viewport);

    // The footer: how it is worked, and the way out.
    let padding = kit.px(layout::PADDING);
    kit.text(
        list,
        panel.x + padding,
        panel.bottom() - padding - kit.px(layout::FOOTER) * 0.25,
        "Select a letter to read it  -  Take moves its item to your bag  -  Esc closes",
        type_scale::META,
        color::TEXT_MUTED,
        TextAlign::Left,
    );
    let close = menu.close_rect(viewport);
    kit::button(
        list,
        &kit,
        close,
        "Close",
        None,
        ButtonVariant::Ghost,
        Interaction::at(close, input),
    );

    push_tooltip(list, menu, &kit, mailbox, character, tooltip, viewport);
}

/// A card: a raised rounded surface with a hairline edge.
fn card(list: &mut UiDrawList, kit: &Kit, rect: UiRect) {
    let corner = kit.px(radius::CARD);
    list.panel(rect, corner, color::SURFACE_2, color::SURFACE_1);
    list.panel_stroke(rect, corner, 1.0, color::BORDER);
}

fn push_header(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    mailbox: &Mailbox,
    viewport: (u32, u32),
) {
    let panel = menu.panel_rect(viewport);
    let padding = kit.px(layout::PADDING);
    let centre = panel.y + kit.px(layout::HEADER) * 0.5 + kit.px(2.0);
    kit.text(
        list,
        panel.x + padding,
        centre,
        "Mailbox",
        type_scale::TITLE,
        color::TEXT_TITLE,
        TextAlign::Left,
    );

    let mut line = String::with_capacity(32);
    write_integer(&mut line, mailbox.len() as i64);
    line.push_str(if mailbox.len() == 1 {
        " letter"
    } else {
        " letters"
    });
    let unread = mailbox.unread_count();
    if unread > 0 {
        line.push_str(" · ");
        write_integer(&mut line, unread as i64);
        line.push_str(" new");
    }
    kit.text(
        list,
        panel.right() - padding,
        centre,
        &line,
        type_scale::HEADING,
        if unread > 0 {
            color::BRONZE_BRIGHT
        } else {
            color::TEXT_SECONDARY
        },
        TextAlign::Right,
    );
    list.rect(
        UiRect::new(
            panel.x + padding,
            panel.y + kit.px(layout::HEADER) - kit.px(6.0),
            panel.width - padding * 2.0,
            1.0,
        ),
        color::BORDER,
    );
}

/// An item slot: a pocket tinted by quality, the icon, and an edge.
fn item_slot(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    stack: ItemStack,
    hovered: bool,
    alpha: f32,
) {
    let Some(def) = stack.def() else {
        return;
    };
    let corner = kit.px(radius::CONTROL);
    let quality = quality_color(def.quality);
    list.panel(
        rect,
        corner,
        color::alpha(blend(tokens::CONTROL, quality, 0.22), alpha),
        color::alpha(blend(tokens::WELL, quality, 0.08), alpha),
    );
    list.icon(
        rect.inset(rect.width * 0.14),
        icon_for_item(def.icon),
        [1.0, 1.0, 1.0, alpha],
    );
    if hovered {
        list.glow(
            rect,
            corner,
            kit.px(10.0),
            color::alpha(tokens::ACCENT, 0.35),
        );
    }
    let edge: UiColor = if hovered {
        tokens::ACCENT
    } else {
        color::alpha(quality, 0.85 * alpha)
    };
    list.panel_stroke(rect, corner, kit.px(2.0).max(1.0), edge);
}

fn push_list(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    mailbox: &Mailbox,
    now: f64,
    viewport: (u32, u32),
) {
    let card_rect = menu.list_rect(viewport);
    card(list, kit, card_rect);
    let area = menu.list_viewport(viewport);
    let letters = mailbox.letters();
    let count = letters.len();

    if count == 0 {
        kit.text(
            list,
            card_rect.x + card_rect.width * 0.5,
            card_rect.y + card_rect.height * 0.45,
            "Your mailbox is empty.",
            type_scale::NAME,
            color::TEXT_SECONDARY,
            TextAlign::Center,
        );
        kit.text(
            list,
            card_rect.x + card_rect.width * 0.5,
            card_rect.y + card_rect.height * 0.45 + kit.px(28.0),
            "Loot that cannot reach your bag arrives here.",
            type_scale::UI,
            color::TEXT_MUTED,
            TextAlign::Center,
        );
    } else {
        list.clip(UiClip::rect(area));
        for (index, letter) in letters.iter().rev().enumerate() {
            let row = menu.row_rect(index, count, viewport);
            if row.bottom() < area.y || row.y > area.bottom() {
                continue;
            }
            push_row(list, menu, kit, letter, row, now, viewport);
        }
        list.clip(UiClip::NONE);
        push_scrollbar(list, menu, kit, count, viewport);
    }

    let take_all = menu.take_all_rect(viewport);
    let mut label = String::from("TAKE ALL");
    if count > 1 {
        label.push_str(" (");
        write_integer(&mut label, count as i64);
        label.push(')');
    }
    kit::button(
        list,
        kit,
        take_all,
        &label,
        None,
        ButtonVariant::Secondary,
        Interaction::default().disabled(count == 0),
    );
}

fn push_scrollbar(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    count: usize,
    viewport: (u32, u32),
) {
    let max = menu.max_scroll(count, viewport);
    if max <= 0.0 {
        return;
    }
    let area = menu.list_viewport(viewport);
    let width = kit.px(layout::SCROLLBAR);
    let track = UiRect::new(area.right() - width, area.y, width, area.height);
    list.panel(track, width * 0.5, tokens::WELL, tokens::WELL);
    let content = area.height + max;
    let thumb_h = (area.height * area.height / content).max(kit.px(24.0));
    let thumb_y = track.y + (track.height - thumb_h) * (menu.scroll / max);
    let thumb = UiRect::new(track.x, thumb_y, width, thumb_h);
    list.panel(
        thumb,
        width * 0.5,
        color::alpha(color::BRONZE, 0.7),
        color::alpha(color::BRONZE_DIM, 0.7),
    );
}

fn push_row(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    letter: &MailItem,
    row: UiRect,
    now: f64,
    viewport: (u32, u32),
) {
    let selected = menu.selected == Some(letter.id);
    let hovered = menu.hovered_row == Some(letter.id);
    let corner = kit.px(radius::CONTROL);
    if selected {
        list.panel(
            row,
            corner,
            color::alpha(tokens::ACCENT, 0.16),
            color::alpha(tokens::ACCENT, 0.08),
        );
        list.panel_stroke(row, corner, 1.0, color::alpha(tokens::ACCENT, 0.7));
    } else if hovered {
        list.panel(row, corner, tokens::CONTROL_HOVER, tokens::CONTROL_HOVER);
        list.panel_stroke(row, corner, 1.0, color::BORDER);
    } else {
        list.panel(row, corner, tokens::CONTROL, tokens::CONTROL);
        list.panel_stroke(row, corner, 1.0, color::BORDER);
    }

    // The unread dot, in the gutter on the left.
    if !letter.read {
        list.disc(
            (row.x + kit.px(11.0), row.y + row.height * 0.5),
            kit.px(4.0),
            tokens::ACCENT,
        );
    }

    let icon = menu.row_icon_rect(row, viewport);
    item_slot(
        list,
        kit,
        icon,
        ItemStack::with_props(letter.item, letter.props),
        menu.hovered_icon == Some(letter.id),
        1.0,
    );

    let text_x = icon.right() + kit.px(12.0);
    let right = row.right() - kit.px(12.0);
    let mut when = String::with_capacity(16);
    write_relative_time(&mut when, now, letter.received_at);
    let when_width = kit.width(type_scale::META, &when);
    kit.text(
        list,
        right,
        row.y + row.height * 0.32,
        &when,
        type_scale::META,
        color::TEXT_MUTED,
        TextAlign::Right,
    );
    let sender = kit.style.fit(
        type_scale::HEADING,
        letter.sender.name(),
        (right - text_x - when_width - kit.px(10.0)).max(0.0),
    );
    kit.text(
        list,
        text_x,
        row.y + row.height * 0.32,
        &sender,
        type_scale::HEADING,
        if letter.read {
            color::TEXT_SECONDARY
        } else {
            color::TEXT_PRIMARY
        },
        TextAlign::Left,
    );
    let subject = kit.style.fit(
        type_scale::BODY,
        subject(letter.reason),
        (right - text_x).max(0.0),
    );
    kit.text(
        list,
        text_x,
        row.y + row.height * 0.68,
        &subject,
        type_scale::BODY,
        color::TEXT_SECONDARY,
        TextAlign::Left,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "a card is drawn from this many independent facts"
)]
fn push_detail(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    mailbox: &Mailbox,
    character: &CharacterView,
    input: &UiInput,
    now: f64,
    viewport: (u32, u32),
) {
    let rect = menu.detail_rect(viewport);
    card(list, kit, rect);
    let inner = rect.inset(kit.px(20.0));
    let Some(letter) = menu.selected.and_then(|id| mailbox.get(id)) else {
        kit.text(
            list,
            rect.x + rect.width * 0.5,
            rect.y + rect.height * 0.5,
            "Nothing to read.",
            type_scale::BODY,
            color::TEXT_MUTED,
            TextAlign::Center,
        );
        let take = menu.take_rect(viewport);
        kit::button(
            list,
            kit,
            take,
            "TAKE",
            None,
            ButtonVariant::Primary,
            Interaction::default().disabled(true),
        );
        return;
    };

    // Title, and who it is from and when.
    kit.text(
        list,
        inner.x,
        inner.y + kit.px(14.0),
        subject(letter.reason),
        type_scale::ENCOUNTER,
        color::TEXT_TITLE,
        TextAlign::Left,
    );
    let mut line = String::with_capacity(48);
    line.push_str("From ");
    line.push_str(letter.sender.name());
    line.push_str(" · ");
    write_relative_time(&mut line, now, letter.received_at);
    kit.text(
        list,
        inner.x,
        inner.y + kit.px(44.0),
        &line,
        type_scale::UI,
        color::TEXT_SECONDARY,
        TextAlign::Left,
    );
    list.rect(
        UiRect::new(inner.x, inner.y + kit.px(62.0), inner.width, 1.0),
        color::BORDER,
    );

    // The explanation.
    let mut y = inner.y + kit.px(78.0);
    let step = kit.style.line(type_scale::BODY);
    for wrapped in kit
        .style
        .wrap(type_scale::BODY, body(letter.reason), inner.width)
    {
        kit.text(
            list,
            inner.x,
            y + step * 0.5,
            &wrapped,
            type_scale::BODY,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        y += step;
    }

    // The attachment.
    kit::section(
        list,
        kit,
        inner.x,
        inner.y + kit.px(156.0),
        inner.width,
        "ATTACHMENT",
    );
    let slot = menu.attachment_rect(viewport);
    let stack = ItemStack::with_props(letter.item, letter.props);
    item_slot(list, kit, slot, stack, menu.attachment_hovered, 1.0);
    if let Some(def) = item(letter.item) {
        let text_x = slot.right() + kit.px(16.0);
        let quality = quality_color(def.quality);
        let mut name = String::with_capacity(32);
        push_title_case(&mut name, def.name);
        kit.text(
            list,
            text_x,
            slot.y + kit.px(14.0),
            &name,
            type_scale::NAME,
            quality,
            TextAlign::Left,
        );
        let mut meta = String::with_capacity(40);
        push_title_case(&mut meta, def.quality.name());
        meta.push(' ');
        push_title_case(&mut meta, def.family.name());
        meta.push_str(" · Item level ");
        write_integer(&mut meta, i64::from(def.item_level));
        kit.text(
            list,
            text_x,
            slot.y + kit.px(38.0),
            &meta,
            type_scale::UI,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        let mut stats = String::with_capacity(48);
        for (stat, value) in def.stats.entries() {
            if !stats.is_empty() {
                stats.push_str("   ");
            }
            stats.push('+');
            write_integer(&mut stats, value.round() as i64);
            stats.push(' ');
            push_title_case(&mut stats, stat.display_for(character.hero));
        }
        kit.text(
            list,
            text_x,
            slot.y + kit.px(60.0),
            &stats,
            type_scale::BODY,
            color::VERDANT,
            TextAlign::Left,
        );
    }

    // The action, and the answer when it cannot be done.
    let take = menu.take_rect(viewport);
    if menu.has_notice() {
        kit.text(
            list,
            take.right() + kit.px(16.0),
            take.y + take.height * 0.5,
            BAG_FULL_NOTICE,
            type_scale::HEADING,
            color::CRIMSON,
            TextAlign::Left,
        );
    }
    kit::button(
        list,
        kit,
        take,
        "TAKE",
        None,
        ButtonVariant::Primary,
        Interaction::at(take, input),
    );
}

fn push_tooltip(
    list: &mut UiDrawList,
    menu: &MailboxMenu,
    kit: &Kit,
    mailbox: &Mailbox,
    character: &CharacterView,
    tooltip: &mut ItemTooltip,
    viewport: (u32, u32),
) {
    let (id, anchor) = if let Some(id) = menu.hovered_icon {
        let count = mailbox.len();
        let Some(index) = mailbox.letters().iter().rev().position(|l| l.id == id) else {
            return;
        };
        let row = menu.row_rect(index, count, viewport);
        (id, menu.row_icon_rect(row, viewport))
    } else if menu.attachment_hovered
        && let Some(id) = menu.selected
    {
        (id, menu.attachment_rect(viewport))
    } else {
        return;
    };
    let Some(letter) = mailbox.get(id) else {
        return;
    };
    let stack = ItemStack::with_props(letter.item, letter.props);
    let context = ItemContext::loose(character, stack, ItemPlace::Mailed);
    tooltip.push(list, kit, stack, character.hero, &context, anchor, viewport);
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::snapshot::UiSnapshot;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::character::{MailSender, loot_pool};
    use engine_render_data::ui::UiCommand;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn character() -> CharacterView {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        UiSnapshot::build(&simulation).character
    }

    fn mailbox_of(count: usize) -> Mailbox {
        let mut mailbox = Mailbox::new();
        for index in 0..count {
            mailbox.send(
                loot_pool()[index].id,
                MailSender::DungeonChest,
                if index % 2 == 0 {
                    MailReason::BagFull
                } else {
                    MailReason::Unclaimed
                },
                1_000.0 + index as f64,
            );
        }
        mailbox
    }

    fn open() -> MailboxMenu {
        let mut menu = MailboxMenu::new();
        menu.open();
        menu
    }

    fn click(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer: (x, y),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        }
    }

    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } | UiCommand::Label { range, .. } => {
                    Some(list.text_of(*range).to_owned())
                }
                _ => None,
            })
            .collect()
    }

    fn render(menu: &MailboxMenu, mailbox: &Mailbox, input: &UiInput) -> UiDrawList {
        let mut list = UiDrawList::new();
        push_mailbox(
            &mut list,
            menu,
            mailbox,
            &character(),
            input.pointer,
            2_000.0,
            &mut ItemTooltip::new(),
            VIEWPORT,
        );
        list
    }

    fn centre(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    #[test]
    fn the_panel_and_everything_in_it_fit_1280_by_720_at_scale_one() {
        let viewport = (1280, 720);
        let menu = open();
        let screen = UiRect::new(0.0, 0.0, 1280.0, 720.0);
        let panel = menu.panel_rect(viewport);
        for rect in [
            panel,
            menu.list_rect(viewport),
            menu.take_all_rect(viewport),
            menu.detail_rect(viewport),
            menu.take_rect(viewport),
            menu.attachment_rect(viewport),
            menu.close_rect(viewport),
        ] {
            assert!(
                rect.x >= screen.x
                    && rect.y >= screen.y
                    && rect.right() <= screen.right()
                    && rect.bottom() <= screen.bottom(),
                "{rect:?} is off a 1280x720 screen"
            );
        }
        for rect in [
            menu.list_rect(viewport),
            menu.take_all_rect(viewport),
            menu.detail_rect(viewport),
            menu.take_rect(viewport),
            menu.attachment_rect(viewport),
        ] {
            assert!(
                rect.x >= panel.x
                    && rect.y >= panel.y
                    && rect.right() <= panel.right() + 0.5
                    && rect.bottom() <= panel.bottom() + 0.5,
                "{rect:?} is outside the panel {panel:?}"
            );
        }
    }

    #[test]
    fn a_large_interface_scale_shrinks_the_panel_to_fit_a_small_window() {
        let mut menu = open();
        menu.set_scale(UiScale::new(2.0));
        let panel = menu.panel_rect((1280, 720));
        assert!(panel.right() <= 1280.0 && panel.bottom() <= 720.0);
    }

    #[test]
    fn an_empty_mailbox_says_so() {
        let drawn = texts(&render(&open(), &Mailbox::new(), &UiInput::new()));
        assert!(drawn.contains(&"Your mailbox is empty.".to_owned()));
    }

    #[test]
    fn the_newest_letter_is_opened_first_and_listed_first() {
        let mut menu = open();
        let mailbox = mailbox_of(3);
        let newest = mailbox.letters().last().expect("a letter").id;
        let outcome = menu.update(&UiInput::new(), 0.0, &mailbox, &character(), VIEWPORT);
        assert_eq!(menu.selected(), Some(newest));
        assert_eq!(outcome, MailboxOutcome::MarkRead { id: newest });
        // Asked once, not every frame.
        let again = menu.update(&UiInput::new(), 0.0, &mailbox, &character(), VIEWPORT);
        assert_eq!(again, MailboxOutcome::Idle);
    }

    #[test]
    fn clicking_an_unread_row_selects_it_and_asks_to_mark_it_read() {
        let mut menu = open();
        let mailbox = mailbox_of(3);
        let character = character();
        menu.update(&UiInput::new(), 0.0, &mailbox, &character, VIEWPORT);
        // Row 2 is the oldest letter.
        let row = menu.row_rect(2, 3, VIEWPORT);
        let y = centre(row).1;
        // Off the icon, so the click is on the row.
        let outcome = menu.update(
            &click(row.right() - 10.0, y),
            0.0,
            &mailbox,
            &character,
            VIEWPORT,
        );
        let oldest = mailbox.letters()[0].id;
        assert_eq!(menu.selected(), Some(oldest));
        assert_eq!(outcome, MailboxOutcome::MarkRead { id: oldest });
    }

    #[test]
    fn take_asks_for_the_open_letter() {
        let mut menu = open();
        let mailbox = mailbox_of(2);
        let character = character();
        menu.update(&UiInput::new(), 0.0, &mailbox, &character, VIEWPORT);
        let (x, y) = centre(menu.take_rect(VIEWPORT));
        let outcome = menu.update(&click(x, y), 0.0, &mailbox, &character, VIEWPORT);
        let id = mailbox.letters().last().expect("a letter").id;
        assert_eq!(outcome, MailboxOutcome::Take { id });
        let (x, y) = centre(menu.take_all_rect(VIEWPORT));
        let outcome = menu.update(&click(x, y), 0.0, &mailbox, &character, VIEWPORT);
        assert_eq!(outcome, MailboxOutcome::TakeAll);
    }

    #[test]
    fn a_full_bag_is_answered_with_feedback_and_no_request() {
        let mut menu = open();
        let mailbox = mailbox_of(1);
        let mut character = character();
        let filler = ItemStack::one(loot_pool()[0].id);
        character.inventory = [Some(filler); engine_game::character::INVENTORY_SLOTS];
        menu.update(&UiInput::new(), 0.0, &mailbox, &character, VIEWPORT);
        let (x, y) = centre(menu.take_rect(VIEWPORT));
        let outcome = menu.update(&click(x, y), 0.0, &mailbox, &character, VIEWPORT);
        assert_eq!(outcome, MailboxOutcome::Idle);
        assert!(menu.has_notice());
        let drawn = texts(&render(&menu, &mailbox, &UiInput::new()));
        assert!(drawn.contains(&BAG_FULL_NOTICE.to_owned()));
        menu.advance(NOTICE_SECONDS + 0.1);
        assert!(!menu.has_notice());
    }

    #[test]
    fn many_letters_scroll() {
        let viewport = (1280, 720);
        let mut menu = open();
        let mailbox = mailbox_of(12);
        let character = character();
        menu.update(&UiInput::new(), 0.0, &mailbox, &character, viewport);
        let (x, y) = centre(menu.list_rect(viewport));
        let before = menu.row_rect(11, 12, viewport).y;
        menu.update(
            &UiInput {
                pointer: (x, y),
                ..UiInput::new()
            },
            -3.0,
            &mailbox,
            &character,
            viewport,
        );
        assert!(
            menu.row_rect(11, 12, viewport).y < before,
            "the list did not scroll"
        );
    }

    #[test]
    fn escape_closes_it() {
        let mut menu = open();
        let outcome = menu.update(
            &UiInput {
                cancel: true,
                ..UiInput::new()
            },
            0.0,
            &Mailbox::new(),
            &character(),
            VIEWPORT,
        );
        assert_eq!(outcome, MailboxOutcome::Close);
        assert!(!menu.is_open());
    }

    #[test]
    fn hovering_the_attachment_shows_the_item_tooltip_for_a_mailed_item() {
        let mut menu = open();
        let mailbox = mailbox_of(1);
        let character = character();
        menu.update(&UiInput::new(), 0.0, &mailbox, &character, VIEWPORT);
        let (x, y) = centre(menu.attachment_rect(VIEWPORT));
        let input = UiInput {
            pointer: (x, y),
            ..UiInput::new()
        };
        menu.update(&input, 0.0, &mailbox, &character, VIEWPORT);
        let drawn = texts(&render(&menu, &mailbox, &input));
        assert!(drawn.contains(&"In your mail".to_owned()), "{drawn:?}");
    }

    #[test]
    fn relative_times_read_naturally() {
        let phrase = |seconds: f64| {
            let mut out = String::new();
            write_relative_time(&mut out, 1_000_000.0, 1_000_000.0 - seconds);
            out
        };
        assert_eq!(phrase(5.0), "Just now");
        assert_eq!(phrase(120.0), "2 min ago");
        assert_eq!(phrase(3.0 * 3_600.0), "3 hr ago");
        assert_eq!(phrase(86_400.0), "1 day ago");
        assert_eq!(phrase(3.0 * 86_400.0), "3 days ago");
        assert_eq!(phrase(-50.0), "Just now");
    }
}
