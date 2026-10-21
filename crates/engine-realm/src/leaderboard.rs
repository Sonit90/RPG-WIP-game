//! Leaderboards: which boards exist, how runs are ranked, and how many rows a page holds.
//!
//! The boards are a query over the persistence API's `dungeon_runs` table, written once per run
//! by `apply_result`. Nothing here talks to the realm, and the realm never reads a board.
//!
//! ## Ranking
//!
//! The ranking rule is data (`assets/catalog/leaderboards.json`, `"score": {"kind": ...}`) and a
//! closed enum ([`ScoreKind`]) in code, so a different rule is one new variant: its
//! [`ScoreKind::compare`] (the specification, unit-tested) and its [`ScoreKind::order_by`] (the
//! same order as SQL; the database tests check the two agree).
//!
//! `key_then_time`:
//!
//! * **highest** - key level descending, then timed before overtime, then duration ascending,
//!   then finish time ascending (the earlier run wins a tie), then run uid (so equal runs
//!   still have one order).
//! * **fastest** (one dungeon at one key level) - duration ascending, then finish time
//!   ascending, then run uid.
//!
//! Only completed runs ([`crate::outcome::RunOutcome::counts_for_boards`]) are ranked; one row
//! per run, not per player.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::outcome::RunOutcome;

const LEADERBOARDS_JSON: &str = include_str!("../../../assets/catalog/leaderboards.json");

/// The most rows a page may hold, whatever the data says.
pub const MAX_PAGE_SIZE: u32 = 100;

/// How a board is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Board {
    /// The quickest runs of one dungeon at one key level.
    Fastest,
    /// The highest keys completed in one dungeon.
    Highest,
    /// Players ranked by how many times they completed one dungeon (a community map's board;
    /// every other board has one row per run, this has one row per player).
    Completions,
}

impl Board {
    /// The stable written name (also the query parameter).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Fastest => "fastest",
            Self::Highest => "highest",
            Self::Completions => "completions",
        }
    }

    /// The board written as `key`.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "fastest" => Some(Self::Fastest),
            "highest" => Some(Self::Highest),
            "completions" => Some(Self::Completions),
            _ => None,
        }
    }
}

/// The ranking rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreKind {
    /// Higher key first, timed before overtime, then faster, then earlier.
    KeyThenTime,
}

/// What ranking looks at in one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunScore {
    /// The key level the run was played at.
    pub key_level: u32,
    /// How it ended.
    pub outcome: RunOutcome,
    /// How long it took, microseconds.
    pub duration_us: i64,
    /// When it finished, microseconds since the epoch.
    pub finished_at_us: i64,
    /// The run's uid: the last tie-break.
    pub run_uid: String,
}

impl ScoreKind {
    /// Whether `outcome` can be ranked at all.
    #[must_use]
    pub fn eligible(self, outcome: RunOutcome) -> bool {
        match self {
            Self::KeyThenTime => outcome.counts_for_boards(),
        }
    }

    /// Orders two runs on `board`: `Less` ranks higher.
    #[must_use]
    pub fn compare(self, board: Board, a: &RunScore, b: &RunScore) -> Ordering {
        match (self, board) {
            // The completions board ranks players, not runs ([`Self::compare_completions`]); asked
            // to order two runs it falls back to the highest-key order.
            (Self::KeyThenTime, Board::Highest | Board::Completions) => b
                .key_level
                .cmp(&a.key_level)
                .then_with(|| timed_first(a.outcome).cmp(&timed_first(b.outcome)))
                .then(a.duration_us.cmp(&b.duration_us))
                .then(a.finished_at_us.cmp(&b.finished_at_us))
                .then_with(|| a.run_uid.cmp(&b.run_uid)),
            (Self::KeyThenTime, Board::Fastest) => a
                .duration_us
                .cmp(&b.duration_us)
                .then(a.finished_at_us.cmp(&b.finished_at_us))
                .then_with(|| a.run_uid.cmp(&b.run_uid)),
        }
    }

    /// The same order as an SQL `ORDER BY` list over `game.dungeon_runs` (`finished_at` is a
    /// timestamp there). A constant, never built from input.
    #[must_use]
    pub fn order_by(self, board: Board) -> &'static str {
        match (self, board) {
            (Self::KeyThenTime, Board::Highest | Board::Completions) => {
                "key_level DESC, (outcome = 'timed') DESC, duration_us ASC, finished_at ASC, run_uid ASC"
            }
            (Self::KeyThenTime, Board::Fastest) => "duration_us ASC, finished_at ASC, run_uid ASC",
        }
    }
}

