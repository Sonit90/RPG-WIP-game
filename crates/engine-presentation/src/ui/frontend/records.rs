//! The record pages: a character's run **History** (with a detail view of one run) and the
//! **Leaderboards**.
//!
//! Both are plain data the application builds from what the persistence API answered when the
//! page was opened (or paged): nothing here asks the network, and nothing is fetched per frame.
//! The pages keep no state of their own; which row is selected, which board is shown and which
//! page are the application's, and a press leaves as a [`super::FrontEndAction`].

use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiColor, UiRect, UiScale};

use super::{
    BAD, ButtonKind, DIM, FAINT, GOLD, GOOD, Hit, Layout, Painter, SELECTED, TEXT, size,
};

/// Runs shown per History page.
pub const HISTORY_ROWS: usize = 6;
/// Rows shown per Leaderboards page.
pub const BOARD_ROWS: usize = 13;
/// The most party, modifier and loot lines the detail panel lists before "and N more".
const DETAIL_LINES: usize = 5;

/// How a run ended, as the pages label it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeTag {
    /// Completed inside the timer.
    Timed,
    /// Completed on a map with no clock (a community map that sets none).
    Untimed,
    /// Completed after the timer.
    Overtime,
    /// Wiped or failed.
    Failed,
    /// The party left.
    Abandoned,
}

impl OutcomeTag {
    /// The label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Timed => "TIMED",
            Self::Untimed => "UNTIMED",
            Self::Overtime => "OVERTIME",
            Self::Failed => "FAILED",
            Self::Abandoned => "ABANDONED",
        }
    }

    fn color(self) -> UiColor {
        match self {
            Self::Timed => GOOD,
            Self::Untimed => DIM,
            Self::Overtime => GOLD,
            Self::Failed => BAD,
            Self::Abandoned => DIM,
        }
    }
}

/// One run in the History list.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    /// The dungeon's name.
    pub dungeon: String,
    /// The key level.
    pub key_level: u32,
    /// How it ended.
    pub outcome: OutcomeTag,
    /// How long it took, as `mm:ss`.
    pub duration: String,
    /// When, as a date.
    pub when: String,
}

/// The detail of one run.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryDetail {
    /// "Graveyard Crypt +4".
    pub title: String,
    /// How it ended.
    pub outcome: OutcomeTag,
    /// Labelled figures: time, deaths, damage taken, healing, DPS.
    pub stats: Vec<(String, String)>,
    /// Who played.
    pub party: Vec<String>,
    /// The key's modifiers.
    pub modifiers: Vec<String>,
    /// What the character received.
    pub loot: Vec<String>,
}

/// The History page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistoryCard {
    /// The page's runs, newest first (at most [`HISTORY_ROWS`]).
    pub rows: Vec<HistoryRow>,
    /// Which row's detail is open.
    pub selected: Option<usize>,
    /// The open run.
    pub detail: Option<HistoryDetail>,
    /// A page is being fetched.
    pub loading: bool,
    /// An older page exists.
    pub can_older: bool,
    /// This is not the newest page.
    pub can_newer: bool,
}

/// One row of a leaderboard.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardRow {
    /// Position, from 1.
    pub rank: u32,
    /// The key level.
    pub key_level: u32,
    /// Timed or overtime.
    pub outcome: OutcomeTag,
    /// The time, as `mm:ss`.
    pub time: String,
    /// The party's names.
    pub party: String,
}

/// The Leaderboards page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoardCard {
    /// The dungeon being shown.
    pub dungeon: String,
    /// The key level shown (the fastest board ranks one level).
    pub key_level: u32,
    /// Whether the fastest board is up (otherwise the highest).
    pub fastest: bool,
    /// The season's name.
    pub season: String,
    /// The page's rows (at most [`BOARD_ROWS`]).
    pub rows: Vec<BoardRow>,
    /// A page is being fetched.
    pub loading: bool,
    /// The page number, from 1.
    pub page: u32,
    /// An earlier page exists.
    pub can_prev: bool,
    /// A later page exists.
    pub can_next: bool,
}

// --- layout -------------------------------------------------------------------------------

/// The list and the detail panels of the History page.
pub(super) fn history_panels(w: f32, h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let top = px(170.0);
    let height = h - top - px(130.0);
    let total = (w - px(120.0)).min(px(1200.0));
    let left = (w - total) * 0.5;
    (
        UiRect::new(left, top, total * 0.52, height),
        UiRect::new(
            left + total * 0.52 + px(24.0),
            top,
            total * 0.48 - px(24.0),
            height,
        ),
    )
}

/// The History list's `index`th row.
pub(super) fn history_row(list: UiRect, index: usize, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        list.x + px(12.0),
        list.y + px(56.0) + px(54.0) * index as f32,
        list.width - px(24.0),
        px(48.0),
    )
}

