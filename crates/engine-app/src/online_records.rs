//! The History and Leaderboards pages' state: what was fetched, what is selected, and the cards
//! the front-end draws from it.
//!
//! Pages are fetched when they open or when the player pages or changes a control, never per
//! frame; between those moments this is plain data. Everything shown was computed by the
//! server (the realm classified the run, the API ranked the board); the client only formats it.

use engine_net::session::NetCommand;
use engine_presentation::ui::frontend::{
    BOARD_ROWS, BoardCard, BoardRow, HISTORY_ROWS, HistoryCard, HistoryDetail, HistoryRow,
    OutcomeTag,
};
use engine_realm::api::{HistoryEntry, HistoryPage, LeaderboardPage};
use engine_realm::catalog::Catalog;
use engine_realm::modifiers::magnitude_text;
use engine_realm::outcome::RunOutcome;

/// The season the boards show until seasons have a picker.
const SEASON_NAME: &str = "All time";

/// A fetch the pages are waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoardRequest {
    fastest: bool,
    dungeon_id: String,
    key_level: u32,
    api_page: u32,
}

/// The History and Leaderboards state.
#[derive(Debug, Default)]
pub struct Records {
    /// The pages fetched, newest first; the last is the one shown.
    history: Vec<HistoryPage>,
    history_character: Option<String>,
    history_loading: bool,
    selected: Option<usize>,
    dungeon: usize,
    key_level: u32,
    fastest: bool,
    ui_page: u32,
    board: Option<(BoardRequest, LeaderboardPage)>,
    board_loading: Option<BoardRequest>,
}

impl Records {
    /// Opens the History page: forgets what was fetched and asks for the newest runs.
    pub fn open_history(&mut self, character_id: &str) -> NetCommand {
        self.history.clear();
        self.history_character = Some(character_id.to_owned());
        self.history_loading = true;
        self.selected = None;
        NetCommand::FetchHistory {
            character_id: character_id.to_owned(),
            before: None,
            limit: HISTORY_ROWS as u32,
        }
    }

    /// Asks for the page older than the one shown, if there is one.
    pub fn older(&mut self) -> Option<NetCommand> {
        let before = self.history.last()?.next_before?;
        let character_id = self.history_character.clone()?;
        if self.history_loading {
            return None;
        }
        self.history_loading = true;
        Some(NetCommand::FetchHistory {
            character_id,
            before: Some(before),
            limit: HISTORY_ROWS as u32,
        })
    }

    /// Back to the newest page (already fetched: no network).
    pub fn newest(&mut self) {
        self.history.truncate(1);
        self.selected = None;
    }

    /// A history page arrived.
    pub fn history_arrived(&mut self, before: Option<i64>, page: HistoryPage) {
        self.history_loading = false;
        if before.is_none() {
            self.history.clear();
        }
        self.history.push(page);
        self.selected = None;
    }

    /// A history fetch failed.
    pub fn history_failed(&mut self) {
        self.history_loading = false;
    }

    /// Opens the detail of row `index` of the shown page.
    pub fn select(&mut self, index: usize) {
        let rows = self.history.last().map_or(0, |page| page.runs.len());
        self.selected = (index < rows).then_some(index);
    }

    /// Opens the Leaderboards page and asks for the board shown.
    pub fn open_board(&mut self, catalog: &Catalog) -> Option<NetCommand> {
        if self.key_level == 0 {
            self.key_level = 1;
        }
        self.dungeon = self.dungeon.min(catalog.dungeons().len().saturating_sub(1));
        self.clamp_key(catalog);
        self.ui_page = 0;
        self.board = None;
        self.request(catalog)
    }

    /// Steps the dungeon (wrapping) and asks for its board.
    pub fn step_dungeon(&mut self, step: i32, catalog: &Catalog) -> Option<NetCommand> {
        let count = catalog.dungeons().len();
        if count == 0 {
            return None;
        }
        let next = (self.dungeon as i64 + i64::from(step)).rem_euclid(count as i64);
        self.dungeon = next as usize;
        self.clamp_key(catalog);
        self.ui_page = 0;
        self.request(catalog)
    }

    /// Steps the key level within the dungeon's range and asks for the board if it matters.
    pub fn step_key(&mut self, step: i32, catalog: &Catalog) -> Option<NetCommand> {
        let (min, max) = self.key_range(catalog);
        let next = (i64::from(self.key_level) + i64::from(step)).clamp(i64::from(min), i64::from(max));
        self.key_level = u32::try_from(next).unwrap_or(min);
        self.ui_page = 0;
        self.request(catalog)
    }

    /// Switches the board and asks for it.
    pub fn set_kind(&mut self, fastest: bool, catalog: &Catalog) -> Option<NetCommand> {
        self.fastest = fastest;
        self.ui_page = 0;
        self.request(catalog)
    }

