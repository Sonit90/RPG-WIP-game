//! Recount: the combat-statistics window.
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │ Recount                       0:34   [x] │
//! │ [Current Fight|Overall]  [Damage|Healing]│
//! │ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ 1. Ash     12.3k  410/s │
//! │ ▓▓▓▓▓▓▓▓▓▓▓       2. Bo       8.1k  270/s │
//! │ ▓▓▓▓▓▓            3. Cy       3.0k  100/s │
//! └─────────────────────────────────────────┘
//!         click a player ─► abilities (damage, share, casts) and top targets
//! ```
//!
//! The window owns no statistics. Online they are the realm's (`instance_meter`, computed from
//! the server's own combat events); offline they come from the local `CombatMeter`. Either way
//! the application converts them into [`RecountRow`]s with [`RecountWindow::set_rows`], at most
//! a few times a second, and drawing reads those rows: nothing is allocated per frame beyond
//! what the kit already does (text is written into a scratch string the window keeps).
//!
//! It is a floating window, not a modal one: [`RecountWindow::update`] reports
//! [`RecountOutcome::Consumed`] while the pointer is over it so a click does not also land in
//! the world, and says nothing otherwise.

use engine_game::entity::EntityId;
use engine_game::hero::Hero;
use engine_game::telemetry::CombatMeter;
use engine_realm::meter::{AbilityStat, Summary, ability_name};
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};

use super::input::UiInput;
use super::kit::{self, Interaction, Kit, Segment, tokens};
use super::map_style::{color, radius, type_scale};
use crate::game_ui::write_integer;

/// Layout, in design pixels.
mod layout {
    pub const WIDTH: f32 = 380.0;
    pub const PADDING: f32 = 10.0;
    pub const HEADER: f32 = 32.0;
    pub const CONTROLS: f32 = 28.0;
    pub const SCOPE_WIDTH: f32 = 200.0;
    pub const MODE_WIDTH: f32 = 150.0;
    pub const CONTROL_GAP: f32 = 10.0;
    pub const ROW: f32 = 20.0;
    pub const ROW_GAP: f32 = 2.0;
    pub const SECTION: f32 = 20.0;
    pub const MARGIN: f32 = 16.0;
    /// Targets listed in the breakdown.
    pub const TARGETS: usize = 4;
    /// Abilities listed in the breakdown.
    pub const ABILITIES: usize = 8;
    pub const CLOSE: f32 = super::kit::CLOSE_EDGE;
}

/// Which scope the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecountTab {
    /// The current (or last finished) fight.
    #[default]
    Fight,
    /// The whole run.
    Overall,
}

/// What the bars measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecountMode {
    /// Damage done.
    #[default]
    Damage,
    /// Healing done.
    Healing,
}

/// One player's figures for one scope, ready to draw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecountScope {
    /// Total damage.
    pub damage: u64,
    /// Total healing.
    pub healing: u64,
    /// The scope's length in seconds: what the rates divide by.
    pub seconds: f32,
    /// Per ability.
    pub abilities: Vec<AbilityStat>,
    /// Top targets: display name and damage, most first.
    pub targets: Vec<(String, u64)>,
}

impl RecountScope {
    /// From the realm's summary, naming each target with `target_name`.
    #[must_use]
    pub fn from_summary(summary: &Summary, mut target_name: impl FnMut(u64) -> String) -> Self {
        Self {
            damage: summary.damage,
            healing: summary.healing,
            seconds: summary.seconds,
            abilities: summary.abilities.clone(),
            targets: summary
                .targets
                .iter()
                .map(|t| (target_name(t.target), t.damage))
                .collect(),
        }
    }

    fn value(&self, mode: RecountMode) -> u64 {
        match mode {
            RecountMode::Damage => self.damage,
            RecountMode::Healing => self.healing,
        }
    }

    fn rate(&self, mode: RecountMode) -> f32 {
        self.value(mode) as f32 / self.seconds.max(1.0)
    }
}

/// One player in the window.
#[derive(Debug, Clone, PartialEq)]
pub struct RecountRow {
    /// The account: what selection is keyed by.
    pub account: String,
    /// The character's name.
    pub name: String,
    /// Their class, for the bar's colour.
    pub hero: Hero,
    /// The whole run.
    pub overall: RecountScope,
    /// The current (or last finished) fight.
    pub fight: RecountScope,
}

