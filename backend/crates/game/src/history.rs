//! Run history and leaderboards: the queries behind `GET /characters/{id}/history`,
//! `GET /runs/{uid}` and `GET /leaderboards`, and the rows `store::apply_result` writes for them.
//!
//! Everything here reads `game.dungeon_runs` and `game.dungeon_results`, which only
//! `apply_result` and `apply_loot` write, from what the realm reported. Nothing a client sends
//! reaches those tables, and nothing here asks the realm.

use engine_realm::api::{
    DungeonResult, HistoryEntry, HistoryPage, LeaderboardPage, LeaderboardRow, RunDetail, RunLoot,
    RunMember, HISTORY_MAX_LIMIT,
};
use engine_realm::leaderboard::{Board, LeaderboardConfig};
use engine_realm::modifiers::KeyModifier;
use engine_realm::outcome::RunOutcome;
use generic_auth_core::AppError;
use sqlx::types::Json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::store::db;

/// The most players a run's row lists.
const MAX_PARTY: usize = 8;

/// The season runs fall in when the table of seasons has no active one.
pub const FALLBACK_SEASON: &str = "season-0";

/// A player's own figures for a run, as `dungeon_results` stores them, validated.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RunStats {
    pub deaths: i32,
    pub damage_taken: i64,
    pub healing: i64,
    pub dps: f32,
}

/// The figures a result's damage report carries. A missing report is zeros; an impossible figure
/// is dropped, because the insert shares a transaction with the experience and a constraint
/// violation over a statistic must not cost the player their reward.
#[must_use]
pub fn run_stats(result: &DungeonResult) -> RunStats {
    let Some(report) = result.damage_report.as_ref() else {
        return RunStats::default();
    };
    let clamp = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    RunStats {
        deaths: i32::try_from(report.deaths).unwrap_or(i32::MAX),
        damage_taken: clamp(report.damage_taken),
        healing: clamp(report.healing),
        dps: if report.dps.is_finite() && report.dps >= 0.0 {
            report.dps
        } else {
            0.0
        },
    }
}

/// The `dungeon_runs` row a result asks for, or `None` when it must not write one: a result from
/// before runs were recorded (no uid) or a member who left a run that went on (no party).
#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub run_uid: String,
    pub key_level: i32,
    pub modifiers: serde_json::Value,
    pub outcome: RunOutcome,
    pub duration_us: i64,
    pub timer_us: i64,
    pub party: serde_json::Value,
}

/// See [`RunRow`].
#[must_use]
pub fn run_row(result: &DungeonResult) -> Option<RunRow> {
    if result.run_uid.is_empty() || result.party.is_empty() {
        return None;
    }
    let party: Vec<&RunMember> = result.party.iter().take(MAX_PARTY).collect();
    Some(RunRow {
        run_uid: result.run_uid.clone(),
        key_level: i32::try_from(result.key_level).unwrap_or(i32::MAX),
        modifiers: serde_json::to_value(&result.modifiers).ok()?,
        outcome: result.outcome(),
        duration_us: result.duration_us.max(0),
        timer_us: result.timer_us.max(0),
        party: serde_json::to_value(party).ok()?,
    })
}

/// The loot entry a delivered item adds to its result's history.
#[must_use]
pub fn loot_entry(delivery: &engine_realm::api::LootDelivery) -> RunLoot {
    let props = delivery.props();
    RunLoot {
        item: delivery.item.clone(),
        quality: props.quality,
        sharpening: props.sharpening,
        broken: props.broken,
    }
}

/// The result id a loot id belongs to: everything before `":loot:"` or `":mob:"`
/// (`"<uid>:<account>:loot:<n>"` belongs to `"<uid>:<account>"`).
#[must_use]
pub fn result_id_of_loot(loot_id: &str) -> Option<&str> {
    [":loot:", ":mob:"]
        .iter()
        .find_map(|marker| loot_id.rfind(marker).map(|at| &loot_id[..at]))
        .filter(|id| !id.is_empty())
}

const HISTORY_COLUMNS: &str = "r.run_uid, r.dungeon_id, r.key_level,
    COALESCE(r.outcome, CASE WHEN r.success THEN 'timed' ELSE 'failed' END) AS outcome,
    r.duration_us, COALESCE(u.timer_us, 0) AS timer_us,
    (EXTRACT(EPOCH FROM r.applied_at) * 1000000)::bigint AS applied_at_us,
    COALESCE(u.party, '[]'::jsonb) AS party, COALESCE(u.modifiers, '[]'::jsonb) AS modifiers,
    r.deaths, r.damage_taken, r.healing, r.dps, r.loot";