    /// Steps the shown page; asks for the board's other API page when the step crosses one.
    pub fn step_page(&mut self, step: i32, catalog: &Catalog) -> Option<NetCommand> {
        let next = i64::from(self.ui_page) + i64::from(step);
        if next < 0 || self.board_loading.is_some() {
            return None;
        }
        self.ui_page = u32::try_from(next).unwrap_or(0);
        self.request(catalog)
    }

    /// A board page arrived.
    pub fn board_arrived(&mut self, page: LeaderboardPage) {
        if let Some(request) = self.board_loading.take() {
            self.board = Some((request, page));
        }
    }

    /// A board fetch failed.
    pub fn board_failed(&mut self) {
        self.board_loading = None;
    }

    fn key_range(&self, catalog: &Catalog) -> (u32, u32) {
        catalog
            .dungeons()
            .get(self.dungeon)
            .map_or((1, 1), |d| (d.keys.min_level.max(1), d.keys.max_level.max(1)))
    }

    fn clamp_key(&mut self, catalog: &Catalog) {
        let (min, max) = self.key_range(catalog);
        self.key_level = self.key_level.clamp(min, max);
    }

    /// The fetch the controls call for, unless the page already shown answers it.
    fn request(&mut self, catalog: &Catalog) -> Option<NetCommand> {
        let dungeon_id = catalog.dungeons().get(self.dungeon)?.id.clone();
        let wanted = BoardRequest {
            fastest: self.fastest,
            dungeon_id,
            key_level: self.key_level,
            // Two screens of rows to one API page.
            api_page: self.ui_page / 2,
        };
        let wanted_key = if wanted.fastest { wanted.key_level } else { 0 };
        let same = |request: &BoardRequest| {
            request.fastest == wanted.fastest
                && request.dungeon_id == wanted.dungeon_id
                && request.api_page == wanted.api_page
                && (if request.fastest { request.key_level } else { 0 }) == wanted_key
        };
        if self.board.as_ref().is_some_and(|(request, _)| same(request)) {
            return None;
        }
        self.board_loading = Some(wanted.clone());
        Some(NetCommand::FetchLeaderboard {
            board: if wanted.fastest { "fastest" } else { "highest" }.to_owned(),
            dungeon_id: wanted.dungeon_id,
            key_level: wanted.fastest.then_some(wanted.key_level),
            page: wanted.api_page,
        })
    }

    /// The History page as the front-end draws it.
    #[must_use]
    pub fn history_card(&self, catalog: &Catalog) -> HistoryCard {
        let Some(page) = self.history.last() else {
            return HistoryCard {
                loading: self.history_loading,
                ..HistoryCard::default()
            };
        };
        let rows: Vec<HistoryRow> = page
            .runs
            .iter()
            .take(HISTORY_ROWS)
            .map(|run| HistoryRow {
                dungeon: dungeon_name(catalog, &run.dungeon_id),
                key_level: run.key_level,
                outcome: tag(run.outcome),
                duration: format_duration_us(run.duration_us),
                when: format_date_utc(run.applied_at_us),
            })
            .collect();
        let detail = self
            .selected
            .and_then(|index| page.runs.get(index))
            .map(|run| detail_of(run, catalog));
        HistoryCard {
            selected: self.selected.filter(|index| *index < rows.len()),
            rows,
            detail,
            loading: self.history_loading,
            can_older: page.next_before.is_some(),
            can_newer: self.history.len() > 1,
        }
    }