impl RecountRow {
    /// The offline game's single player, from the local meter. The local meter has one
    /// session, so both tabs show it; its hits taken (the owner is in its target list) are not
    /// targets.
    #[must_use]
    pub fn from_local(
        account: &str,
        name: &str,
        hero: Hero,
        meter: &CombatMeter,
        mut target_name: impl FnMut(EntityId) -> String,
    ) -> Self {
        let mut ranked = Vec::new();
        meter.ranked_abilities(&mut ranked);
        let abilities: Vec<AbilityStat> = ranked
            .iter()
            .filter(|(id, _)| id.is_valid())
            .map(|(id, totals)| AbilityStat {
                ability: u16::try_from(id.slot()).unwrap_or(u16::MAX),
                damage: totals.damage.max(0.0).round() as u64,
                healing: totals.healing.max(0.0).round() as u64,
                hits: totals.hits,
                crits: 0,
                casts: totals.casts,
            })
            .collect();
        let mut hit = Vec::new();
        meter.ranked_targets(&mut hit);
        let targets: Vec<(String, u64)> = hit
            .iter()
            .filter(|target| target.entity != meter.owner())
            .take(layout::TARGETS)
            .map(|target| {
                (
                    target_name(target.entity),
                    target.damage.max(0.0).round() as u64,
                )
            })
            .collect();
        let scope = RecountScope {
            damage: meter.total_damage().max(0.0).round() as u64,
            healing: meter.total_healing().max(0.0).round() as u64,
            seconds: meter.combat_seconds() as f32,
            abilities,
            targets,
        };
        Self {
            account: account.to_owned(),
            name: name.to_owned(),
            hero,
            overall: scope.clone(),
            fight: scope,
        }
    }
}

/// The colour of a class's bars.
#[must_use]
pub fn class_color(hero: Hero) -> UiColor {
    match hero {
        Hero::FireMage => [0.86, 0.42, 0.20, 1.0],
        Hero::Warrior => [0.78, 0.70, 0.58, 1.0],
        Hero::Druid => [0.38, 0.70, 0.44, 1.0],
    }
}

/// What the window decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecountOutcome {
    /// The pointer is not on the window.
    Idle,
    /// The pointer is on the window: the world should not also see it.
    Consumed,
    /// The close button was pressed.
    Closed,
}

/// The Recount window: whether it is open, what it shows and what is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct RecountWindow {
    open: bool,
    scale: UiScale,
    tab: RecountTab,
    mode: RecountMode,
    selected: Option<String>,
    rows: Vec<RecountRow>,
    /// `rows` indices, best first for the current tab and mode.
    order: Vec<usize>,
    /// The selected row's `abilities` indices that count in the current mode, best first.
    ability_order: Vec<usize>,
    scratch: String,
}