/// What the completions board ranks in one player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionScore {
    /// Completed runs (timed or overtime) of the dungeon.
    pub completions: u32,
    /// The highest key level among them.
    pub best_key_level: u32,
    /// The fastest of them, microseconds.
    pub best_duration_us: i64,
    /// The player's name: the last tie-break.
    pub name: String,
}

impl ScoreKind {
    /// Orders two players on the completions board: `Less` ranks higher. More completions
    /// first, then the higher best key, then the faster best time, then the name.
    #[must_use]
    pub fn compare_completions(self, a: &CompletionScore, b: &CompletionScore) -> Ordering {
        match self {
            Self::KeyThenTime => b
                .completions
                .cmp(&a.completions)
                .then(b.best_key_level.cmp(&a.best_key_level))
                .then(a.best_duration_us.cmp(&b.best_duration_us))
                .then_with(|| a.name.cmp(&b.name)),
        }
    }

    /// The same order as an SQL `ORDER BY` over the completions query's columns. A constant.
    #[must_use]
    pub fn order_by_completions(self) -> &'static str {
        match self {
            Self::KeyThenTime => "completions DESC, best_key DESC, best_us ASC, name ASC",
        }
    }
}

fn timed_first(outcome: RunOutcome) -> u8 {
    u8::from(outcome != RunOutcome::Timed)
}

/// The leaderboard data file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LeaderboardConfig {
    version: u32,
    score: ScoreRecord,
    page_size: u32,
    boards: Vec<Board>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ScoreRecord {
    kind: ScoreKind,
}

/// The leaderboard file is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardError(pub String);

impl std::fmt::Display for LeaderboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "leaderboards.json: {}", self.0)
    }
}

impl std::error::Error for LeaderboardError {}

impl LeaderboardConfig {
    /// The file compiled into the program.
    ///
    /// # Errors
    ///
    /// The embedded file is invalid (a test keeps that from shipping).
    pub fn builtin() -> Result<Self, LeaderboardError> {
        Self::parse(LEADERBOARDS_JSON)
    }

    /// Reads and validates a leaderboard file.
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version, a page size outside `1..=100`, or no (or a repeated)
    /// board.
    pub fn parse(text: &str) -> Result<Self, LeaderboardError> {
        let config: Self =
            serde_json::from_str(text).map_err(|e| LeaderboardError(e.to_string()))?;
        if config.version != 1 {
            return Err(LeaderboardError(format!(
                "unsupported version {}",
                config.version
            )));
        }
        if !(1..=MAX_PAGE_SIZE).contains(&config.page_size) {
            return Err(LeaderboardError(format!(
                "page_size must be 1..={MAX_PAGE_SIZE}"
            )));
        }
        if config.boards.is_empty() {
            return Err(LeaderboardError("no boards".into()));
        }
        for (index, board) in config.boards.iter().enumerate() {
            if config.boards[..index].contains(board) {
                return Err(LeaderboardError(format!("board {} twice", board.key())));
            }
        }
        Ok(config)
    }

    /// The ranking rule.
    #[must_use]
    pub fn score(&self) -> ScoreKind {
        self.score.kind
    }

    /// Rows per page.
    #[must_use]
    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    /// The boards, in display order.
    #[must_use]
    pub fn boards(&self) -> &[Board] {
        &self.boards
    }

