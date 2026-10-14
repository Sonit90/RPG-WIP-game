//! Run anomalies: pure functions over rows already read from Postgres.
//!
//! Nothing here touches the database, so the thresholds and the arithmetic are tested
//! directly. `routes::ops` reads the rows and calls these.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::config::Anomaly;

/// One completed run, one row per run (not per player).
#[derive(Debug, Clone)]
pub struct RunRow {
    pub result_id: String,
    pub dungeon_id: String,
    pub key_level: i32,
    pub duration_seconds: f64,
    /// Unix seconds.
    pub completed_at: i64,
}

/// One player's damage report for a run (`game.dungeon_damage_reports`).
#[derive(Debug, Clone)]
pub struct DamageRow {
    pub instance_id: String,
    pub dungeon_id: String,
    pub key_level: i32,
    pub account_id: String,
    pub character_id: Option<String>,
    pub class: String,
    pub total_damage: i64,
    pub dps: f64,
    pub duration_seconds: f64,
    pub boss_kill_seconds: Option<f64>,
    pub completed_at: i64,
}

/// A run, as the report lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunEntry {
    pub result_id: String,
    pub dungeon_id: String,
    pub key_level: i32,
    pub duration_seconds: f64,
    pub completed_at: i64,
}

impl From<&RunRow> for RunEntry {
    fn from(run: &RunRow) -> Self {
        Self {
            result_id: run.result_id.clone(),
            dungeon_id: run.dungeon_id.clone(),
            key_level: run.key_level,
            duration_seconds: run.duration_seconds,
            completed_at: run.completed_at,
        }
    }
}

/// Fastest and slowest completed run of one dungeon at one key level.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Extremes {
    pub dungeon_id: String,
    pub key_level: i32,
    pub runs: usize,
    pub fastest: RunEntry,
    pub slowest: RunEntry,
}

/// A run that tripped a threshold, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlaggedRun {
    pub run: RunEntry,
    pub reasons: Vec<String>,
    /// The dungeon's median completed run, when it had enough samples.
    pub dungeon_median_seconds: Option<f64>,
}

/// What the completed runs say.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RunAnomalies {
    pub runs_considered: usize,
    pub extremes: Vec<Extremes>,
    pub too_fast: Vec<FlaggedRun>,
}

/// A damage report, as the report lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DamageEntry {
    pub instance_id: String,
    pub dungeon_id: String,
    pub key_level: i32,
    pub account_id: String,
    pub character_id: Option<String>,
    pub class: String,
    pub total_damage: i64,
    pub dps: f64,
    pub duration_seconds: f64,
    pub boss_kill_seconds: Option<f64>,
    pub completed_at: i64,
}

impl From<&DamageRow> for DamageEntry {
    fn from(row: &DamageRow) -> Self {
        Self {
            instance_id: row.instance_id.clone(),
            dungeon_id: row.dungeon_id.clone(),
            key_level: row.key_level,
            account_id: row.account_id.clone(),
            character_id: row.character_id.clone(),
            class: row.class.clone(),
            total_damage: row.total_damage,
            dps: row.dps,
            duration_seconds: row.duration_seconds,
            boss_kill_seconds: row.boss_kill_seconds,
            completed_at: row.completed_at,
        }
    }
}

