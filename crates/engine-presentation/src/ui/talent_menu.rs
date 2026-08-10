//! The talent menu: four tiers of nodes, bought with the mouse.
//!
//! ```text
//! talent button (top right) -> TalentMenu -> click a node -> TalentMenuOutcome::Purchase
//! ```
//!
//! The menu owns no talent state. It reads the grid out of the frame's [`UiSnapshot`] - which
//! is the simulation's own answer about what is owned, what is affordable and how many points
//! are left - and it emits a purchase for the application to apply. Buying is a command like
//! any other, so the same click is reproducible in a replay.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};

use engine_game::firemage::{TALENT_TIERS, TalentId};

use super::input::UiInput;
use crate::snapshot::{TalentView, UiSnapshot};

/// Layout of the menu, in pixels.
mod layout {
    /// Width of the whole panel.
    pub const PANEL_WIDTH: f32 = 1000.0;
    /// Height of the whole panel.
    pub const PANEL_HEIGHT: f32 = 830.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 24.0;
    /// Height of the heading.
    pub const HEADER: f32 = 56.0;
    /// Width of one talent node.
    pub const NODE_WIDTH: f32 = 210.0;
    /// Nodes across one row. A tier with more than this wraps onto another row, which is
    /// what keeps a tier that grows from running off the panel.
    pub const COLUMNS: usize = 4;
    /// Height of one talent node.
    pub const NODE_HEIGHT: f32 = 76.0;
    /// Gap between nodes.
    pub const NODE_GAP: f32 = 10.0;
    /// Width of the tier label column.
    pub const TIER_LABEL: f32 = 74.0;
}

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
    /// The player asked to buy a talent.
    Purchase(TalentId),
    /// The menu was closed.
    Closed,
}

/// The talent menu's own state: whether it is open, and what the pointer is over.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TalentMenu {
    open: bool,
    hovered: Option<TalentId>,
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

    /// Which talent the pointer is over, if any.
    #[must_use]
    pub fn hovered(&self) -> Option<TalentId> {
        self.hovered
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
        self.hovered = None;
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
        let mut clicked = None;
        for (index, talent) in snapshot.talents.iter().enumerate() {
            let rect = self.node_rect(index, snapshot, viewport);
            if rect.contains(input.pointer.0, input.pointer.1) {
                self.hovered = Some(talent.id);
                if input.pointer_pressed {
                    clicked = Some(talent.id);
                }
            }
        }

        // The close button, and clicking outside the panel.
        let panel = self.panel_rect(viewport);
        if input.pointer_pressed && !panel.contains(input.pointer.0, input.pointer.1) {
            self.close();
            return TalentMenuOutcome::Closed;
        }

        match clicked {
            Some(id) => TalentMenuOutcome::Purchase(id),
            None => TalentMenuOutcome::Idle,
        }
    }

    /// The panel the menu draws in.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        UiRect::new(
            ((width - self.metrics.panel_width) * 0.5).max(0.0),
            ((height - self.metrics.panel_height) * 0.5).max(0.0),
            self.metrics.panel_width,
            self.metrics.panel_height,
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

        let x = panel.x
            + self.metrics.padding
            + self.metrics.tier_label
            + column as f32 * (self.metrics.node_width + self.metrics.node_gap);
        let y = panel.y
            + self.metrics.padding
            + self.metrics.header
            + row as f32 * (self.metrics.node_height + self.metrics.node_gap);
        UiRect::new(x, y, self.metrics.node_width, self.metrics.node_height)
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
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let panel = menu.panel_rect(viewport);
    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        theme.scrim,
    );
    list.rect(panel, theme.panel);
    list.outline(panel, scale.px(2.0), theme.border);

    list.text_at(
        (panel.x + metrics.padding, panel.y + scale.px(16.0)),
        "TALENTS",
        theme.title,
        scale.text(3.0),
    );

    scratch.clear();
    scratch.push_str("POINTS ");
    push_integer(scratch, snapshot.talent_points);
    list.text_aligned(
        (panel.right() - metrics.padding, panel.y + scale.px(22.0)),
        scratch,
        theme.title,
        scale.text(2.0),
        TextAlign::Right,
    );

    // Tier labels down the left-hand side.
    for tier in 1..=TALENT_TIERS {
        let rows_above: usize = (1..tier).map(|above| tier_rows(snapshot, above)).sum();
        let y = panel.y
            + metrics.padding
            + metrics.header
            + rows_above as f32 * (metrics.node_height + metrics.node_gap)
            + metrics.node_height * 0.5
            - scale.px(7.0);
        scratch.clear();
        scratch.push_str("TIER ");
        push_integer(scratch, u32::from(tier));
        list.text_at(
            (panel.x + metrics.padding, y),
            scratch,
            theme.text_dim,
            scale.text(1.0),
        );
    }

    for (index, talent) in snapshot.talents.iter().enumerate() {
        let rect = menu.node_rect(index, snapshot, viewport);
        push_node(
            list,
            rect,
            talent,
            menu.hovered() == Some(talent.id),
            theme,
            scratch,
            scale,
        );
    }

    list.text_at(
        (panel.x + metrics.padding, panel.bottom() - scale.px(26.0)),
        "CLICK A TALENT TO LEARN IT. ESCAPE CLOSES.",
        theme.text_dim,
        scale.text(1.0),
    );
}