impl Default for RecountWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl RecountWindow {
    /// Closed, showing the current fight's damage.
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
            tab: RecountTab::Fight,
            mode: RecountMode::Damage,
            selected: None,
            rows: Vec::new(),
            order: Vec::new(),
            ability_order: Vec::new(),
            scratch: String::with_capacity(64),
        }
    }

    /// Re-lays the window out at `scale`.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.scale = scale;
    }

    /// Whether it is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Closes it.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Opens or closes it.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// Opens it on the whole-run tab with no player selected: what a finished run shows.
    pub fn show_overall(&mut self) {
        self.open = true;
        self.tab = RecountTab::Overall;
        self.selected = None;
        self.resort();
    }

    /// The tab showing.
    #[must_use]
    pub fn tab(&self) -> RecountTab {
        self.tab
    }

    /// The mode showing.
    #[must_use]
    pub fn mode(&self) -> RecountMode {
        self.mode
    }

    /// The account whose breakdown is open.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The rows being shown.
    #[must_use]
    pub fn rows(&self) -> &[RecountRow] {
        &self.rows
    }

    /// Replaces the data. The selection survives if the player is still there.
    pub fn set_rows(&mut self, rows: Vec<RecountRow>) {
        self.rows = rows;
        self.resort();
    }

    fn scope<'a>(&self, row: &'a RecountRow) -> &'a RecountScope {
        match self.tab {
            RecountTab::Fight => &row.fight,
            RecountTab::Overall => &row.overall,
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let account = self.selected.as_deref()?;
        self.rows.iter().position(|row| row.account == account)
    }

    fn resort(&mut self) {
        let (tab, mode) = (self.tab, self.mode);
        let value = |row: &RecountRow| {
            match tab {
                RecountTab::Fight => &row.fight,
                RecountTab::Overall => &row.overall,
            }
            .value(mode)
        };
        self.order.clear();
        self.order.extend(0..self.rows.len());
        let rows = &self.rows;
        self.order.sort_by(|a, b| {
            value(&rows[*b])
                .cmp(&value(&rows[*a]))
                .then_with(|| rows[*a].name.cmp(&rows[*b].name))
        });

        self.ability_order.clear();
        if let Some(index) = self.selected_index() {
            let scope = self.scope(&self.rows[index]);
            let of = |a: &AbilityStat| match mode {
                RecountMode::Damage => a.damage,
                RecountMode::Healing => a.healing,
            };
            let mut order: Vec<usize> = (0..scope.abilities.len())
                .filter(|i| of(&scope.abilities[*i]) > 0)
                .collect();
            order.sort_by(|a, b| {
                of(&scope.abilities[*b])
                    .cmp(&of(&scope.abilities[*a]))
                    .then(
                        scope.abilities[*a]
                            .ability
                            .cmp(&scope.abilities[*b].ability),
                    )
            });
            order.truncate(layout::ABILITIES);
            self.ability_order = order;
        }
    }

    // ---- layout ----

    fn kit(&self) -> Kit {
        Kit::new(self.scale)
    }

    /// Rows in the player list, and in the breakdown's ability list.
    fn body_height(&self, kit: &Kit) -> f32 {
        let row = kit.px(layout::ROW) + kit.px(layout::ROW_GAP);
        match self.selected_index() {
            None => row * self.rows.len().max(1) as f32,
            Some(index) => {
                let scope = self.scope(&self.rows[index]);
                let mut height = row * (1 + self.ability_order.len().max(1)) as f32;
                if self.mode == RecountMode::Damage && !scope.targets.is_empty() {
                    height += kit.px(layout::SECTION)
                        + row * scope.targets.len().min(layout::TARGETS) as f32;
                }
                height
            }
        }
    }

    /// The window, docked to the right edge below the minimap.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit();
        let width = kit.px(layout::WIDTH);
        let height = kit.px(layout::HEADER)
            + kit.px(layout::CONTROLS)
            + kit.px(layout::PADDING) * 2.0
            + self.body_height(&kit);
        // Below the minimap, but never so low that its bottom leaves the screen.
        let top = (viewport.1 as f32 * 0.33).max(kit.px(260.0));
        let lowest = (viewport.1 as f32 - height - kit.px(layout::MARGIN)).max(0.0);
        UiRect::new(
            (viewport.0 as f32 - width - kit.px(layout::MARGIN)).max(0.0),
            top.min(lowest),
            width,
            height,
        )
    }

    fn close_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        let edge = kit.px(layout::CLOSE);
        UiRect::new(
            panel.right() - kit.px(layout::PADDING) - edge,
            panel.y + (kit.px(layout::HEADER) - edge) * 0.5 + kit.px(2.0),
            edge,
            edge,
        )
    }

    fn controls_top(&self, panel: UiRect) -> f32 {
        panel.y + self.kit().px(layout::HEADER)
    }

    fn scope_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        UiRect::new(
            panel.x + kit.px(layout::PADDING),
            self.controls_top(panel),
            kit.px(layout::SCOPE_WIDTH),
            kit.px(layout::CONTROLS),
        )
    }

    fn mode_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        let scope = self.scope_rect(panel);
        UiRect::new(
            scope.right() + kit.px(layout::CONTROL_GAP),
            scope.y,
            kit.px(layout::MODE_WIDTH),
            scope.height,
        )
    }

    fn body_top(&self, panel: UiRect) -> f32 {
        let kit = self.kit();
        self.controls_top(panel) + kit.px(layout::CONTROLS) + kit.px(layout::PADDING)
    }

    /// Row `slot` of the body: the nth player, or in a breakdown the header (0) and abilities.
    fn row_rect(&self, panel: UiRect, slot: usize) -> UiRect {
        let kit = self.kit();
        let pitch = kit.px(layout::ROW) + kit.px(layout::ROW_GAP);
        UiRect::new(
            panel.x + kit.px(layout::PADDING),
            self.body_top(panel) + pitch * slot as f32,
            panel.width - kit.px(layout::PADDING) * 2.0,
            kit.px(layout::ROW),
        )
    }

    /// Whether `pointer` is over the open window.
    #[must_use]
    pub fn wants_pointer(&self, pointer: (f32, f32), viewport: (u32, u32)) -> bool {
        self.open && self.panel_rect(viewport).contains(pointer.0, pointer.1)
    }

    /// Handles this frame's pointer.
    pub fn update(&mut self, input: &UiInput, viewport: (u32, u32)) -> RecountOutcome {
        if !self.open {
            return RecountOutcome::Idle;
        }
        let panel = self.panel_rect(viewport);
        let (x, y) = input.pointer;
        if !panel.contains(x, y) {
            return RecountOutcome::Idle;
        }
        if !input.pointer_pressed {
            return RecountOutcome::Consumed;
        }
        if self.close_rect(panel).contains(x, y) {
            self.open = false;
            return RecountOutcome::Closed;
        }
        let scope = self.scope_rect(panel);
        let mode = self.mode_rect(panel);
        let mut changed = false;
        if scope.contains(x, y) {
            let next = if x < scope.x + scope.width * 0.5 {
                RecountTab::Fight
            } else {
                RecountTab::Overall
            };
            changed = next != self.tab;
            self.tab = next;
        } else if mode.contains(x, y) {
            let next = if x < mode.x + mode.width * 0.5 {
                RecountMode::Damage
            } else {
                RecountMode::Healing
            };
            changed = next != self.mode;
            self.mode = next;
        } else if self.selected_index().is_some() {
            // In a breakdown the header row goes back to the list.
            if self.row_rect(panel, 0).contains(x, y) {
                self.selected = None;
                changed = true;
            }
        } else {
            for (slot, index) in self.order.iter().enumerate() {
                if self.row_rect(panel, slot).contains(x, y) {
                    self.selected = Some(self.rows[*index].account.clone());
                    changed = true;
                    break;
                }
            }
        }
        if changed {
            self.resort();
        }
        RecountOutcome::Consumed
    }
}