    /// Whether `board` is offered.
    #[must_use]
    pub fn offers(&self, board: Board) -> bool {
        self.boards.contains(&board)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn run(key: u32, outcome: RunOutcome, duration: i64, finished: i64, uid: &str) -> RunScore {
        RunScore {
            key_level: key,
            outcome,
            duration_us: duration,
            finished_at_us: finished,
            run_uid: uid.to_owned(),
        }
    }

    fn sorted(board: Board, mut runs: Vec<RunScore>) -> Vec<String> {
        runs.sort_by(|a, b| ScoreKind::KeyThenTime.compare(board, a, b));
        runs.into_iter().map(|r| r.run_uid).collect()
    }

    #[test]
    fn the_shipped_file_parses() {
        let config = LeaderboardConfig::builtin().expect("valid");
        assert_eq!(config.score(), ScoreKind::KeyThenTime);
        assert_eq!(config.page_size(), 25);
        assert_eq!(
            config.boards(),
            [Board::Fastest, Board::Highest, Board::Completions]
        );
        assert!(config.offers(Board::Highest));
    }

    #[test]
    fn bad_files_are_refused() {
        let with = |version: u32, page: u32, boards: &str| {
            format!(
                r#"{{"version":{version},"score":{{"kind":"key_then_time"}},"page_size":{page},"boards":{boards}}}"#
            )
        };
        assert!(LeaderboardConfig::parse(&with(1, 25, r#"["fastest"]"#)).is_ok());
        assert!(LeaderboardConfig::parse(&with(2, 25, r#"["fastest"]"#)).is_err());
        assert!(LeaderboardConfig::parse(&with(1, 0, r#"["fastest"]"#)).is_err());
        assert!(LeaderboardConfig::parse(&with(1, 101, r#"["fastest"]"#)).is_err());
        assert!(LeaderboardConfig::parse(&with(1, 25, "[]")).is_err());
        assert!(LeaderboardConfig::parse(&with(1, 25, r#"["fastest","fastest"]"#)).is_err());
        assert!(LeaderboardConfig::parse(&with(1, 25, r#"["slowest"]"#)).is_err());
        assert!(
            LeaderboardConfig::parse(
                r#"{"version":1,"score":{"kind":"mystery"},"page_size":5,"boards":["fastest"]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn highest_orders_by_key_then_timed_then_speed_then_age() {
        let order = sorted(
            Board::Highest,
            vec![
                run(5, RunOutcome::Overtime, 100, 1, "overtime-5"),
                run(7, RunOutcome::Overtime, 900, 9, "overtime-7"),
                run(5, RunOutcome::Timed, 300, 5, "timed-5-slow"),
                run(5, RunOutcome::Timed, 200, 6, "timed-5-late"),
                run(5, RunOutcome::Timed, 200, 4, "timed-5-early"),
                run(7, RunOutcome::Timed, 800, 8, "timed-7"),
            ],
        );
        assert_eq!(
            order,
            [
                "timed-7",
                "overtime-7",
                "timed-5-early",
                "timed-5-late",
                "timed-5-slow",
                "overtime-5"
            ]
        );
    }

    #[test]
    fn fastest_orders_by_duration_then_age_and_ignores_timed_versus_overtime() {
        let order = sorted(
            Board::Fastest,
            vec![
                run(5, RunOutcome::Timed, 300, 1, "c"),
                run(5, RunOutcome::Overtime, 200, 2, "b-late"),
                run(5, RunOutcome::Timed, 200, 1, "b-early"),
                run(5, RunOutcome::Timed, 100, 9, "a"),
            ],
        );
        assert_eq!(order, ["a", "b-early", "b-late", "c"]);
    }

    #[test]
    fn identical_runs_still_have_one_order() {
        for board in [Board::Fastest, Board::Highest] {
            let a = run(3, RunOutcome::Timed, 10, 10, "a");
            let b = run(3, RunOutcome::Timed, 10, 10, "b");
            let kind = ScoreKind::KeyThenTime;
            assert_eq!(kind.compare(board, &a, &b), Ordering::Less);
            assert_eq!(kind.compare(board, &b, &a), Ordering::Greater);
            assert_eq!(kind.compare(board, &a, &a), Ordering::Equal);
        }
    }

    #[test]
    fn completions_rank_by_count_then_key_then_speed_then_name() {
        let score = |completions, best_key_level, best_duration_us, name: &str| CompletionScore {
            completions,
            best_key_level,
            best_duration_us,
            name: name.to_owned(),
        };
        let kind = ScoreKind::KeyThenTime;
        let mut players = [
            score(3, 5, 100, "b"),
            score(5, 1, 900, "a"),
            score(3, 7, 500, "c"),
            score(3, 5, 80, "d"),
            score(3, 5, 80, "a2"),
        ];
        players.sort_by(|a, b| kind.compare_completions(a, b));
        let names: Vec<&str> = players.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "c", "a2", "d", "b"]);
        assert!(kind.order_by_completions().starts_with("completions DESC"));
    }

    #[test]
    fn only_completions_are_eligible() {
        let eligible: Vec<_> = RunOutcome::ALL
            .into_iter()
            .filter(|o| ScoreKind::KeyThenTime.eligible(*o))
            .collect();
        assert_eq!(eligible, [RunOutcome::Timed, RunOutcome::Overtime]);
    }

    #[test]
    fn board_names_round_trip() {
        for board in [Board::Fastest, Board::Highest] {
            assert_eq!(Board::from_key(board.key()), Some(board));
        }
        assert_eq!(Board::from_key("all"), None);
    }
}
