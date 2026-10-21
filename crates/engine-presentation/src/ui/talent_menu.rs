//! The talent menu: four tiers of nodes, bought with the mouse.
//!
//! ```text
//! talent button (top right) -> TalentMenu -> click a node -> TalentMenuOutcome::Purchase
//!                                         -> click RESET   -> TalentMenuOutcome::Reset
//! ```
//!
//! The menu owns no talent state. It reads the grid out of the frame's [`UiSnapshot`] - which
//! is the simulation's own answer about what is owned, what is affordable and how many points
//! are left - and it emits a purchase for the application to apply. Buying is a command like
//! any other, so the same click is reproducible in a replay.
//!
//! It is generic over the hero: the snapshot lists whichever hero's table is in force, in table
//! order, and the menu lays out whatever it finds. A hero whose table is empty shows a
//! placeholder (every hero has a tree today).
//!
//! While the character is **locked** (queued, in a party, in a dungeon) the menu still opens and
//! shows the build, with a banner saying why nothing can be changed, and emits nothing.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use engine_game::talent::TALENT_TIERS;

use super::input::UiInput;
use super::kit::{self, ButtonVariant, Interaction, Kit, tokens, tone};
use super::map_style::{color, radius, type_scale};
use crate::snapshot::{TalentView, UiSnapshot};

/// Layout of the menu, in pixels.
mod layout {
    /// Width of the whole panel.
    pub const PANEL_WIDTH: f32 = 1000.0;
    /// Height of the whole panel.
    pub const PANEL_HEIGHT: f32 = 720.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 24.0;
    /// Height of the heading.
    pub const HEADER: f32 = 76.0;
    /// Width of one talent node.
    pub const NODE_WIDTH: f32 = 210.0;
    /// Nodes across one row. A tier with more than this wraps onto another row, which is
    /// what keeps a tier that grows from running off the panel.
    pub const COLUMNS: usize = 4;
    /// Height of one talent node.
    pub const NODE_HEIGHT: f32 = 84.0;
    /// Gap between nodes.
    pub const NODE_GAP: f32 = 10.0;
    /// Width of the tier label column.
    pub const TIER_LABEL: f32 = 74.0;
    /// Width of the reset button.
    pub const RESET_WIDTH: f32 = 130.0;
    /// Height of the reset button.
    pub const RESET_HEIGHT: f32 = 30.0;
}

/// Height of the row of talent-point books on the Books page, in design pixels.
const BOOK_ROW: f32 = 58.0;
/// Height of an enhancement book's heading on the Books page.
const BOOK_HEAD: f32 = 46.0;
/// Height of one option card on the Books page.
const BOOK_CARD: f32 = 128.0;

/// Every talent-menu measurement, converted from its design size to physical pixels once.
///
/// The same arrangement as [`crate::game_ui::HudMetrics`]; see it for why the conversion
/// happens in one place rather than at each use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentMetrics {
    /// The scale these were derived with.
    pub scale: UiScale,
    /// `layout::PANEL_WIDTH` in physical pixels.
    pub panel_width: f32,
    /// `layout::PANEL_HEIGHT` in physical pixels.
    pub panel_height: f32,
    /// `layout::PADDING` in physical pixels.
    pub padding: f32,
    /// `layout::HEADER` in physical pixels.
    pub header: f32,
    /// `layout::NODE_WIDTH` in physical pixels.
    pub node_width: f32,
    /// `layout::NODE_HEIGHT` in physical pixels.
    pub node_height: f32,
    /// `layout::NODE_GAP` in physical pixels.
    pub node_gap: f32,
    /// `layout::TIER_LABEL` in physical pixels.
    pub tier_label: f32,
    /// `layout::RESET_WIDTH` in physical pixels.
    pub reset_width: f32,
    /// `layout::RESET_HEIGHT` in physical pixels.
    pub reset_height: f32,
}

impl TalentMetrics {
    /// The talent menu's measurements at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            panel_width: scale.px(layout::PANEL_WIDTH),
            panel_height: scale.px(layout::PANEL_HEIGHT),
            padding: scale.px(layout::PADDING),
            header: scale.px(layout::HEADER),
            node_width: scale.px(layout::NODE_WIDTH),
            node_height: scale.px(layout::NODE_HEIGHT),
            node_gap: scale.px(layout::NODE_GAP),
            tier_label: scale.px(layout::TIER_LABEL),
            reset_width: scale.px(layout::RESET_WIDTH),
            reset_height: scale.px(layout::RESET_HEIGHT),
        }
    }
}

impl Default for TalentMetrics {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

/// What the menu decided this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TalentMenuOutcome {
    /// Nothing happened.
    Idle,
    /// The player asked to buy the talent in this row of the hero's table.
    Purchase(usize),
    /// The player asked for every talent to be refunded.
    Reset,
    /// The player picked option `option` of enhancement book `book` (positions in
    /// [`UiSnapshot::books`]) on the Books page. Only emitted for a learned book, an option that is
    /// not already in force, and while the build is not locked.
    SetBookOption {
        /// Which enhancement book.
        book: usize,
        /// Which of its options.
        option: usize,
    },
    /// The menu was closed.
    Closed,
}

/// The talent menu's own state: whether it is open, and what the pointer is over.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TalentMenu {
    open: bool,
    hovered: Option<usize>,
    /// Whether the build cannot be changed right now (queued, in a party, in a dungeon).
    locked: bool,
    /// Whether the pointer is over the reset button.
    reset_hovered: bool,
    /// Whether the Books page is showing instead of the talent tiers.
    books_page: bool,
    /// Whether the pointer is over the page toggle.
    books_hovered: bool,
    /// The book option under the pointer on the Books page, `(book, option)`.
    option_hovered: Option<(usize, usize)>,
    /// Every measurement, already converted to physical pixels for the current UI scale.
    metrics: TalentMetrics,
}