    /// The Leaderboards page as the front-end draws it.
    #[must_use]
    pub fn board_card(&self, catalog: &Catalog) -> BoardCard {
        let dungeon = catalog
            .dungeons()
            .get(self.dungeon)
            .map_or_else(String::new, |d| d.name.clone());
        let mut card = BoardCard {
            dungeon,
            key_level: self.key_level.max(1),
            fastest: self.fastest,
            season: SEASON_NAME.to_owned(),
            loading: self.board_loading.is_some(),
            page: self.ui_page + 1,
            ..BoardCard::default()
        };
        let Some((request, page)) = &self.board else {
            return card;
        };
        let half = (self.ui_page % 2) as usize;
        let start = half * BOARD_ROWS;
        card.rows = page
            .rows
            .iter()
            .skip(start)
            .take(BOARD_ROWS)
            .map(|row| BoardRow {
                rank: row.rank,
                key_level: row.key_level,
                outcome: tag(row.outcome),
                time: format_duration_us(row.duration_us),
                party: row
                    .party
                    .iter()
                    .map(|member| member.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            })
            .collect();
        // A later screen exists inside this API page, or in the next one.
        card.can_next = page.rows.len() > start + BOARD_ROWS || (half == 1 && page.has_more);
        card.can_prev = self.ui_page > 0;
        // The rows belong to the request that fetched them: a stale answer is not shown under
        // new controls.
        if request.api_page != self.ui_page / 2 {
            card.rows.clear();
        }
        card
    }
}

fn tag(outcome: RunOutcome) -> OutcomeTag {
    match outcome {
        RunOutcome::Timed => OutcomeTag::Timed,
        RunOutcome::Overtime => OutcomeTag::Overtime,
        RunOutcome::Failed => OutcomeTag::Failed,
        RunOutcome::Abandoned => OutcomeTag::Abandoned,
    }
}

fn dungeon_name(catalog: &Catalog, id: &str) -> String {
    catalog
        .dungeon(id)
        .map_or_else(|| id.replace('_', " "), |d| d.name.clone())
}

fn detail_of(run: &HistoryEntry, catalog: &Catalog) -> HistoryDetail {
    let timer = if run.timer_us > 0 {
        format!(
            "{} / {}",
            format_duration_us(run.duration_us),
            format_duration_us(run.timer_us)
        )
    } else {
        format_duration_us(run.duration_us)
    };
    HistoryDetail {
        title: format!(
            "{} +{}",
            dungeon_name(catalog, &run.dungeon_id),
            run.key_level
        ),
        outcome: tag(run.outcome),
        stats: vec![
            ("Time".to_owned(), timer),
            ("Deaths".to_owned(), run.deaths.to_string()),
            ("Damage taken".to_owned(), thousands(run.damage_taken)),
            ("Healing".to_owned(), thousands(run.healing)),
            ("DPS".to_owned(), format!("{:.0}", run.dps)),
        ],
        party: run
            .party
            .iter()
            .map(|member| format!("{} ({})", member.name, member.hero))
            .collect(),
        modifiers: run
            .modifiers
            .iter()
            .map(|modifier| {
                let name = catalog
                    .modifiers()
                    .modifier(&modifier.id)
                    .map_or_else(|| modifier.id.clone(), |d| d.name.clone());
                format!("{name} {}", magnitude_text(modifier.magnitude_pct))
            })
            .collect(),
        loot: run
            .loot
            .iter()
            .map(|loot| {
                let name = engine_game::character::item_by_key(&loot.item)
                    .map_or_else(|| loot.item.clone(), |def| def.name.to_owned());
                let broken = if loot.broken { " (broken)" } else { "" };
                format!("{name} {}% +{}{broken}", loot.quality, loot.sharpening)
            })
            .collect(),
    }
}

/// A figure with thousands separators.
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Microseconds as `mm:ss` (minutes keep counting past an hour).
#[must_use]
pub fn format_duration_us(us: i64) -> String {
    let seconds = (us.max(0) / engine_realm::SECOND) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Microseconds since the Unix epoch as `YYYY-MM-DD HH:MM` (UTC).
#[must_use]
pub fn format_date_utc(us: i64) -> String {
    let seconds = us.div_euclid(engine_realm::SECOND);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Civil date from the day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_realm::api::{LeaderboardRow, RunMember};

    fn catalog() -> Catalog {
        Catalog::builtin().expect("catalog")
    }

    fn entry(key: u32, outcome: RunOutcome) -> HistoryEntry {
        HistoryEntry {
            run_uid: format!("{key:032x}"),
            dungeon_id: "graveyard_crypt".into(),
            key_level: key,
            outcome,
            duration_us: 754 * engine_realm::SECOND,
            timer_us: 1500 * engine_realm::SECOND,
            applied_at_us: 1_792_000_000_000_000,
            party: vec![RunMember {
                character_id: String::new(),
                name: "Ash".into(),
                hero: "Fire Mage".into(),
                role: "dps".into(),
            }],
            modifiers: vec![],
            deaths: 1,
            damage_taken: 1_234_567,
            healing: 0,
            dps: 812.4,
            loot: vec![],
        }
    }

    #[test]
    fn dates_and_durations_format() {
        assert_eq!(format_date_utc(0), "1970-01-01 00:00");
        assert_eq!(format_date_utc(951_782_400 * 1_000_000), "2000-02-29 00:00");
        assert_eq!(format_date_utc(1_792_000_000 * 1_000_000), "2026-10-14 17:46");
        assert_eq!(format_duration_us(754 * 1_000_000), "12:34");
        assert_eq!(format_duration_us(-5), "00:00");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }

    #[test]
    fn history_opens_pages_older_and_returns_to_the_newest_without_the_network() {
        let catalog = catalog();
        let mut records = Records::default();
        assert!(matches!(
            records.open_history("c1"),
            NetCommand::FetchHistory { before: None, .. }
        ));
        assert!(records.history_card(&catalog).loading);
        records.history_arrived(
            None,
            HistoryPage {
                runs: vec![entry(5, RunOutcome::Timed), entry(4, RunOutcome::Failed)],
                next_before: Some(77),
            },
        );
        let card = records.history_card(&catalog);
        assert_eq!(card.rows.len(), 2);
        assert!(card.can_older && !card.can_newer && !card.loading);
        assert_eq!(card.rows[0].outcome, OutcomeTag::Timed);
        assert_eq!(card.rows[0].duration, "12:34");
        // Older asks for the cursor; a second press while loading does nothing.
        assert!(matches!(
            records.older(),
            Some(NetCommand::FetchHistory { before: Some(77), .. })
        ));
        assert!(records.older().is_none());
        records.history_arrived(
            Some(77),
            HistoryPage {
                runs: vec![entry(3, RunOutcome::Overtime)],
                next_before: None,
            },
        );
        let card = records.history_card(&catalog);
        assert_eq!(card.rows[0].key_level, 3);
        assert!(card.can_newer && !card.can_older);
        records.newest();
        assert_eq!(records.history_card(&catalog).rows[0].key_level, 5);
        // Selecting a row opens its detail.
        records.select(0);
        let detail = records.history_card(&catalog).detail.expect("detail");
        assert_eq!(detail.title, "Graveyard Crypt +5");
        assert_eq!(detail.stats[0], ("Time".to_owned(), "12:34 / 25:00".to_owned()));
        assert_eq!(detail.stats[2].1, "1,234,567");
        assert_eq!(detail.party, ["Ash (Fire Mage)"]);
        records.select(9);
        assert!(records.history_card(&catalog).detail.is_none());
    }

    fn board_page(rows: usize, has_more: bool) -> LeaderboardPage {
        LeaderboardPage {
            board: "highest".into(),
            dungeon_id: "graveyard_crypt".into(),
            key_level: None,
            season_id: "season-0".into(),
            page: 0,
            page_size: 25,
            rows: (1..=rows as u32)
                .map(|rank| LeaderboardRow {
                    rank,
                    run_uid: format!("{rank:032x}"),
                    key_level: 9,
                    outcome: RunOutcome::Timed,
                    duration_us: 600 * engine_realm::SECOND,
                    timer_us: 0,
                    finished_at_us: 0,
                    party: vec![],
                })
                .collect(),
            has_more,
        }
    }

    #[test]
    fn the_board_fetches_on_open_and_on_control_changes_only() {
        let catalog = catalog();
        let mut records = Records::default();
        let open = records.open_board(&catalog).expect("fetch on open");
        assert!(matches!(
            open,
            NetCommand::FetchLeaderboard { key_level: None, page: 0, .. }
        ));
        records.board_arrived(board_page(25, false));
        // Same request again: nothing to fetch. Paging inside the API page is free too.
        assert!(records.step_page(1, &catalog).is_none());
        let card = records.board_card(&catalog);
        assert_eq!(card.rows.len(), 25 - BOARD_ROWS);
        assert_eq!(card.rows[0].rank, BOARD_ROWS as u32 + 1);
        assert_eq!(card.page, 2);
        assert!(card.can_prev && !card.can_next, "25 rows, no more pages");
        // Crossing into the API's second page fetches it, but only if it exists.
        let mut records = Records::default();
        records.open_board(&catalog);
        records.board_arrived(board_page(25, true));
        assert!(records.board_card(&catalog).can_next);
        records.step_page(1, &catalog);
        assert!(records.board_card(&catalog).can_next, "has_more past the second half");
        assert!(matches!(
            records.step_page(1, &catalog),
            Some(NetCommand::FetchLeaderboard { page: 1, .. })
        ));
        // The fastest board ranks one key level: it asks for it, and a key step refetches.
        let mut records = Records::default();
        records.open_board(&catalog);
        records.board_arrived(board_page(3, false));
        assert!(matches!(
            records.set_kind(true, &catalog),
            Some(NetCommand::FetchLeaderboard { key_level: Some(1), .. })
        ));
        records.board_arrived(board_page(3, false));
        assert!(matches!(
            records.step_key(1, &catalog),
            Some(NetCommand::FetchLeaderboard { key_level: Some(2), .. })
        ));
        // On the highest board the key level is not part of the question.
        records.board_arrived(board_page(3, false));
        records.set_kind(false, &catalog);
        records.board_arrived(board_page(3, false));
        assert!(records.step_key(1, &catalog).is_none());
    }

    #[test]
    fn the_key_stays_inside_the_dungeons_range() {
        let catalog = catalog();
        let mut records = Records::default();
        records.open_board(&catalog);
        records.step_key(-5, &catalog);
        assert_eq!(records.board_card(&catalog).key_level, 1);
        records.step_key(10_000, &catalog);
        let max = catalog.dungeons()[0].keys.max_level;
        assert_eq!(records.board_card(&catalog).key_level, max);
        records.step_dungeon(1, &catalog);
        records.step_dungeon(-1, &catalog);
        assert!(!records.board_card(&catalog).dungeon.is_empty());
    }
}