/// One talent node: name, cost, description, and what state it is in.
fn push_node(
    list: &mut UiDrawList,
    rect: UiRect,
    talent: &TalentView,
    hovered: bool,
    theme: &TalentTheme,
    scratch: &mut String,
    scale: UiScale,
) {
    let fill = if talent.purchased {
        theme.purchased
    } else if talent.affordable {
        theme.affordable
    } else {
        theme.locked
    };
    list.rect(rect, fill);
    if hovered {
        list.outline(rect, scale.px(2.0), theme.hover);
    } else {
        list.outline(rect, scale.px(1.0), theme.border);
    }

    let text = if talent.purchased || talent.affordable {
        theme.text
    } else {
        theme.text_dim
    };
    list.text_at(
        (rect.x + scale.px(6.0), rect.y + scale.px(6.0)),
        talent.name,
        text,
        scale.text(1.5),
    );

    scratch.clear();
    if talent.purchased {
        scratch.push_str("LEARNED");
    } else {
        scratch.push_str("COST ");
        push_integer(scratch, talent.cost);
    }
    list.text_aligned(
        (rect.right() - scale.px(6.0), rect.y + scale.px(6.0)),
        scratch,
        if talent.purchased { theme.hover } else { text },
        scale.text(1.0),
        TextAlign::Right,
    );

    // The description, wrapped to what fits the node.
    let mut line = 0;
    let mut start = 0;
    let characters: Vec<char> = talent.description.chars().collect();
    while start < characters.len() && line < 3 {
        let end = (start + DESCRIPTION_COLUMNS).min(characters.len());
        scratch.clear();
        scratch.extend(characters[start..end].iter());
        list.text_at(
            (
                rect.x + scale.px(6.0),
                rect.y + scale.px(26.0) + line as f32 * scale.px(11.0),
            ),
            scratch,
            theme.text_dim,
            1.0,
        );
        start = end;
        line += 1;
    }
}

/// Characters of a description that fit one line of a node.
const DESCRIPTION_COLUMNS: usize = 34;

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
    use engine_game::firemage::TALENTS;
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
                UiCommand::Text { range, .. } => Some(list.text_of(*range).to_owned()),
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

        assert!(drawn.contains(&"TALENTS".to_owned()), "{drawn:?}");
        for talent in &TALENTS {
            assert!(
                drawn.iter().any(|text| text == talent.name),
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
        assert_eq!(outcome, TalentMenuOutcome::Purchase(snapshot.talents[0].id));
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
        assert_eq!(menu.hovered(), Some(snapshot.talents[3].id));

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

        let first = before.talents[0].id;
        simulation
            .mage_mut()
            .purchase_talent(first)
            .expect("testing mode pays for it");
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
}