impl TalentMenu {
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
            hovered: None,
            locked: false,
            reset_hovered: false,
            books_page: false,
            books_hovered: false,
            option_hovered: None,
            metrics: TalentMetrics::new(scale),
        }
    }

    /// Lays the menu out at a new scale from here on.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.metrics = TalentMetrics::new(scale);
    }

    /// The scale the menu is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.metrics.scale
    }

    /// The menu's measurements in physical pixels.
    ///
    /// Public so the painter positions text with the same numbers the node rectangles
    /// were built from.
    #[must_use]
    pub fn metrics(&self) -> &TalentMetrics {
        &self.metrics
    }

    /// Whether the menu is on screen.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Which talent the pointer is over, if any: its row in the hero's table.
    #[must_use]
    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// Tells the menu whether the build is locked. Set by the application every frame from
    /// what the realm says about the character; a locked menu emits no purchase and no reset.
    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    /// Whether the build is locked.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// The measurements for a window of `viewport` pixels: the configured scale, reduced until
    /// the whole panel fits, so a small window never cuts the tree off.
    #[must_use]
    pub fn metrics_for(&self, viewport: (u32, u32)) -> TalentMetrics {
        let scale = kit::fit_scale(
            self.metrics.scale,
            (layout::PANEL_WIDTH, layout::PANEL_HEIGHT),
            viewport,
        );
        if scale == self.metrics.scale {
            self.metrics
        } else {
            TalentMetrics::new(scale)
        }
    }

    /// Where the reset button sits: the header's right-hand side, under the point total.
    #[must_use]
    pub fn reset_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let m = self.metrics_for(viewport);
        UiRect::new(
            panel.right() - m.padding - m.reset_width,
            panel.y + m.padding + m.header * 0.55,
            m.reset_width,
            m.reset_height,
        )
    }

    /// Whether the Books page is showing.
    #[must_use]
    pub fn books_page(&self) -> bool {
        self.books_page
    }

    /// Where the Books/Talents page toggle sits: left of the reset button.
    #[must_use]
    pub fn books_rect(&self, viewport: (u32, u32)) -> UiRect {
        let reset = self.reset_rect(viewport);
        let m = self.metrics_for(viewport);
        let width = reset.width * 1.15;
        UiRect::new(
            reset.x - m.node_gap - width,
            reset.y,
            width,
            reset.height,
        )
    }

    /// Where option `option` of enhancement book `book` sits on the Books page: three columns,
    /// one per book, under a row of talent-point books.
    #[must_use]
    pub fn book_option_rect(&self, book: usize, option: usize, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let m = self.metrics_for(viewport);
        let inner = panel.width - m.padding * 2.0;
        let column = (inner - m.node_gap * 2.0) / 3.0;
        let top = panel.y + m.padding + m.header + m.scale.px(BOOK_ROW + 18.0) + m.scale.px(BOOK_HEAD);
        let card = m.scale.px(BOOK_CARD);
        UiRect::new(
            panel.x + m.padding + book as f32 * (column + m.node_gap),
            top + option as f32 * (card + m.node_gap),
            column,
            card,
        )
    }

    /// Where talent-point book `index` sits on the Books page.
    #[must_use]
    pub fn talent_book_rect(&self, index: usize, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let m = self.metrics_for(viewport);
        let inner = panel.width - m.padding * 2.0;
        let column = (inner - m.node_gap * 2.0) / 3.0;
        UiRect::new(
            panel.x + m.padding + index as f32 * (column + m.node_gap),
            panel.y + m.padding + m.header,
            column,
            m.scale.px(BOOK_ROW),
        )
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
        self.hovered = None;
        self.option_hovered = None;
    }

    /// Closes it.
    pub fn close(&mut self) {
        self.open = false;
        self.hovered = None;
    }

    /// Opens it if it is closed, closes it if it is open.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Reads the frame's pointer and reports what the player asked for.
    pub fn update(
        &mut self,
        input: &UiInput,
        snapshot: &UiSnapshot,
        viewport: (u32, u32),
    ) -> TalentMenuOutcome {
        if !self.open {
            return TalentMenuOutcome::Idle;
        }
        if input.cancel {
            self.close();
            return TalentMenuOutcome::Closed;
        }

        self.hovered = None;
        self.option_hovered = None;
        let mut clicked = None;
        let mut option_clicked = None;
        // The page toggle: only when the hero has books to show.
        let toggle = self.books_rect(viewport);
        self.books_hovered = !snapshot.books.is_empty()
            && toggle.contains(input.pointer.0, input.pointer.1);
        if self.books_hovered && input.pointer_pressed {
            self.books_page = !self.books_page;
            return TalentMenuOutcome::Idle;
        }
        if snapshot.books.is_empty() {
            self.books_page = false;
        }
        if self.books_page {
            for (book_index, book) in snapshot.books.enhancement.iter().enumerate() {
                for option_index in 0..book.options.len() {
                    let rect = self.book_option_rect(book_index, option_index, viewport);
                    if rect.contains(input.pointer.0, input.pointer.1) {
                        self.option_hovered = Some((book_index, option_index));
                        if input.pointer_pressed {
                            option_clicked = Some((book_index, option_index));
                        }
                    }
                }
            }
        } else {
            for (index, talent) in snapshot.talents.iter().enumerate() {
                let rect = self.node_rect(index, snapshot, viewport);
                if rect.contains(input.pointer.0, input.pointer.1) {
                    self.hovered = Some(talent.index);
                    if input.pointer_pressed {
                        clicked = Some(talent.index);
                    }
                }
            }
        }

        // Reset: refunds everything, so only offered when there is something to refund.
        let reset = self.reset_rect(viewport);
        self.reset_hovered = reset.contains(input.pointer.0, input.pointer.1);
        if self.reset_hovered
            && input.pointer_pressed
            && !self.locked
            && snapshot.talent_points < snapshot.talent_points_total
        {
            return TalentMenuOutcome::Reset;
        }

        // The close button, and clicking outside the panel.
        let panel = self.panel_rect(viewport);
        if input.pointer_pressed && !panel.contains(input.pointer.0, input.pointer.1) {
            self.close();
            return TalentMenuOutcome::Closed;
        }

        if let Some((book, option)) = option_clicked
            && !self.locked
            && let Some(view) = snapshot.books.enhancement.get(book)
            && view.learned
            && view.options.get(option).is_some_and(|candidate| !candidate.active)
        {
            return TalentMenuOutcome::SetBookOption { book, option };
        }
        match clicked {
            Some(index) if !self.locked => TalentMenuOutcome::Purchase(index),
            _ => TalentMenuOutcome::Idle,
        }
    }

    /// The panel the menu draws in.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let m = self.metrics_for(viewport);
        UiRect::new(
            ((width - m.panel_width) * 0.5).max(0.0),
            ((height - m.panel_height) * 0.5).max(0.0),
            m.panel_width,
            m.panel_height,
        )
    }

    /// Where the node for `index` sits.
    ///
    /// Laid out by tier: one row per tier, nodes across it in table order. The grid is
    /// derived from the talent data, so adding a talent moves nothing by hand.
    #[must_use]
    pub fn node_rect(&self, index: usize, snapshot: &UiSnapshot, viewport: (u32, u32)) -> UiRect {
        let Some(talent) = snapshot.talents.get(index) else {
            return UiRect::new(0.0, 0.0, 0.0, 0.0);
        };
        let panel = self.panel_rect(viewport);
        // Where this node sits inside its own tier, and how many rows the tiers above it
        // needed. Both are counted from the data, so a tier that grows simply wraps.
        let within_tier = snapshot
            .talents
            .iter()
            .take(index)
            .filter(|other| other.tier == talent.tier)
            .count();
        let rows_above: usize = (1..talent.tier).map(|tier| tier_rows(snapshot, tier)).sum();

        let column = within_tier % layout::COLUMNS;
        let row = rows_above + within_tier / layout::COLUMNS;

        let m = self.metrics_for(viewport);
        let x = panel.x + m.padding + m.tier_label + column as f32 * (m.node_width + m.node_gap);
        let y = panel.y + m.padding + m.header + row as f32 * (m.node_height + m.node_gap);
        UiRect::new(x, y, m.node_width, m.node_height)
    }
}