/// What the damage reports say. Empty lists when no report was ever written.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DamageAnomalies {
    /// Whether the reports could be read at all.
    pub available: bool,
    /// Why not, when they could not.
    pub note: Option<String>,
    pub reports_considered: usize,
    /// Highest DPS first.
    pub highest_dps: Vec<DamageEntry>,
    /// Above `max_dps` or `max_total_damage`, highest first.
    pub suspicious_damage: Vec<DamageEntry>,
    /// Boss killed faster than `min_boss_kill_seconds`, fastest first.
    pub fast_boss_kills: Vec<DamageEntry>,
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Fastest/slowest per dungeon and key level, and the runs faster than the thresholds allow.
#[must_use]
pub fn run_anomalies(runs: &[RunRow], config: &Anomaly) -> RunAnomalies {
    let mut groups: BTreeMap<(&str, i32), Vec<&RunRow>> = BTreeMap::new();
    let mut by_dungeon: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for run in runs {
        groups
            .entry((run.dungeon_id.as_str(), run.key_level))
            .or_default()
            .push(run);
        by_dungeon
            .entry(run.dungeon_id.as_str())
            .or_default()
            .push(run.duration_seconds);
    }
    let medians: BTreeMap<&str, f64> = by_dungeon
        .into_iter()
        .filter(|(_, durations)| durations.len() >= config.relative_min_samples.max(1))
        .map(|(dungeon, mut durations)| {
            durations.sort_by(f64::total_cmp);
            (dungeon, median(&durations))
        })
        .collect();

    let extremes = groups
        .iter()
        .filter_map(|((dungeon, key_level), group)| {
            let fastest = group
                .iter()
                .min_by(|a, b| a.duration_seconds.total_cmp(&b.duration_seconds))?;
            let slowest = group
                .iter()
                .max_by(|a, b| a.duration_seconds.total_cmp(&b.duration_seconds))?;
            Some(Extremes {
                dungeon_id: (*dungeon).to_owned(),
                key_level: *key_level,
                runs: group.len(),
                fastest: RunEntry::from(*fastest),
                slowest: RunEntry::from(*slowest),
            })
        })
        .collect();

    let mut too_fast: Vec<FlaggedRun> = runs
        .iter()
        .filter_map(|run| {
            let mut reasons = Vec::new();
            if config.min_completion_seconds > 0.0
                && run.duration_seconds < config.min_completion_seconds
            {
                reasons.push(format!(
                    "{:.1}s is under the {:.0}s minimum",
                    run.duration_seconds, config.min_completion_seconds
                ));
            }
            let median = medians.get(run.dungeon_id.as_str()).copied();
            if let Some(median) = median {
                if config.relative_fast_percent > 0.0
                    && run.duration_seconds < median * config.relative_fast_percent / 100.0
                {
                    reasons.push(format!(
                        "{:.1}s is under {:.0}% of the dungeon median ({median:.1}s)",
                        run.duration_seconds, config.relative_fast_percent
                    ));
                }
            }
            (!reasons.is_empty()).then(|| FlaggedRun {
                run: RunEntry::from(run),
                reasons,
                dungeon_median_seconds: median,
            })
        })
        .collect();
    too_fast.sort_by(|a, b| a.run.duration_seconds.total_cmp(&b.run.duration_seconds));
    too_fast.truncate(config.list_limit);

    RunAnomalies {
        runs_considered: runs.len(),
        extremes,
        too_fast,
    }
}