/// Back, Newer and Older, centred in the footer.
pub(super) fn history_footer(w: f32, h: f32, scale: UiScale) -> [UiRect; 3] {
    let px = |v: f32| scale.px(v);
    let y = h - px(96.0);
    let (width, gap) = (px(190.0), px(20.0));
    let x = w * 0.5 - (width * 3.0 + gap * 2.0) * 0.5;
    [0, 1, 2].map(|i| UiRect::new(x + (width + gap) * i as f32, y, width, px(50.0)))
}

/// The controls panel and the table panel of the Leaderboards page.
pub(super) fn board_panels(w: f32, h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let top = px(170.0);
    let height = h - top - px(130.0);
    let total = (w - px(120.0)).min(px(1200.0));
    let left = (w - total) * 0.5;
    (
        UiRect::new(left, top, total * 0.34, height),
        UiRect::new(
            left + total * 0.34 + px(24.0),
            top,
            total * 0.66 - px(24.0),
            height,
        ),
    )
}

/// A stepper row in the controls panel: `[<] value [>]`. Returns `(row, minus, plus)`.
pub(super) fn board_stepper(
    panel: UiRect,
    index: usize,
    scale: UiScale,
) -> (UiRect, UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let row = UiRect::new(
        panel.x + px(16.0),
        panel.y + px(78.0) + px(76.0) * index as f32,
        panel.width - px(32.0),
        px(size::COMPACT_HEIGHT),
    );
    let button = px(size::COMPACT_HEIGHT);
    (
        row,
        UiRect::new(row.x, row.y, button, row.height),
        UiRect::new(row.right() - button, row.y, button, row.height),
    )
}

/// The Fastest and Highest toggle. Returns `(fastest, highest)`.
pub(super) fn board_kinds(panel: UiRect, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let y = panel.y + px(78.0) + px(76.0) * 2.0;
    let width = (panel.width - px(32.0) - px(10.0)) * 0.5;
    (
        UiRect::new(panel.x + px(16.0), y, width, px(size::COMPACT_HEIGHT)),
        UiRect::new(
            panel.x + px(16.0) + width + px(10.0),
            y,
            width,
            px(size::COMPACT_HEIGHT),
        ),
    )
}

/// The table's `index`th row.
pub(super) fn board_row(table: UiRect, index: usize, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        table.x + px(12.0),
        table.y + px(52.0) + px(26.0) * index as f32,
        table.width - px(24.0),
        px(24.0),
    )
}

/// Back, Previous and Next, centred in the footer (the History footer's shape).
pub(super) fn board_footer(w: f32, h: f32, scale: UiScale) -> [UiRect; 3] {
    history_footer(w, h, scale)
}

// --- hit areas ----------------------------------------------------------------------------

pub(super) fn history_hits(layout: &mut Layout, card: &HistoryCard, w: f32, h: f32, s: UiScale) {
    let (list, detail) = history_panels(w, h, s);
    layout.add(Hit::Panel, detail);
    for index in 0..card.rows.len().min(HISTORY_ROWS) {
        layout.add(Hit::HistoryRow(index), history_row(list, index, s));
    }
    let [back, newer, older] = history_footer(w, h, s);
    layout.add(Hit::Back, back);
    if card.can_newer && !card.loading {
        layout.add(Hit::HistoryNewest, newer);
    }
    if card.can_older && !card.loading {
        layout.add(Hit::HistoryOlder, older);
    }
}

pub(super) fn board_hits(layout: &mut Layout, card: &BoardCard, w: f32, h: f32, s: UiScale) {
    let (controls, _) = board_panels(w, h, s);
    layout.add(Hit::Panel, controls);
    let (_, minus, plus) = board_stepper(controls, 0, s);
    layout.add(Hit::BoardDungeon(-1), minus);
    layout.add(Hit::BoardDungeon(1), plus);
    let (_, minus, plus) = board_stepper(controls, 1, s);
    layout.add(Hit::BoardKey(-1), minus);
    layout.add(Hit::BoardKey(1), plus);
    let (fastest, highest) = board_kinds(controls, s);
    layout.add(Hit::BoardKind(true), fastest);
    layout.add(Hit::BoardKind(false), highest);
    let [back, prev, next] = board_footer(w, h, s);
    layout.add(Hit::Back, back);
    if card.can_prev && !card.loading {
        layout.add(Hit::BoardPage(-1), prev);
    }
    if card.can_next && !card.loading {
        layout.add(Hit::BoardPage(1), next);
    }
}

// --- drawing ------------------------------------------------------------------------------