#[derive(sqlx::FromRow)]
struct HistoryRow {
    run_uid: Option<String>,
    dungeon_id: String,
    key_level: i32,
    outcome: String,
    duration_us: i64,
    timer_us: i64,
    applied_at_us: i64,
    party: Json<serde_json::Value>,
    modifiers: Json<serde_json::Value>,
    deaths: i32,
    damage_taken: i64,
    healing: i64,
    dps: f32,
    loot: Json<serde_json::Value>,
}

/// A stored document read leniently: a shape this build does not know is an empty list rather
/// than an error that hides the player's whole history.
fn lenient<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Vec<T> {
    serde_json::from_value(value).unwrap_or_default()
}

impl HistoryRow {
    fn into_entry(self) -> HistoryEntry {
        HistoryEntry {
            run_uid: self.run_uid.unwrap_or_default(),
            dungeon_id: self.dungeon_id,
            key_level: u32::try_from(self.key_level).unwrap_or(0),
            outcome: RunOutcome::from_key(&self.outcome).unwrap_or(RunOutcome::Failed),
            duration_us: self.duration_us,
            timer_us: self.timer_us,
            applied_at_us: self.applied_at_us,
            party: lenient::<RunMember>(self.party.0),
            modifiers: lenient::<KeyModifier>(self.modifiers.0),
            deaths: u32::try_from(self.deaths).unwrap_or(0),
            damage_taken: u64::try_from(self.damage_taken).unwrap_or(0),
            healing: u64::try_from(self.healing).unwrap_or(0),
            dps: self.dps,
            loot: lenient::<RunLoot>(self.loot.0),
        }
    }
}

/// One page of a character's runs, newest first. `before` is the previous page's
/// `next_before` (microseconds since the epoch); `limit` is clamped to `1..=50`.
///
/// The caller has already checked the character is theirs.
///
/// # Errors
///
/// The database failed.
pub async fn history(
    pool: &PgPool,
    character_id: Uuid,
    before: Option<i64>,
    limit: u32,
) -> Result<HistoryPage, AppError> {
    let limit = limit.clamp(1, HISTORY_MAX_LIMIT);
    let sql = format!(
        "SELECT {HISTORY_COLUMNS}
         FROM game.dungeon_results r LEFT JOIN game.dungeon_runs u ON u.run_uid = r.run_uid
         WHERE r.character_id = $1
           AND ($2::bigint IS NULL
                OR r.applied_at < 'epoch'::timestamptz + $2::float8 * interval '1 microsecond')
         ORDER BY r.applied_at DESC, r.result_id DESC
         LIMIT $3"
    );
    // One more than asked, to know whether an older page exists.
    let rows: Vec<HistoryRow> = sqlx::query_as(&sql)
        .bind(character_id)
        .bind(before)
        .bind(i64::from(limit) + 1)
        .fetch_all(pool)
        .await
        .map_err(db)?;
    let more = rows.len() > limit as usize;
    let runs: Vec<HistoryEntry> = rows
        .into_iter()
        .take(limit as usize)
        .map(HistoryRow::into_entry)
        .collect();
    let next_before = if more {
        runs.last().map(|run| run.applied_at_us)
    } else {
        None
    };
    Ok(HistoryPage { runs, next_before })
}

#[derive(sqlx::FromRow)]
struct RunRecord {
    run_uid: String,
    season_id: String,
    dungeon_id: String,
    key_level: i32,
    outcome: String,
    duration_us: i64,
    timer_us: i64,
    finished_at_us: i64,
    party: Json<serde_json::Value>,
    modifiers: Json<serde_json::Value>,
}