/// Writes `value` compactly: `950`, `12.3k`, `1.24M`.
pub fn write_compact(out: &mut String, value: u64) {
    use std::fmt::Write;
    let _ = if value >= 1_000_000 {
        write!(out, "{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        write!(out, "{:.1}k", value as f64 / 1_000.0)
    } else {
        write!(out, "{value}")
    };
}

/// A clock: `m:ss`.
fn write_clock(out: &mut String, seconds: f32) {
    let whole = seconds.max(0.0) as i64;
    write_integer(out, whole / 60);
    out.push(':');
    if whole % 60 < 10 {
        out.push('0');
    }
    write_integer(out, whole % 60);
}

/// A bar: a sunken track with a fill of `fraction` in `fill`.
fn bar(list: &mut UiDrawList, kit: &Kit, rect: UiRect, fraction: f32, fill: UiColor, hot: bool) {
    let corner = kit.px(radius::CHIP);
    list.panel(rect, corner, tokens::WELL, tokens::WELL);
    let width = (rect.width * fraction.clamp(0.0, 1.0)).max(if fraction > 0.0 { 2.0 } else { 0.0 });
    if width > 0.0 {
        list.panel(
            UiRect::new(rect.x, rect.y, width, rect.height),
            corner,
            color::alpha(fill, if hot { 0.72 } else { 0.55 }),
            color::alpha(fill, if hot { 0.60 } else { 0.42 }),
        );
    }
    if hot {
        list.panel_stroke(rect, corner, 1.0, color::BORDER_STRONG);
    }
}

/// Draws the window. Takes the pointer to light the row under it.
pub fn push_recount(
    list: &mut UiDrawList,
    window: &mut RecountWindow,
    pointer: (f32, f32),
    viewport: (u32, u32),
) {
    if !window.is_open() {
        return;
    }
    // Taken so the rest of the window can be read while text is written into it.
    let mut text = std::mem::take(&mut window.scratch);
    let kit = window.kit();
    let panel = window.panel_rect(viewport);
    kit::panel(list, &kit, panel, color::BORDER_STRONG);
    let padding = kit.px(layout::PADDING);
    let input = UiInput::new().with_pointer(pointer.0, pointer.1);

    // Header: title, how long the scope has run, the close button.
    let header_centre = panel.y + kit.px(layout::HEADER) * 0.5 + kit.px(2.0);
    kit.text(
        list,
        panel.x + padding,
        header_centre,
        "Recount",
        type_scale::HEADING,
        color::TEXT_TITLE,
        TextAlign::Left,
    );
    let seconds = window
        .rows
        .iter()
        .map(|row| window.scope(row).seconds)
        .fold(0.0, f32::max);
    let close = window.close_rect(panel);
    text.clear();
    write_clock(&mut text, seconds);
    kit.text(
        list,
        close.x - kit.px(8.0),
        header_centre,
        &text,
        type_scale::META,
        color::TEXT_SECONDARY,
        TextAlign::Right,
    );
    kit::close_button(list, &kit, close, &input);

    // Tabs and modes.
    let scope_rect = window.scope_rect(panel);
    let mode_rect = window.mode_rect(panel);
    let states = |rect: UiRect, active: usize| -> [Interaction; 2] {
        std::array::from_fn(|i| {
            Interaction::at(kit::segment_rect(rect, 2, i), &input).active(i == active)
        })
    };
    kit::segmented(
        list,
        &kit,
        scope_rect,
        &[
            Segment {
                label: "Current Fight",
                icon: None,
            },
            Segment {
                label: "Overall",
                icon: None,
            },
        ],
        &states(scope_rect, usize::from(window.tab == RecountTab::Overall)),
    );
    kit::segmented(
        list,
        &kit,
        mode_rect,
        &[
            Segment {
                label: "Damage",
                icon: None,
            },
            Segment {
                label: "Healing",
                icon: None,
            },
        ],
        &states(mode_rect, usize::from(window.mode == RecountMode::Healing)),
    );

    match window.selected_index() {
        None => push_players(list, window, &kit, panel, &input, &mut text),
        Some(index) => push_breakdown(list, window, &kit, panel, &input, index, &mut text),
    }
    if close.contains(pointer.0, pointer.1) {
        kit::hint(
            list,
            &kit,
            pointer,
            viewport,
            "Close",
            "Esc also closes this window.",
        );
    }
    window.scratch = text;
}

fn push_players(
    list: &mut UiDrawList,
    window: &RecountWindow,
    kit: &Kit,
    panel: UiRect,
    input: &UiInput,
    text: &mut String,
) {
    let mode = window.mode;
    let best = window
        .order
        .first()
        .map_or(0, |i| window.scope(&window.rows[*i]).value(mode));
    if window.rows.is_empty() || best == 0 && window.rows.len() == 1 {
        let rect = window.row_rect(panel, 0);
        kit.text(
            list,
            rect.x + kit.px(4.0),
            rect.y + rect.height * 0.5,
            "No combat yet",
            type_scale::UI,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
        return;
    }
    let total: u64 = window
        .rows
        .iter()
        .map(|row| window.scope(row).value(mode))
        .sum();
    for (slot, index) in window.order.iter().enumerate() {
        let row = &window.rows[*index];
        let scope = window.scope(row);
        let rect = window.row_rect(panel, slot);
        let hot = Interaction::at(rect, input).hovered;
        let value = scope.value(mode);
        bar(
            list,
            kit,
            rect,
            value as f32 / best.max(1) as f32,
            class_color(row.hero),
            hot,
        );
        text.clear();
        write_integer(text, slot as i64 + 1);
        text.push_str(". ");
        text.push_str(&row.name);
        kit.text(
            list,
            rect.x + kit.px(6.0),
            rect.y + rect.height * 0.5,
            text,
            type_scale::UI,
            color::TEXT_PRIMARY,
            TextAlign::Left,
        );
        text.clear();
        // Three columns, right to left: the share, the rate (what players compare), the total.
        let centre = rect.y + rect.height * 0.5;
        text.clear();
        write_integer(
            text,
            (value as f64 * 100.0 / total.max(1) as f64).round() as i64,
        );
        text.push('%');
        kit.text(
            list,
            rect.right() - kit.px(6.0),
            centre,
            text,
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Right,
        );
        text.clear();
        write_compact(text, scope.rate(mode).round() as u64);
        text.push_str("/s");
        kit.text(
            list,
            rect.right() - kit.px(48.0),
            centre,
            text,
            type_scale::META,
            color::TEXT_PRIMARY,
            TextAlign::Right,
        );
        text.clear();
        write_compact(text, value);
        kit.text(
            list,
            rect.right() - kit.px(108.0),
            centre,
            text,
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Right,
        );
    }
}

fn push_breakdown(
    list: &mut UiDrawList,
    window: &RecountWindow,
    kit: &Kit,
    panel: UiRect,
    input: &UiInput,
    index: usize,
    text: &mut String,
) {
    let mode = window.mode;
    let row = &window.rows[index];
    let scope = window.scope(row);
    let tint = class_color(row.hero);
    let total = scope.value(mode);

    // The header doubles as the way back.
    let head = window.row_rect(panel, 0);
    bar(
        list,
        kit,
        head,
        1.0,
        tint,
        Interaction::at(head, input).hovered,
    );
    text.clear();
    text.push_str("< ");
    text.push_str(&row.name);
    kit.text(
        list,
        head.x + kit.px(6.0),
        head.y + head.height * 0.5,
        text,
        type_scale::UI,
        color::TEXT_PRIMARY,
        TextAlign::Left,
    );
    text.clear();
    write_compact(text, total);
    text.push_str("  ");
    write_compact(text, scope.rate(mode).round() as u64);
    text.push_str("/s");
    kit.text(
        list,
        head.right() - kit.px(6.0),
        head.y + head.height * 0.5,
        text,
        type_scale::META,
        color::TEXT_PRIMARY,
        TextAlign::Right,
    );

    // Abilities.
    let of = |a: &AbilityStat| match mode {
        RecountMode::Damage => a.damage,
        RecountMode::Healing => a.healing,
    };
    let best = window
        .ability_order
        .first()
        .map_or(0, |i| of(&scope.abilities[*i]));
    if window.ability_order.is_empty() {
        let rect = window.row_rect(panel, 1);
        kit.text(
            list,
            rect.x + kit.px(4.0),
            rect.y + rect.height * 0.5,
            "Nothing recorded",
            type_scale::UI,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
    }
    for (slot, ability) in window.ability_order.iter().enumerate() {
        let stat = &scope.abilities[*ability];
        let rect = window.row_rect(panel, slot + 1);
        let value = of(stat);
        bar(
            list,
            kit,
            rect,
            value as f32 / best.max(1) as f32,
            tint,
            false,
        );
        kit.text(
            list,
            rect.x + kit.px(6.0),
            rect.y + rect.height * 0.5,
            ability_name(stat.ability),
            type_scale::UI,
            color::TEXT_PRIMARY,
            TextAlign::Left,
        );
        text.clear();
        write_compact(text, value);
        text.push_str("  ");
        write_integer(
            text,
            (value as f64 * 100.0 / total.max(1) as f64).round() as i64,
        );
        text.push_str("%  x");
        write_integer(text, i64::from(stat.casts));
        kit.text(
            list,
            rect.right() - kit.px(6.0),
            rect.y + rect.height * 0.5,
            text,
            type_scale::META,
            color::TEXT_PRIMARY,
            TextAlign::Right,
        );
    }

    // Top targets (damage only: healing has none).
    if mode == RecountMode::Damage && !scope.targets.is_empty() {
        let first = 1 + window.ability_order.len().max(1);
        let label = window.row_rect(panel, first);
        kit.text(
            list,
            label.x + kit.px(2.0),
            label.y + kit.px(layout::SECTION) * 0.5 - kit.px(4.0),
            "Targets",
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        let top = scope.targets.first().map_or(0, |t| t.1);
        for (n, (name, damage)) in scope.targets.iter().take(layout::TARGETS).enumerate() {
            let mut rect = window.row_rect(panel, first + n);
            rect.y += kit.px(layout::SECTION) - kit.px(layout::ROW_GAP);
            bar(
                list,
                kit,
                rect,
                *damage as f32 / top.max(1) as f32,
                color::EMBER,
                false,
            );
            kit.text(
                list,
                rect.x + kit.px(6.0),
                rect.y + rect.height * 0.5,
                name,
                type_scale::UI,
                color::TEXT_PRIMARY,
                TextAlign::Left,
            );
            text.clear();
            write_compact(text, *damage);
            text.push_str("  ");
            write_integer(
                text,
                (*damage as f64 * 100.0 / total.max(1) as f64).round() as i64,
            );
            text.push('%');
            kit.text(
                list,
                rect.right() - kit.px(6.0),
                rect.y + rect.height * 0.5,
                text,
                type_scale::META,
                color::TEXT_PRIMARY,
                TextAlign::Right,
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use engine_realm::meter::TargetStat;
    use engine_render_data::ui::UiCommand;

    use super::*;

    const VIEWPORT: (u32, u32) = (1280, 720);

    fn scope(damage: u64, healing: u64, seconds: f32) -> RecountScope {
        RecountScope {
            damage,
            healing,
            seconds,
            abilities: vec![
                AbilityStat {
                    ability: 0,
                    damage: damage * 3 / 4,
                    healing: 0,
                    hits: 10,
                    crits: 2,
                    casts: 7,
                },
                AbilityStat {
                    ability: 1,
                    damage: damage / 4,
                    healing,
                    hits: 4,
                    crits: 0,
                    casts: 3,
                },
            ],
            targets: vec![("Skeleton".to_owned(), damage)],
        }
    }

    fn row(account: &str, hero: Hero, damage: u64, healing: u64) -> RecountRow {
        RecountRow {
            account: account.into(),
            name: account.to_uppercase(),
            hero,
            overall: scope(damage * 2, healing * 2, 100.0),
            fight: scope(damage, healing, 20.0),
        }
    }

    fn window() -> RecountWindow {
        let mut window = RecountWindow::new();
        window.open();
        window.set_rows(vec![
            row("bo", Hero::Warrior, 800, 0),
            row("ash", Hero::FireMage, 4000, 0),
            row("cy", Hero::Druid, 100, 900),
        ]);
        window
    }

    fn click(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer: (x, y),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        }
    }

    fn centre(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
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

    fn draw(window: &mut RecountWindow) -> UiDrawList {
        let mut list = UiDrawList::new();
        push_recount(&mut list, window, (0.0, 0.0), VIEWPORT);
        list
    }

    #[test]
    fn a_closed_window_draws_nothing_and_ignores_the_pointer() {
        let mut window = window();
        window.close();
        assert!(draw(&mut window).is_empty());
        let panel = window.panel_rect(VIEWPORT);
        assert_eq!(
            window.update(&click(panel.x + 5.0, panel.y + 5.0), VIEWPORT),
            RecountOutcome::Idle
        );
    }

    #[test]
    fn players_are_listed_best_first_with_totals_and_rates() {
        let mut window = window();
        let order: Vec<&str> = window
            .order
            .iter()
            .map(|i| window.rows[*i].account.as_str())
            .collect();
        assert_eq!(order, ["ash", "bo", "cy"]);
        let words = texts(&draw(&mut window));
        assert!(words.contains(&"Recount".to_owned()));
        assert!(words.contains(&"1. ASH".to_owned()), "{words:?}");
        // 4000 damage over 20 s: 200 a second.
        for expected in ["4000", "200/s", "82%"] {
            assert!(words.iter().any(|t| t == expected), "{expected}: {words:?}");
        }
    }

    #[test]
    fn tabs_and_modes_change_what_is_ranked() {
        let mut window = window();
        let panel = window.panel_rect(VIEWPORT);
        let healing = kit::segment_rect(window.mode_rect(panel), 2, 1);
        let (x, y) = centre(healing);
        assert_eq!(
            window.update(&click(x, y), VIEWPORT),
            RecountOutcome::Consumed
        );
        assert_eq!(window.mode(), RecountMode::Healing);
        assert_eq!(window.rows[window.order[0]].account, "cy");

        let overall = kit::segment_rect(window.scope_rect(panel), 2, 1);
        let (x, y) = centre(overall);
        window.update(&click(x, y), VIEWPORT);
        assert_eq!(window.tab(), RecountTab::Overall);
        let words = texts(&draw(&mut window));
        // Overall is the whole run: 1800 healing over 100 s.
        assert!(words.iter().any(|t| t == "1800"), "{words:?}");
        assert!(words.iter().any(|t| t == "18/s"), "{words:?}");
        assert!(words.iter().any(|t| t == "100%"), "{words:?}");
    }

    #[test]
    fn clicking_a_player_opens_their_breakdown_and_the_header_goes_back() {
        let mut window = window();
        let panel = window.panel_rect(VIEWPORT);
        let (x, y) = centre(window.row_rect(panel, 0));
        window.update(&click(x, y), VIEWPORT);
        assert_eq!(window.selected(), Some("ash"));

        let list = draw(&mut window);
        let words = texts(&list);
        assert!(words.contains(&"< ASH".to_owned()), "{words:?}");
        assert!(words.contains(&"FIREBOLT".to_owned()), "{words:?}");
        // 3000 of 4000: 75 percent, seven casts.
        assert!(words.iter().any(|t| t == "3000  75%  x7"), "{words:?}");
        assert!(words.contains(&"Targets".to_owned()));
        assert!(words.contains(&"Skeleton".to_owned()));

        // The breakdown is taller than the list, and the header goes back.
        let panel = window.panel_rect(VIEWPORT);
        let (x, y) = centre(window.row_rect(panel, 0));
        window.update(&click(x, y), VIEWPORT);
        assert_eq!(window.selected(), None);
    }

    #[test]
    fn healing_breakdown_lists_healing_abilities_only() {
        let mut window = window();
        window.mode = RecountMode::Healing;
        window.resort();
        window.selected = Some("cy".into());
        window.resort();
        let words = texts(&draw(&mut window));
        assert!(words.contains(&"< CY".to_owned()));
        assert!(words.iter().any(|t| t == "900  100%  x3"), "{words:?}");
        assert!(
            !words.contains(&"Targets".to_owned()),
            "healing has no targets"
        );
    }

    #[test]
    fn the_close_button_closes_and_the_window_owns_the_pointer_over_it() {
        let mut window = window();
        let panel = window.panel_rect(VIEWPORT);
        assert!(window.wants_pointer((panel.x + 2.0, panel.y + 2.0), VIEWPORT));
        assert!(!window.wants_pointer((2.0, 2.0), VIEWPORT));
        let (x, y) = centre(window.close_rect(panel));
        assert_eq!(
            window.update(&click(x, y), VIEWPORT),
            RecountOutcome::Closed
        );
        assert!(!window.is_open());
    }

    #[test]
    fn the_selection_survives_new_data_and_drops_with_the_player() {
        let mut window = window();
        window.selected = Some("bo".into());
        window.set_rows(vec![
            row("bo", Hero::Warrior, 5000, 0),
            row("ash", Hero::FireMage, 1, 0),
        ]);
        assert_eq!(window.selected(), Some("bo"));
        window.set_rows(vec![row("ash", Hero::FireMage, 1, 0)]);
        assert_eq!(window.selected_index(), None, "gone players show the list");
        assert!(!texts(&draw(&mut window)).is_empty());
    }

    #[test]
    fn an_empty_window_says_so_and_fits_the_screen() {
        let mut window = RecountWindow::new();
        window.open();
        let words = texts(&draw(&mut window));
        assert!(words.contains(&"No combat yet".to_owned()));
        // The tallest case (a breakdown with every list full) still fits 1280x720.
        let mut full = RecountWindow::new();
        full.open();
        let mut r = row("ash", Hero::FireMage, 4000, 0);
        r.fight.abilities = (0..10)
            .map(|i| AbilityStat {
                ability: i,
                damage: 100 + u64::from(i),
                ..AbilityStat::default()
            })
            .collect();
        r.fight.targets = (0..8).map(|i| (format!("T{i}"), 10)).collect();
        full.set_rows(vec![r]);
        full.selected = Some("ash".into());
        full.resort();
        assert!(full.panel_rect(VIEWPORT).bottom() <= VIEWPORT.1 as f32);
    }

    #[test]
    fn the_local_meter_feeds_the_same_rows() {
        use engine_game::ability::AbilityId;
        use engine_game::combat::DamageType;
        use engine_game::event::{CombatEvent, DamageEvent};

        let player = EntityId::new(1, 1);
        let dummy = EntityId::new(2, 1);
        let mut meter = CombatMeter::new();
        meter.set_owner(player);
        let hit = |target, amount| {
            CombatEvent::Damage(DamageEvent {
                source: player,
                target,
                amount,
                overkill: 0.0,
                ability: AbilityId::from_slot(0).expect("slot zero"),
                damage_type: DamageType::Fire,
                killed: false,
                critical: false,
                periodic: false,
                position: engine_core::math::Vec3::ZERO,
                tick: 0,
            })
        };
        meter.observe(&[hit(dummy, 300.0), hit(player, 50.0)], 1.0);
        meter.observe(&[hit(dummy, 100.0)], 4.0);
        let row = RecountRow::from_local("me", "You", Hero::FireMage, &meter, |_| "Dummy".into());
        assert_eq!(row.fight, row.overall);
        assert_eq!(row.fight.damage, 400);
        assert_eq!(row.fight.targets, vec![("Dummy".to_owned(), 400)]);
        assert_eq!(row.fight.abilities[0].ability, 0);
        assert!((row.fight.seconds - 3.0).abs() < 1e-6);
        let mut window = RecountWindow::new();
        window.open();
        window.set_rows(vec![row]);
        assert!(texts(&draw(&mut window)).iter().any(|t| t == "1. You"));
    }

    #[test]
    fn numbers_are_compact() {
        let mut out = String::new();
        for (value, expected) in [
            (950, "950"),
            (9_999, "9999"),
            (12_340, "12.3k"),
            (1_240_000, "1.24M"),
        ] {
            out.clear();
            write_compact(&mut out, value);
            assert_eq!(out, expected);
        }
    }

    #[test]
    fn scope_converts_a_realm_summary_naming_targets() {
        let summary = Summary {
            damage: 10,
            healing: 5,
            seconds: 2.0,
            targets: vec![TargetStat {
                target: 7,
                damage: 10,
            }],
            ..Summary::default()
        };
        let scope = RecountScope::from_summary(&summary, |key| format!("Monster {key}"));
        assert_eq!(scope.targets, vec![("Monster 7".to_owned(), 10)]);
        assert!((scope.rate(RecountMode::Damage) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn the_window_stays_on_screen_and_clear_of_the_others_at_every_size() {
        use crate::ui::layout_audit;
        for (viewport, scale) in layout_audit::sizes() {
            let mut window = RecountWindow::with_scale(scale);
            window.open();
            window.set_rows(
                (0..5)
                    .map(|i| row(&format!("p{i}"), Hero::Warrior, 1000 * (i + 1), 10))
                    .collect(),
            );
            let mut list = UiDrawList::new();
            push_recount(&mut list, &mut window, (0.0, 0.0), viewport);
            layout_audit::assert_sound(&list, viewport, &format!("recount {viewport:?}"));
        }
    }

    #[test]
    fn every_bar_reads_its_total_its_rate_and_its_share() {
        let mut window = window();
        let words = texts(&draw(&mut window));
        // 4000 of 4900 damage is 82 per cent; the bars are class coloured by `class_color`.
        assert!(words.iter().any(|t| t == "82%"), "{words:?}");
    }
}