/// How many rows a tier needs at the menu's column count.
fn tier_rows(snapshot: &UiSnapshot, tier: u8) -> usize {
    let count = snapshot
        .talents
        .iter()
        .filter(|talent| talent.tier == tier)
        .count();
    count.div_ceil(layout::COLUMNS).max(1)
}

/// How the talent menu is coloured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentTheme {
    /// The wash over the game behind the menu.
    pub scrim: UiColor,
    /// The panel.
    pub panel: UiColor,
    /// The panel's border.
    pub border: UiColor,
    /// A talent that is owned.
    pub purchased: UiColor,
    /// A talent that can be bought now.
    pub affordable: UiColor,
    /// A talent that cannot be afforded.
    pub locked: UiColor,
    /// The outline on the node under the pointer.
    pub hover: UiColor,
    /// Ordinary text.
    pub text: UiColor,
    /// Dimmed text, for a locked node.
    pub text_dim: UiColor,
    /// The heading.
    pub title: UiColor,
}

impl Default for TalentTheme {
    fn default() -> Self {
        Self {
            scrim: [0.02, 0.03, 0.05, 0.78],
            panel: [0.08, 0.09, 0.13, 0.98],
            border: [0.42, 0.50, 0.64, 1.0],
            purchased: [0.20, 0.42, 0.26, 1.0],
            affordable: [0.18, 0.24, 0.34, 1.0],
            locked: [0.11, 0.11, 0.13, 1.0],
            hover: [0.98, 0.84, 0.35, 1.0],
            text: [0.92, 0.94, 0.98, 1.0],
            text_dim: [0.48, 0.50, 0.55, 1.0],
            title: [0.98, 0.78, 0.35, 1.0],
        }
    }
}