/// `text` cut to `max` characters, with an ellipsis.
fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// "name" lines cut to [`DETAIL_LINES`] with a closing count.
fn limited(lines: &[String]) -> Vec<String> {
    let mut shown: Vec<String> = lines.iter().take(DETAIL_LINES).cloned().collect();
    if lines.len() > DETAIL_LINES {
        shown.push(format!("and {} more", lines.len() - DETAIL_LINES));
    }
    shown
}

pub(super) fn draw_history(
    ctx: &mut Painter<'_>,
    card: &HistoryCard,
    busy: bool,
    w: f32,
    h: f32,
) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading("HISTORY", "Your recent runs. Select one for its details.", w);
    let (list, detail) = history_panels(w, h, scale);
    ctx.panel(list);
    ctx.panel(detail);
    ctx.text(
        (list.x + px(20.0), list.y + px(18.0)),
        "RUNS",
        GOLD,
        UiFont::SemiBold,
        17.0,
        TextAlign::Left,
    );
    if card.rows.is_empty() {
        let message = if card.loading || busy {
            "Loading..."
        } else {
            "No runs yet. Finish a dungeon and it will be listed here."
        };
        ctx.text(
            (list.x + px(20.0), list.y + px(64.0)),
            message,
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
    }
    for (index, row) in card.rows.iter().take(HISTORY_ROWS).enumerate() {
        let rect = history_row(list, index, scale);
        let selected = card.selected == Some(index);
        let hovered = ctx.is_hovered(&Hit::HistoryRow(index));
        let fill = if selected {
            ([0.20, 0.17, 0.10, 1.0], [0.13, 0.11, 0.07, 1.0])
        } else if hovered {
            ([0.16, 0.17, 0.22, 1.0], [0.11, 0.12, 0.16, 1.0])
        } else {
            ([0.09, 0.10, 0.13, 1.0], [0.07, 0.075, 0.10, 1.0])
        };
        ctx.list
            .panel(rect, px(size::BUTTON_CORNER), fill.0, fill.1);
        if selected {
            ctx.list
                .panel_stroke(rect, px(size::BUTTON_CORNER), px(1.5), SELECTED);
        }
        ctx.text(
            (rect.x + px(12.0), rect.y + px(6.0)),
            &fit(&format!("{} +{}", row.dungeon, row.key_level), 26),
            TEXT,
            UiFont::SemiBold,
            15.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.x + px(12.0), rect.y + px(27.0)),
            &row.when,
            FAINT,
            UiFont::Medium,
            12.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.right() - px(12.0), rect.y + px(6.0)),
            row.outcome.label(),
            row.outcome.color(),
            UiFont::SemiBold,
            13.0,
            TextAlign::Right,
        );
        ctx.text(
            (rect.right() - px(12.0), rect.y + px(27.0)),
            &row.duration,
            DIM,
            UiFont::Medium,
            12.0,
            TextAlign::Right,
        );
    }
    // The detail of the selected run.
    match &card.detail {
        Some(run) => {
            let x = detail.x + px(20.0);
            let mut y = detail.y + px(16.0);
            ctx.text(
                (x, y),
                &fit(&run.title, 30),
                GOLD,
                UiFont::SemiBold,
                18.0,
                TextAlign::Left,
            );
            ctx.text(
                (detail.right() - px(20.0), y + px(2.0)),
                run.outcome.label(),
                run.outcome.color(),
                UiFont::SemiBold,
                14.0,
                TextAlign::Right,
            );
            y += px(34.0);
            for (label, value) in &run.stats {
                ctx.text((x, y), label, DIM, UiFont::Medium, 13.0, TextAlign::Left);
                ctx.text(
                    (detail.right() - px(20.0), y),
                    value,
                    TEXT,
                    UiFont::SemiBold,
                    13.0,
                    TextAlign::Right,
                );
                y += px(19.0);
            }
            for (title, lines) in [
                ("PARTY", &run.party),
                ("MODIFIERS", &run.modifiers),
                ("LOOT", &run.loot),
            ] {
                if lines.is_empty() {
                    continue;
                }
                y += px(6.0);
                ctx.text((x, y), title, GOLD, UiFont::SemiBold, 12.0, TextAlign::Left);
                y += px(16.0);
                for line in limited(lines) {
                    ctx.text(
                        (x + px(6.0), y),
                        &fit(&line, 40),
                        TEXT,
                        UiFont::Medium,
                        12.0,
                        TextAlign::Left,
                    );
                    y += px(15.0);
                }
            }
        }
        None => ctx.text(
            (detail.x + detail.width * 0.5, detail.y + px(24.0)),
            "Select a run",
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Center,
        ),
    }
    let [back, newer, older] = history_footer(w, h, scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    ctx.button(
        newer,
        "Newest",
        ButtonKind::Secondary,
        &Hit::HistoryNewest,
        card.can_newer && !card.loading,
    );
    ctx.button(
        older,
        "Older",
        ButtonKind::Secondary,
        &Hit::HistoryOlder,
        card.can_older && !card.loading,
    );
}