/// One run, for an account that took part in it. `None` when there is no such run or the account
/// was not in it (the same answer, so a uid reveals nothing).
///
/// # Errors
///
/// The database failed.
pub async fn run_detail(
    pool: &PgPool,
    account_id: Uuid,
    run_uid: &str,
) -> Result<Option<RunDetail>, AppError> {
    let run: Option<RunRecord> = sqlx::query_as(
        "SELECT u.run_uid, u.season_id, u.dungeon_id, u.key_level, u.outcome, u.duration_us,
                u.timer_us, (EXTRACT(EPOCH FROM u.finished_at) * 1000000)::bigint AS finished_at_us,
                u.party, u.modifiers
         FROM game.dungeon_runs u
         WHERE u.run_uid = $1
           AND EXISTS (SELECT 1 FROM game.dungeon_results r
                       WHERE r.run_uid = u.run_uid AND r.account_id = $2)",
    )
    .bind(run_uid)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(db)?;
    let Some(run) = run else {
        return Ok(None);
    };
    let sql = format!(
        "SELECT {HISTORY_COLUMNS}
         FROM game.dungeon_results r LEFT JOIN game.dungeon_runs u ON u.run_uid = r.run_uid
         WHERE r.run_uid = $1 AND r.account_id = $2
         ORDER BY r.applied_at, r.result_id LIMIT 1"
    );
    let you: Option<HistoryRow> = sqlx::query_as(&sql)
        .bind(run_uid)
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .map_err(db)?;
    Ok(Some(RunDetail {
        run_uid: run.run_uid,
        season_id: Some(run.season_id),
        dungeon_id: run.dungeon_id,
        key_level: u32::try_from(run.key_level).unwrap_or(0),
        outcome: RunOutcome::from_key(&run.outcome).unwrap_or(RunOutcome::Failed),
        duration_us: run.duration_us,
        timer_us: run.timer_us,
        finished_at_us: run.finished_at_us,
        party: lenient::<RunMember>(run.party.0),
        modifiers: lenient::<KeyModifier>(run.modifiers.0),
        you: you.map(HistoryRow::into_entry),
    }))
}

/// Which board to read.
#[derive(Debug, Clone)]
pub struct BoardQuery<'a> {
    pub board: Board,
    pub dungeon_id: &'a str,
    /// `fastest` ranks one key level; `highest` ignores it.
    pub key_level: Option<u32>,
    pub season_id: &'a str,
    /// From 0.
    pub page: u32,
}

#[derive(sqlx::FromRow)]
struct BoardRecord {
    run_uid: String,
    key_level: i32,
    outcome: String,
    duration_us: i64,
    timer_us: i64,
    finished_at_us: i64,
    party: Json<serde_json::Value>,
}

/// One page of a leaderboard: completed runs of one dungeon in one season, ranked by the
/// configured rule. One row per run.
///
/// # Errors
///
/// The database failed.
pub async fn leaderboard(
    pool: &PgPool,
    config: &LeaderboardConfig,
    query: &BoardQuery<'_>,
) -> Result<LeaderboardPage, AppError> {
    let page_size = config.page_size();
    let offset = i64::from(query.page) * i64::from(page_size);
    let key = match query.board {
        Board::Fastest => query.key_level,
        Board::Highest => None,
    };
    // The order is a constant chosen by the configured rule and the board; the eligible
    // outcomes are the rule's too.
    let eligible: Vec<&str> = RunOutcome::ALL
        .into_iter()
        .filter(|outcome| config.score().eligible(*outcome))
        .map(RunOutcome::key)
        .collect();
    let sql = format!(
        "SELECT run_uid, key_level, outcome, duration_us, timer_us,
                (EXTRACT(EPOCH FROM finished_at) * 1000000)::bigint AS finished_at_us, party
         FROM game.dungeon_runs
         WHERE season_id = $1 AND dungeon_id = $2 AND outcome = ANY($3)
           AND ($4::int IS NULL OR key_level = $4)
         ORDER BY {}
         LIMIT $5 OFFSET $6",
        config.score().order_by(query.board)
    );
    let rows: Vec<BoardRecord> = sqlx::query_as(&sql)
        .bind(query.season_id)
        .bind(query.dungeon_id)
        .bind(&eligible)
        .bind(key.map(|level| i32::try_from(level).unwrap_or(i32::MAX)))
        .bind(i64::from(page_size) + 1)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(db)?;
    let has_more = rows.len() > page_size as usize;
    let rows = rows
        .into_iter()
        .take(page_size as usize)
        .enumerate()
        .map(|(index, row)| LeaderboardRow {
            rank: u32::try_from(offset).unwrap_or(u32::MAX).saturating_add(index as u32 + 1),
            run_uid: row.run_uid,
            key_level: u32::try_from(row.key_level).unwrap_or(0),
            outcome: RunOutcome::from_key(&row.outcome).unwrap_or(RunOutcome::Overtime),
            duration_us: row.duration_us,
            timer_us: row.timer_us,
            finished_at_us: row.finished_at_us,
            party: lenient::<RunMember>(row.party.0),
        })
        .collect();
    Ok(LeaderboardPage {
        board: query.board.key().to_owned(),
        dungeon_id: query.dungeon_id.to_owned(),
        key_level: key,
        season_id: query.season_id.to_owned(),
        page: query.page,
        page_size,
        rows,
        has_more,
    })
}