/// Draws the menu, if it is open.
///
/// `scratch` is the caller's, reused: the menu prints a cost and a point total every frame it
/// is up, and formatting them would otherwise allocate.
///
/// Drawn from the interface kit like the character sheet: the points available are the
/// heading's loudest number, the tiers are separated by rules, and a node is one of three
/// things at a glance - learned (green), learnable (bronze edge, gold cost) or out of reach
/// (dim, red cost). While the build is locked a banner says why, at the top where it is read.
pub fn push_talent_menu(
    list: &mut UiDrawList,
    menu: &TalentMenu,
    snapshot: &UiSnapshot,
    theme: &TalentTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    if !menu.is_open() {
        return;
    }
    let metrics = menu.metrics_for(viewport);
    let kit = Kit::new(metrics.scale);
    let panel = menu.panel_rect(viewport);
    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        theme.scrim,
    );
    kit::panel(list, &kit, panel, color::BORDER_STRONG);

    scratch.clear();
    scratch.push_str(snapshot.talent_hero.name());
    scratch.push_str(" TALENTS");
    kit.text(
        list,
        panel.x + metrics.padding,
        panel.y + metrics.padding + kit.px(14.0),
        scratch,
        type_scale::TITLE,
        color::TEXT_TITLE,
        TextAlign::Left,
    );

    // The points: gold while there are some to spend, muted when there are none.
    scratch.clear();
    scratch.push_str("POINTS ");
    push_integer(scratch, snapshot.talent_points);
    scratch.push_str(" / ");
    push_integer(scratch, snapshot.talent_points_total);
    kit.text(
        list,
        panel.right() - metrics.padding,
        panel.y + metrics.padding + kit.px(14.0),
        scratch,
        type_scale::TITLE,
        if snapshot.talent_points > 0 {
            tone::CURRENCY
        } else {
            color::TEXT_MUTED
        },
        TextAlign::Right,
    );

    // The reset button, dimmed when there is nothing to refund or the build is locked.
    let reset = menu.reset_rect(viewport);
    let can_reset = !menu.is_locked() && snapshot.talent_points < snapshot.talent_points_total;
    kit::button(
        list,
        &kit,
        reset,
        "Reset",
        None,
        ButtonVariant::Secondary,
        Interaction {
            hovered: menu.reset_hovered,
            ..Interaction::default()
        }
        .disabled(!can_reset),
    );

    let toggle = menu.books_rect(viewport);
    let has_books = !snapshot.books.is_empty();
    if has_books {
        kit::button(
            list,
            &kit,
            toggle,
            if menu.books_page() { "Talents" } else { "Books" },
            None,
            ButtonVariant::Secondary,
            Interaction {
                hovered: menu.books_hovered,
                ..Interaction::default()
            },
        );
    }

    if menu.is_locked() {
        let banner = UiRect::new(
            panel.x + metrics.padding,
            reset.y,
            ((if has_books { toggle.x } else { reset.x }) - panel.x - metrics.padding * 2.0)
                .max(0.0),
            reset.height,
        );
        kit::banner(
            list,
            &kit,
            banner,
            "LOCKED FOR THIS RUN: talents cannot change while you are queued, in a party or in a dungeon.",
            tone::WARN,
        );
    }

    if menu.books_page() {
        push_books_page(list, &kit, menu, snapshot, viewport, scratch);
        kit.text(
            list,
            panel.x + metrics.padding,
            panel.bottom() - metrics.padding - kit.px(4.0),
            if menu.is_locked() {
                "Esc closes."
            } else {
                "Click an option to put it in force. You can switch at the hub. Esc closes."
            },
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
        return;
    }

    if snapshot.talents.is_empty() {
        kit.text(
            list,
            panel.x + panel.width * 0.5,
            panel.y + panel.height * 0.45,
            "NO TALENTS YET. THIS HERO'S TREE IS COMING.",
            type_scale::HEADING,
            color::TEXT_MUTED,
            TextAlign::Center,
        );
        return;
    }

    // Tier labels down the left-hand side, and a rule above every tier but the first.
    for tier in 1..=TALENT_TIERS {
        let rows_above: usize = (1..tier).map(|above| tier_rows(snapshot, above)).sum();
        let top = panel.y
            + metrics.padding
            + metrics.header
            + rows_above as f32 * (metrics.node_height + metrics.node_gap);
        scratch.clear();
        scratch.push_str("TIER ");
        push_integer(scratch, u32::from(tier));
        kit.text(
            list,
            panel.x + metrics.padding,
            top + metrics.node_height * 0.5,
            scratch,
            type_scale::LABEL,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        if tier > 1 {
            list.rect(
                UiRect::new(
                    panel.x + metrics.padding,
                    (top - metrics.node_gap * 0.5).round(),
                    panel.width - metrics.padding * 2.0,
                    1.0,
                ),
                color::BORDER,
            );
        }
    }

    for (index, talent) in snapshot.talents.iter().enumerate() {
        let rect = menu.node_rect(index, snapshot, viewport);
        push_node(
            list,
            &kit,
            rect,
            talent,
            menu.hovered() == Some(talent.index),
            menu.is_locked(),
            scratch,
        );
    }

    kit.text(
        list,
        panel.x + metrics.padding,
        panel.bottom() - metrics.padding - kit.px(4.0),
        if menu.is_locked() {
            "Esc closes."
        } else {
            "Click a talent to learn it. Reset refunds everything. Esc closes."
        },
        type_scale::META,
        color::TEXT_MUTED,
        TextAlign::Left,
    );
}

/// The Books page: the talent-point books across the top (each worth one point once read), then one
/// column per enhancement book with its three options. The option in force is outlined in green;
/// a book that is not learned is dim and says what it takes.
fn push_books_page(
    list: &mut UiDrawList,
    kit: &Kit,
    menu: &TalentMenu,
    snapshot: &UiSnapshot,
    viewport: (u32, u32),
    scratch: &mut String,
) {
    let corner = kit.px(radius::CARD);
    let pad = kit.px(8.0);
    for (index, book) in snapshot.books.talent.iter().enumerate().take(3) {
        let rect = menu.talent_book_rect(index, viewport);
        let (top, bottom, edge) = if book.learned {
            (
                color::alpha(tone::GOOD, 0.26),
                color::alpha(tone::GOOD, 0.12),
                color::alpha(tone::GOOD, 0.80),
            )
        } else {
            (color::SURFACE_1, color::SURFACE_1, color::BORDER)
        };
        list.panel(rect, corner, top, bottom);
        list.panel_stroke(rect, corner, 1.0, edge);
        kit.text(
            list,
            rect.x + pad,
            rect.y + kit.px(17.0),
            &book.name,
            type_scale::HEADING,
            if book.learned {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
        scratch.clear();
        if book.learned {
            scratch.push_str("LEARNED: +1 TALENT POINT");
        } else {
            scratch.push_str("+1 TALENT POINT. BOSS DROP, KEY LEVEL ");
            push_integer(scratch, book.min_key_level);
            scratch.push('+');
        }
        kit.text(
            list,
            rect.x + pad,
            rect.y + kit.px(40.0),
            scratch,
            type_scale::META,
            if book.learned {
                tone::GOOD
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
    }

    for (book_index, book) in snapshot.books.enhancement.iter().enumerate().take(3) {
        let first = menu.book_option_rect(book_index, 0, viewport);
        let head_y = first.y - kit.px(BOOK_HEAD);
        kit.text(
            list,
            first.x,
            head_y + kit.px(14.0),
            &book.name,
            type_scale::HEADING,
            if book.learned {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
        scratch.clear();
        scratch.push_str(&book.ability);
        if book.learned {
            scratch.push_str(" - CHOOSE ONE");
        } else {
            scratch.push_str(" - NOT LEARNED. BOSS DROP, KEY LEVEL ");
            push_integer(scratch, book.min_key_level);
            scratch.push('+');
        }
        kit.text(
            list,
            first.x,
            head_y + kit.px(32.0),
            scratch,
            type_scale::META,
            if book.learned {
                tone::CURRENCY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
        for (option_index, option) in book.options.iter().enumerate() {
            let rect = menu.book_option_rect(book_index, option_index, viewport);
            let hovered = menu.option_hovered == Some((book_index, option_index));
            let clickable = book.learned && !menu.is_locked() && !option.active;
            let (top, bottom, edge) = if option.active {
                (
                    color::alpha(tone::GOOD, 0.26),
                    color::alpha(tone::GOOD, 0.12),
                    color::alpha(tone::GOOD, 0.80),
                )
            } else if clickable {
                (
                    if hovered {
                        color::SURFACE_3
                    } else {
                        color::SURFACE_2
                    },
                    color::SURFACE_1,
                    tokens::ACCENT,
                )
            } else {
                (color::SURFACE_1, color::SURFACE_1, color::BORDER)
            };
            list.panel(rect, corner, top, bottom);
            list.panel_stroke(rect, corner, kit.px(if option.active { 2.0 } else { 1.0 }), edge);
            let reachable = book.learned;
            kit.text(
                list,
                rect.x + pad,
                rect.y + kit.px(15.0),
                &option.name,
                type_scale::HEADING,
                if reachable {
                    color::TEXT_PRIMARY
                } else {
                    color::TEXT_MUTED
                },
                TextAlign::Left,
            );
            if option.active {
                kit.text(
                    list,
                    rect.right() - pad,
                    rect.y + kit.px(15.0),
                    "ACTIVE",
                    type_scale::LABEL,
                    tone::GOOD,
                    TextAlign::Right,
                );
            }
            let lines = kit
                .style
                .wrap(type_scale::META, &option.description, rect.width - pad * 2.0);
            for (line, text) in lines.iter().take(6).enumerate() {
                kit.text(
                    list,
                    rect.x + pad,
                    rect.y + kit.px(36.0) + kit.px(14.0) * line as f32,
                    text,
                    type_scale::META,
                    if reachable {
                        color::TEXT_SECONDARY
                    } else {
                        color::TEXT_MUTED
                    },
                    TextAlign::Left,
                );
            }
        }
    }
}

/// The icon a talent wears: the ability family it modifies, by the talent's key.
///
/// One icon per talent, reusing the ability and aura art rather than growing the atlas by a
/// tile for every row; a talent that reshapes Meteor wears Meteor.
#[must_use]
pub fn talent_icon(key: &str) -> Option<UiIcon> {
    Some(match key {
        // Fire Mage.
        "burning_embers" => UiIcon::Detonate,
        "extended_flame" => UiIcon::AuraBurning,
        "infernal_capacity" => UiIcon::AuraInferno,
        "rapid_combustion" => UiIcon::FlameCone,
        "molten_meteor" | "meteoric_impact" => UiIcon::Meteor,
        "furnace_mastery" | "dual_fire_mastery" => UiIcon::FireMastery,
        "spellfire_feedback" => UiIcon::FlameLance,
        "burning_precision" => UiIcon::Firebolt,
        "volatile_ignition" => UiIcon::AuraScorched,
        "chain_pyroblast" => UiIcon::Pyroblast,
        // Druid.
        "swift_wrath" => UiIcon::Wrath,
        "verdant_sunfire" => UiIcon::Sunfire,
        "deep_exposure" => UiIcon::AuraNaturesExposure,
        "living_seed" => UiIcon::Regrowth,
        "overgrowth" => UiIcon::Swiftmend,
        "astral_attunement" => UiIcon::AstralPower,
        "barkskin_instinct" => UiIcon::WildRush,
        "tranquil_mind" => UiIcon::Tranquility,
        // Warrior.
        "unbroken_momentum" => UiIcon::Valor,
        "shockwave" => UiIcon::WarStomp,
        "executioner" => UiIcon::ColossusStrike,
        "bloodied_valor" => UiIcon::PunctureBlood,
        "fervent_guard" => UiIcon::BattleFervor,
        "relentless_charge" => UiIcon::ShieldCharge,
        "bulwark_rhythm" => UiIcon::ShieldBlock,
        "unyielding" => UiIcon::LastStand,
        _ => return None,
    })
}

/// One talent node: name, cost or LEARNED, description, and what state it is in.
fn push_node(
    list: &mut UiDrawList,
    kit: &Kit,
    rect: UiRect,
    talent: &TalentView,
    hovered: bool,
    locked: bool,
    scratch: &mut String,
) {
    let corner = kit.px(radius::CARD);
    let learnable = talent.affordable && !locked;
    let (top, bottom, edge) = if talent.purchased {
        (
            color::alpha(tone::GOOD, 0.26),
            color::alpha(tone::GOOD, 0.12),
            color::alpha(tone::GOOD, 0.80),
        )
    } else if learnable {
        (
            if hovered {
                color::SURFACE_3
            } else {
                color::SURFACE_2
            },
            color::SURFACE_1,
            tokens::ACCENT,
        )
    } else {
        (color::SURFACE_1, color::SURFACE_1, color::BORDER)
    };
    list.panel(rect, corner, top, bottom);
    // The family's art, faint behind the words so no layout had to give way for it.
    if let Some(icon) = talent.icon {
        let size = kit.px(52.0);
        let alpha = if talent.purchased || learnable {
            0.42
        } else {
            0.2
        };
        list.icon(
            UiRect::new(
                rect.right() - size - kit.px(4.0),
                rect.y + (rect.height - size) * 0.5 + kit.px(6.0),
                size,
                size,
            ),
            icon,
            [1.0, 1.0, 1.0, alpha],
        );
    }
    let stroke = if hovered && !talent.purchased {
        color::TEXT_PRIMARY
    } else {
        edge
    };
    list.panel_stroke(
        rect,
        corner,
        kit.px(if learnable || hovered { 2.0 } else { 1.0 }),
        stroke,
    );

    let reachable = talent.purchased || learnable;
    let pad = kit.px(8.0);
    kit.text(
        list,
        rect.x + pad,
        rect.y + kit.px(15.0),
        talent.name,
        type_scale::HEADING,
        if reachable {
            color::TEXT_PRIMARY
        } else {
            color::TEXT_MUTED
        },
        TextAlign::Left,
    );

    scratch.clear();
    let (state_color, state_font) = if talent.purchased {
        scratch.push_str("LEARNED");
        (tone::GOOD, type_scale::LABEL)
    } else {
        scratch.push_str("COST ");
        push_integer(scratch, talent.cost);
        (
            if learnable {
                tone::CURRENCY
            } else {
                color::alpha(tone::DANGER, 0.85)
            },
            type_scale::LABEL,
        )
    };
    kit.text(
        list,
        rect.right() - pad,
        rect.y + kit.px(15.0),
        scratch,
        state_font,
        state_color,
        TextAlign::Right,
    );

    // The description, wrapped to the node's width.
    let lines = kit
        .style
        .wrap(type_scale::META, talent.description, rect.width - pad * 2.0);
    for (line, text) in lines.iter().take(DESCRIPTION_LINES).enumerate() {
        kit.text(
            list,
            rect.x + pad,
            rect.y + kit.px(34.0) + kit.px(13.5) * line as f32,
            text,
            type_scale::META,
            if reachable {
                color::TEXT_SECONDARY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
    }
}

/// Characters of a description that fit one line of a node.
#[cfg(test)]
const DESCRIPTION_COLUMNS: usize = 34;

/// Lines of a description a node has room for.
const DESCRIPTION_LINES: usize = 4;

/// Writes an integer into `out` without allocating.
fn push_integer(out: &mut String, value: u32) {
    use std::fmt::Write as _;
    let _ = write!(out, "{value}");
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::hero::Hero;
    use engine_game::talent::hero_talents;
    use engine_render_data::ui::UiCommand;

    const VIEWPORT: (u32, u32) = (1600, 900);

    fn snapshot() -> UiSnapshot {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        UiSnapshot::build(&simulation)
    }

    fn render(menu: &TalentMenu, snapshot: &UiSnapshot) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_talent_menu(
            &mut list,
            menu,
            snapshot,
            &TalentTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        list
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

    fn click(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new().with_pointer(x, y)
        }
    }

    fn with_books(learned: bool, active: Option<usize>) -> UiSnapshot {
        use crate::snapshot::{
            BookOptionView, BooksView, EnhancementBookView, TalentBookView,
        };
        let mut snapshot = snapshot();
        let option = |index: usize| BookOptionView {
            id: format!("o{index}"),
            name: format!("OPTION {index}"),
            description: "DOES A THING".to_owned(),
            active: active == Some(index),
        };
        snapshot.books = BooksView {
            talent: vec![TalentBookView {
                name: "CODEX".to_owned(),
                learned,
                min_key_level: 8,
            }],
            enhancement: vec![EnhancementBookView {
                id: "b".to_owned(),
                name: "MANUAL".to_owned(),
                ability: "WAR STOMP".to_owned(),
                learned,
                min_key_level: 6,
                options: (0..3).map(option).collect(),
            }],
        };
        snapshot
    }

    fn open_books(snapshot: &UiSnapshot) -> TalentMenu {
        let mut menu = TalentMenu::new();
        menu.open();
        let toggle = menu.books_rect(VIEWPORT);
        let outcome = menu.update(&click(toggle.x + 4.0, toggle.y + 4.0), snapshot, VIEWPORT);
        assert_eq!(outcome, TalentMenuOutcome::Idle);
        assert!(menu.books_page());
        menu
    }

    #[test]
    fn the_books_page_lists_books_highlights_the_active_option_and_switches_on_click() {
        let snapshot = with_books(true, Some(1));
        let mut menu = open_books(&snapshot);
        let shown = texts(&render(&menu, &snapshot));
        for wanted in ["CODEX", "MANUAL", "OPTION 0", "OPTION 1", "OPTION 2", "ACTIVE"] {
            assert!(shown.iter().any(|t| t == wanted), "{wanted} missing: {shown:?}");
        }
        // Clicking the active option does nothing; another one is a request.
        let active = menu.book_option_rect(0, 1, VIEWPORT);
        assert_eq!(
            menu.update(&click(active.x + 5.0, active.y + 5.0), &snapshot, VIEWPORT),
            TalentMenuOutcome::Idle
        );
        let other = menu.book_option_rect(0, 2, VIEWPORT);
        assert_eq!(
            menu.update(&click(other.x + 5.0, other.y + 5.0), &snapshot, VIEWPORT),
            TalentMenuOutcome::SetBookOption { book: 0, option: 2 }
        );
    }

    #[test]
    fn an_option_cannot_be_chosen_while_locked_or_before_the_book_is_learned() {
        let snapshot = with_books(true, None);
        let mut menu = open_books(&snapshot);
        menu.set_locked(true);
        let rect = menu.book_option_rect(0, 0, VIEWPORT);
        assert_eq!(
            menu.update(&click(rect.x + 5.0, rect.y + 5.0), &snapshot, VIEWPORT),
            TalentMenuOutcome::Idle
        );
        let shown = texts(&render(&menu, &snapshot));
        assert!(shown.iter().any(|t| t.starts_with("LOCKED FOR THIS RUN")), "{shown:?}");

        let unlearned = with_books(false, None);
        let mut menu = open_books(&unlearned);
        assert_eq!(
            menu.update(&click(rect.x + 5.0, rect.y + 5.0), &unlearned, VIEWPORT),
            TalentMenuOutcome::Idle
        );
        let shown = texts(&render(&menu, &unlearned));
        assert!(shown.iter().any(|t| t.contains("NOT LEARNED")), "{shown:?}");
    }

    #[test]
    fn without_books_there_is_no_page_to_open() {
        let snapshot = snapshot();
        let mut menu = TalentMenu::new();
        menu.open();
        let toggle = menu.books_rect(VIEWPORT);
        menu.update(&click(toggle.x + 4.0, toggle.y + 4.0), &snapshot, VIEWPORT);
        assert!(!menu.books_page());
    }

    #[test]
    fn a_closed_menu_draws_nothing_and_answers_nothing() {
        let menu = TalentMenu::new();
        assert!(!menu.is_open());
        assert!(render(&menu, &snapshot()).is_empty());
    }

    #[test]
    fn the_menu_lists_every_talent_with_its_tier_and_cost() {
        let mut menu = TalentMenu::new();
        menu.open();
        let snapshot = snapshot();
        let drawn = texts(&render(&menu, &snapshot));

        assert!(drawn.contains(&"FIREMAGE TALENTS".to_owned()), "{drawn:?}");
        for talent in hero_talents(Hero::FireMage).visible() {
            assert!(
                drawn.contains(&talent.name),
                "{} is missing from the menu",
                talent.name
            );
        }
        for tier in 1..=TALENT_TIERS {
            assert!(
                drawn.iter().any(|text| text == &format!("TIER {tier}")),
                "tier {tier} has no label"
            );
        }
        assert!(
            drawn.iter().any(|text| text.starts_with("POINTS")),
            "the point total is missing: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|text| text.starts_with("COST")),
            "no costs are shown: {drawn:?}"
        );
    }

    #[test]
    fn every_node_sits_inside_the_panel() {
        let snapshot = snapshot();
        let menu = TalentMenu::new();
        let panel = menu.panel_rect(VIEWPORT);
        for index in 0..snapshot.talents.len() {
            let rect = menu.node_rect(index, &snapshot, VIEWPORT);
            assert!(rect.x >= panel.x, "node {index} escaped left");
            assert!(
                rect.right() <= panel.right() + 0.01,
                "node {index} escaped right"
            );
            assert!(
                rect.bottom() <= panel.bottom() + 0.01,
                "node {index} escaped below"
            );
        }
    }

    #[test]
    fn clicking_a_node_asks_for_it_to_be_bought() {
        let mut menu = TalentMenu::new();
        menu.open();
        let snapshot = snapshot();

        let rect = menu.node_rect(0, &snapshot, VIEWPORT);
        let outcome = menu.update(&click(rect.x + 10.0, rect.y + 10.0), &snapshot, VIEWPORT);
        assert_eq!(
            outcome,
            TalentMenuOutcome::Purchase(snapshot.talents[0].index)
        );
        assert!(menu.is_open(), "buying closed the menu");
    }

    #[test]
    fn hovering_a_node_is_reported_so_it_can_be_highlighted() {
        let mut menu = TalentMenu::new();
        menu.open();
        let snapshot = snapshot();
        let rect = menu.node_rect(3, &snapshot, VIEWPORT);

        menu.update(
            &UiInput::new().with_pointer(rect.x + 5.0, rect.y + 5.0),
            &snapshot,
            VIEWPORT,
        );
        assert_eq!(menu.hovered(), Some(snapshot.talents[3].index));

        menu.update(&UiInput::new().with_pointer(1.0, 1.0), &snapshot, VIEWPORT);
        assert_eq!(menu.hovered(), None);
    }

    #[test]
    fn escape_and_a_click_outside_both_close_it() {
        let snapshot = snapshot();
        let mut menu = TalentMenu::new();
        menu.open();
        assert_eq!(
            menu.update(
                &UiInput {
                    cancel: true,
                    ..UiInput::new()
                },
                &snapshot,
                VIEWPORT
            ),
            TalentMenuOutcome::Closed
        );
        assert!(!menu.is_open());

        menu.open();
        assert_eq!(
            menu.update(&click(2.0, 2.0), &snapshot, VIEWPORT),
            TalentMenuOutcome::Closed
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn a_purchased_talent_is_drawn_differently_from_an_unpurchased_one() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let before = UiSnapshot::build(&simulation);

        let first = before.talents[0].index;
        simulation
            .purchase_talent_index(first)
            .expect("the starting points pay for the first talent");
        let after = UiSnapshot::build(&simulation);

        let mut menu = TalentMenu::new();
        menu.open();
        assert_ne!(
            render(&menu, &before).commands(),
            render(&menu, &after).commands(),
            "learning a talent changed nothing on screen"
        );
        assert!(texts(&render(&menu, &after)).contains(&"LEARNED".to_owned()));
    }

    #[test]
    fn toggling_opens_and_closes() {
        let mut menu = TalentMenu::new();
        menu.toggle();
        assert!(menu.is_open());
        menu.toggle();
        assert!(!menu.is_open());
    }

    #[test]
    fn a_locked_menu_shows_why_and_emits_nothing() {
        let mut menu = TalentMenu::new();
        menu.open();
        menu.set_locked(true);
        let snapshot = snapshot();
        assert!(
            texts(&render(&menu, &snapshot))
                .iter()
                .any(|text| text.starts_with("LOCKED")),
            "the banner is missing"
        );
        let rect = menu.node_rect(0, &snapshot, VIEWPORT);
        let outcome = menu.update(&click(rect.x + 10.0, rect.y + 10.0), &snapshot, VIEWPORT);
        assert_eq!(outcome, TalentMenuOutcome::Idle);
        menu.set_locked(false);
        let outcome = menu.update(&click(rect.x + 10.0, rect.y + 10.0), &snapshot, VIEWPORT);
        assert!(matches!(outcome, TalentMenuOutcome::Purchase(_)));
    }

    #[test]
    fn reset_is_offered_only_when_something_was_bought() {
        let mut menu = TalentMenu::new();
        menu.open();
        let fresh = snapshot();
        let reset = menu.reset_rect(VIEWPORT);
        let press = click(reset.x + 5.0, reset.y + 5.0);
        assert_eq!(
            menu.update(&press, &fresh, VIEWPORT),
            TalentMenuOutcome::Idle
        );

        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.purchase_talent_index(0).expect("buys");
        let bought = UiSnapshot::build(&simulation);
        assert_eq!(
            menu.update(&press, &bought, VIEWPORT),
            TalentMenuOutcome::Reset
        );
        menu.set_locked(true);
        assert_eq!(
            menu.update(&press, &bought, VIEWPORT),
            TalentMenuOutcome::Idle
        );
    }

    #[test]
    fn the_menu_draws_whichever_heros_tree_is_selected() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let mut menu = TalentMenu::new();
        menu.open();

        simulation.set_hero(Hero::Druid);
        simulation.step_once();
        let druid = UiSnapshot::build(&simulation);
        let drawn = texts(&render(&menu, &druid));
        assert!(drawn.contains(&"DRUID TALENTS".to_owned()), "{drawn:?}");
        for talent in hero_talents(Hero::Druid).visible() {
            assert!(drawn.contains(&talent.name), "{}", talent.name);
        }

        simulation.set_hero(Hero::Warrior);
        simulation.step_once();
        let warrior = UiSnapshot::build(&simulation);
        let drawn = texts(&render(&menu, &warrior));
        assert!(drawn.contains(&"WARRIOR TALENTS".to_owned()), "{drawn:?}");
        for talent in hero_talents(Hero::Warrior).visible() {
            assert!(drawn.contains(&talent.name), "{}", talent.name);
        }
        assert!(
            !drawn.iter().any(|text| text.starts_with("NO TALENTS YET")),
            "{drawn:?}"
        );
    }

    #[test]
    fn every_description_fits_its_node() {
        for hero in Hero::ALL {
            for talent in hero_talents(hero).visible() {
                assert!(
                    talent.description.len() <= DESCRIPTION_COLUMNS * DESCRIPTION_LINES - 26,
                    "{} is too long for a node: {}",
                    talent.name,
                    talent.description.len()
                );
            }
        }
    }

    #[test]
    fn the_menu_fits_every_window_and_says_which_nodes_are_out_of_reach() {
        use crate::ui::layout_audit;
        let snapshot = snapshot();
        for (viewport, scale) in layout_audit::sizes() {
            let mut menu = TalentMenu::with_scale(scale);
            menu.open();
            menu.set_locked(true);
            let mut list = UiDrawList::new();
            let mut scratch = String::new();
            push_talent_menu(
                &mut list,
                &menu,
                &snapshot,
                &TalentTheme::default(),
                &mut scratch,
                viewport,
            );
            layout_audit::assert_sound(&list, viewport, &format!("talents {viewport:?}"));
            let panel = menu.panel_rect(viewport);
            for index in 0..snapshot.talents.len() {
                let node = menu.node_rect(index, &snapshot, viewport);
                assert!(
                    node.right() <= panel.right() + 0.5 && node.bottom() <= panel.bottom() + 0.5,
                    "node {index} leaves the panel at {viewport:?} x{}",
                    scale.factor()
                );
            }
        }
    }

    #[test]
    fn every_talent_of_every_hero_has_an_icon() {
        for hero in engine_game::hero::Hero::ALL {
            for talent in engine_game::talent::hero_talents(hero).visible() {
                assert!(
                    talent_icon(&talent.key).is_some(),
                    "{} has no icon",
                    talent.key
                );
            }
        }
    }
}