fn stepper(ctx: &mut Painter<'_>, parts: (UiRect, UiRect, UiRect), label: &str, value: &str, hits: (Hit, Hit)) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let (row, minus, plus) = parts;
    ctx.text(
        (row.x, row.y - px(22.0)),
        label,
        DIM,
        UiFont::Medium,
        13.0,
        TextAlign::Left,
    );
    ctx.button(minus, "<", ButtonKind::Secondary, &hits.0, true);
    ctx.button(plus, ">", ButtonKind::Secondary, &hits.1, true);
    ctx.text(
        (row.x + row.width * 0.5, row.y + px(7.0)),
        &fit(value, 20),
        TEXT,
        UiFont::SemiBold,
        15.0,
        TextAlign::Center,
    );
}

pub(super) fn draw_boards(ctx: &mut Painter<'_>, card: &BoardCard, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading(
        "LEADERBOARDS",
        "The best completed runs. One row per run.",
        w,
    );
    let (controls, table) = board_panels(w, h, scale);
    ctx.panel(controls);
    ctx.panel(table);
    ctx.text(
        (controls.x + px(20.0), controls.y + px(18.0)),
        "BOARD",
        GOLD,
        UiFont::SemiBold,
        17.0,
        TextAlign::Left,
    );
    stepper(
        ctx,
        board_stepper(controls, 0, scale),
        "Dungeon",
        &card.dungeon,
        (Hit::BoardDungeon(-1), Hit::BoardDungeon(1)),
    );
    stepper(
        ctx,
        board_stepper(controls, 1, scale),
        "Key level",
        &format!("+{}", card.key_level),
        (Hit::BoardKey(-1), Hit::BoardKey(1)),
    );
    let (fastest, highest) = board_kinds(controls, scale);
    ctx.button(
        fastest,
        "Fastest",
        if card.fastest {
            ButtonKind::Primary
        } else {
            ButtonKind::Secondary
        },
        &Hit::BoardKind(true),
        true,
    );
    ctx.button(
        highest,
        "Highest",
        if card.fastest {
            ButtonKind::Secondary
        } else {
            ButtonKind::Primary
        },
        &Hit::BoardKind(false),
        true,
    );
    ctx.text(
        (controls.x + px(20.0), controls.bottom() - px(34.0)),
        &format!("Season: {}", card.season),
        FAINT,
        UiFont::Medium,
        13.0,
        TextAlign::Left,
    );
    // The table.
    let title = if card.fastest {
        format!("FASTEST  +{}", card.key_level)
    } else {
        "HIGHEST KEYS".to_owned()
    };
    ctx.text(
        (table.x + px(20.0), table.y + px(18.0)),
        &title,
        GOLD,
        UiFont::SemiBold,
        17.0,
        TextAlign::Left,
    );
    ctx.text(
        (table.right() - px(20.0), table.y + px(22.0)),
        &format!("Page {}", card.page.max(1)),
        FAINT,
        UiFont::Medium,
        13.0,
        TextAlign::Right,
    );
    if card.rows.is_empty() {
        let message = if card.loading {
            "Loading..."
        } else {
            "No completed runs here yet."
        };
        ctx.text(
            (table.x + px(20.0), table.y + px(64.0)),
            message,
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
    }
    for (index, row) in card.rows.iter().take(BOARD_ROWS).enumerate() {
        let rect = board_row(table, index, scale);
        if index % 2 == 0 {
            ctx.list.panel(
                rect,
                px(4.0),
                [0.10, 0.11, 0.15, 1.0],
                [0.08, 0.085, 0.115, 1.0],
            );
        }
        let y = rect.y + px(4.0);
        ctx.text(
            (rect.x + px(10.0), y),
            &format!("#{}", row.rank),
            GOLD,
            UiFont::SemiBold,
            13.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.x + px(58.0), y),
            &format!("+{}", row.key_level),
            TEXT,
            UiFont::SemiBold,
            13.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.x + px(104.0), y),
            &row.time,
            row.outcome.color(),
            UiFont::SemiBold,
            13.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.x + px(176.0), y),
            &fit(&row.party, 42),
            DIM,
            UiFont::Medium,
            13.0,
            TextAlign::Left,
        );
    }
    let [back, prev, next] = board_footer(w, h, scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    ctx.button(
        prev,
        "Previous",
        ButtonKind::Secondary,
        &Hit::BoardPage(-1),
        card.can_prev && !card.loading,
    );
    ctx.button(
        next,
        "Next",
        ButtonKind::Secondary,
        &Hit::BoardPage(1),
        card.can_next && !card.loading,
    );
}