/// Whether a season exists.
///
/// # Errors
///
/// The database failed.
pub async fn season_exists(pool: &PgPool, season_id: &str) -> Result<bool, AppError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM game.seasons WHERE id = $1)")
        .bind(season_id)
        .fetch_one(pool)
        .await
        .map_err(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_realm::meter::DamageReport;

    fn result() -> DungeonResult {
        DungeonResult {
            result_id: "ab:acct".into(),
            run_uid: "ab".into(),
            account_id: "acct".into(),
            character_id: String::new(),
            dungeon_id: "graveyard_crypt".into(),
            success: true,
            duration_us: 5,
            experience: 0,
            key_level: 3,
            key_owner: false,
            key_level_after: 0,
            resources: Vec::new(),
            damage_report: None,
            talent_points: 0,
            looted: Vec::new(),
            currency_spent: Vec::new(),
            participant_key_bump: false,
            own_key_level: 0,
            outcome: Some(RunOutcome::Timed),
            timer_us: 10,
            party: vec![RunMember {
                character_id: "c".into(),
                name: "Ash".into(),
                hero: "Fire Mage".into(),
                role: "dps".into(),
            }],
            modifiers: Vec::new(),
        }
    }

    #[test]
    fn a_result_with_a_party_and_a_uid_writes_the_run() {
        let row = run_row(&result()).expect("a row");
        assert_eq!((row.run_uid.as_str(), row.key_level, row.timer_us), ("ab", 3, 10));
        assert_eq!(row.outcome, RunOutcome::Timed);
        assert_eq!(row.party.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn a_leavers_or_a_legacy_result_writes_no_run() {
        let leaver = DungeonResult {
            party: Vec::new(),
            ..result()
        };
        assert!(run_row(&leaver).is_none());
        let legacy = DungeonResult {
            run_uid: String::new(),
            ..result()
        };
        assert!(run_row(&legacy).is_none());
    }

    #[test]
    fn an_oversized_party_is_cut_and_a_negative_duration_is_zero() {
        let member = result().party[0].clone();
        let big = DungeonResult {
            party: vec![member; 20],
            duration_us: -5,
            ..result()
        };
        let row = run_row(&big).expect("a row");
        assert_eq!(row.party.as_array().map(Vec::len), Some(MAX_PARTY));
        assert_eq!(row.duration_us, 0);
    }

    #[test]
    fn stats_come_from_the_report_and_impossible_figures_are_dropped() {
        assert_eq!(run_stats(&result()), RunStats::default());
        let report = |dps: f32| DamageReport {
            party: Vec::new(),
            total_damage: 100,
            dps,
            duration_seconds: 10.0,
            boss_kill_seconds: None,
            abilities: Vec::new(),
            healing: 7,
            damage_taken: 9,
            deaths: 2,
        };
        let with = |dps| DungeonResult {
            damage_report: Some(report(dps)),
            ..result()
        };
        let stats = run_stats(&with(12.5));
        assert_eq!((stats.deaths, stats.damage_taken, stats.healing), (2, 9, 7));
        assert!((stats.dps - 12.5).abs() < f32::EPSILON);
        assert_eq!(run_stats(&with(f32::NAN)).dps, 0.0);
        assert_eq!(run_stats(&with(-1.0)).dps, 0.0);
    }

    #[test]
    fn a_loot_id_names_its_result() {
        assert_eq!(result_id_of_loot("ab:acct:loot:3"), Some("ab:acct"));
        assert_eq!(result_id_of_loot("ab:acct:mob:0"), Some("ab:acct"));
        assert_eq!(result_id_of_loot("ab:acct"), None);
        assert_eq!(result_id_of_loot(":loot:1"), None);
    }
}