/// Highest DPS, suspicious damage and abnormally fast boss kills.
#[must_use]
pub fn damage_anomalies(rows: &[DamageRow], config: &Anomaly) -> DamageAnomalies {
    let limit = config.list_limit;
    let mut highest: Vec<&DamageRow> = rows.iter().collect();
    highest.sort_by(|a, b| b.dps.total_cmp(&a.dps));

    let mut suspicious: Vec<&DamageRow> = rows
        .iter()
        .filter(|r| r.dps > config.max_dps || r.total_damage > config.max_total_damage)
        .collect();
    suspicious.sort_by(|a, b| b.dps.total_cmp(&a.dps));

    let mut fast: Vec<&DamageRow> = rows
        .iter()
        .filter(|r| {
            r.boss_kill_seconds
                .is_some_and(|s| s < config.min_boss_kill_seconds)
        })
        .collect();
    fast.sort_by(|a, b| {
        a.boss_kill_seconds
            .unwrap_or(f64::MAX)
            .total_cmp(&b.boss_kill_seconds.unwrap_or(f64::MAX))
    });

    let entries = |list: Vec<&DamageRow>| -> Vec<DamageEntry> {
        list.into_iter()
            .take(limit)
            .map(DamageEntry::from)
            .collect()
    };
    DamageAnomalies {
        available: true,
        note: None,
        reports_considered: rows.len(),
        highest_dps: entries(highest),
        suspicious_damage: entries(suspicious),
        fast_boss_kills: entries(fast),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, dungeon: &str, key: i32, seconds: f64) -> RunRow {
        RunRow {
            result_id: id.into(),
            dungeon_id: dungeon.into(),
            key_level: key,
            duration_seconds: seconds,
            completed_at: 0,
        }
    }

    fn damage(dps: f64, total: i64, boss: Option<f64>) -> DamageRow {
        DamageRow {
            instance_id: "1".into(),
            dungeon_id: "d".into(),
            key_level: 1,
            account_id: "a".into(),
            character_id: None,
            class: "warrior".into(),
            total_damage: total,
            dps,
            duration_seconds: 300.0,
            boss_kill_seconds: boss,
            completed_at: 0,
        }
    }

    #[test]
    fn extremes_are_per_dungeon_and_key_level() {
        let runs = [
            run("1", "crypt", 1, 300.0),
            run("2", "crypt", 1, 200.0),
            run("3", "crypt", 1, 400.0),
            run("4", "crypt", 2, 500.0),
        ];
        let report = run_anomalies(&runs, &Anomaly::default());
        assert_eq!(report.runs_considered, 4);
        assert_eq!(report.extremes.len(), 2);
        let one = &report.extremes[0];
        assert_eq!(
            (one.dungeon_id.as_str(), one.key_level, one.runs),
            ("crypt", 1, 3)
        );
        assert_eq!(one.fastest.result_id, "2");
        assert_eq!(one.slowest.result_id, "3");
        assert_eq!(report.extremes[1].fastest.result_id, "4");
    }

    #[test]
    fn a_run_under_the_absolute_minimum_is_flagged() {
        let runs = [run("1", "crypt", 1, 20.0), run("2", "crypt", 1, 300.0)];
        let report = run_anomalies(&runs, &Anomaly::default());
        assert_eq!(report.too_fast.len(), 1);
        assert_eq!(report.too_fast[0].run.result_id, "1");
        assert!(report.too_fast[0].dungeon_median_seconds.is_none());
    }

    #[test]
    fn a_run_far_under_the_median_is_flagged_once_there_are_samples() {
        let mut runs: Vec<RunRow> = (0..6)
            .map(|i| run(&i.to_string(), "crypt", 1, 600.0))
            .collect();
        runs.push(run("fast", "crypt", 1, 200.0));
        let report = run_anomalies(&runs, &Anomaly::default());
        assert_eq!(report.too_fast.len(), 1);
        assert_eq!(report.too_fast[0].run.result_id, "fast");
        assert_eq!(report.too_fast[0].dungeon_median_seconds, Some(600.0));
    }

    #[test]
    fn too_few_samples_skip_the_relative_check() {
        let runs = [run("1", "crypt", 1, 600.0), run("2", "crypt", 1, 200.0)];
        assert!(run_anomalies(&runs, &Anomaly::default())
            .too_fast
            .is_empty());
    }

    #[test]
    fn a_zero_minimum_turns_the_absolute_check_off() {
        let config = Anomaly {
            min_completion_seconds: 0.0,
            ..Anomaly::default()
        };
        assert!(run_anomalies(&[run("1", "d", 1, 1.0)], &config)
            .too_fast
            .is_empty());
    }

    #[test]
    fn no_runs_is_an_empty_report() {
        assert_eq!(
            run_anomalies(&[], &Anomaly::default()),
            RunAnomalies::default()
        );
    }

    #[test]
    fn median_of_even_and_odd_counts() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 10.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
    }

    #[test]
    fn damage_reports_rank_and_flag() {
        let config = Anomaly::default();
        let rows = [
            damage(900.0, 100_000, Some(60.0)),
            damage(9_000.0, 200_000, Some(40.0)),
            damage(1_000.0, 9_000_000, Some(3.0)),
        ];
        let report = damage_anomalies(&rows, &config);
        assert!(report.available);
        assert_eq!(report.reports_considered, 3);
        assert_eq!(report.highest_dps[0].dps, 9_000.0);
        assert_eq!(report.suspicious_damage.len(), 2);
        assert_eq!(report.fast_boss_kills.len(), 1);
        assert_eq!(report.fast_boss_kills[0].boss_kill_seconds, Some(3.0));
    }

    #[test]
    fn lists_respect_the_limit() {
        let config = Anomaly {
            list_limit: 2,
            ..Anomaly::default()
        };
        let rows: Vec<DamageRow> = (0..5).map(|i| damage(f64::from(i), 1, None)).collect();
        assert_eq!(damage_anomalies(&rows, &config).highest_dps.len(), 2);
    }
}
