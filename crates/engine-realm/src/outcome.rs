//! How a run ended, and what it was worth against the dungeon's timer.
//!
//! ```text
//! success, duration <= timer  ─▶ Timed
//! success, duration >  timer  ─▶ Overtime
//! wipe / admin fail           ─▶ Failed
//! party left / all gone       ─▶ Abandoned
//! ```
//!
//! The realm classifies a run once, when it ends, from server time only; the persistence API
//! stores the answer and the leaderboards read it. A member who is merely disconnected and
//! still inside the reconnect grace has not abandoned anything: the janitor ends the run as
//! abandoned only after the grace has run out for everyone.

use serde::{Deserialize, Serialize};

use crate::catalog::DungeonDefinition;
use crate::{Micros, SECOND};

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// Completed inside the dungeon's timer.
    Timed,
    /// Completed after the timer ran out.
    Overtime,
    /// The party was wiped (or an operator failed the run).
    Failed,
    /// The party left, or everyone was gone past the reconnect grace.
    Abandoned,
}

impl RunOutcome {
    /// Every outcome.
    pub const ALL: [Self; 4] = [Self::Timed, Self::Overtime, Self::Failed, Self::Abandoned];

    /// The stable written name (also the database value).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Timed => "timed",
            Self::Overtime => "overtime",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }

    /// The outcome written as `key`.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|outcome| outcome.key() == key)
    }

    /// Whether the dungeon was completed.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Timed | Self::Overtime)
    }

    /// Whether the run may appear on a leaderboard: completed runs only.
    #[must_use]
    pub fn counts_for_boards(self) -> bool {
        self.is_success()
    }

    /// The outcome a result from before outcomes existed implies.
    #[must_use]
    pub fn legacy(success: bool) -> Self {
        if success { Self::Timed } else { Self::Failed }
    }
}

/// How the instance's life ended, before the timer is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The boss fell.
    Completed,
    /// The run was failed (a wipe, an operator).
    Failed,
    /// Nobody was left to finish it.
    Abandoned,
}

/// The dungeon's timer in microseconds; `None` when it has none.
#[must_use]
pub fn timer_us(definition: &DungeonDefinition) -> Option<Micros> {
    definition
        .timer_seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| i64::from(seconds) * SECOND)
}

/// Classifies a finished run. A dungeon with no timer can only be timed.
#[must_use]
pub fn classify(end: RunEnd, duration_us: Micros, timer_us: Option<Micros>) -> RunOutcome {
    match end {
        RunEnd::Failed => RunOutcome::Failed,
        RunEnd::Abandoned => RunOutcome::Abandoned,
        RunEnd::Completed => match timer_us {
            Some(timer) if duration_us > timer => RunOutcome::Overtime,
            _ => RunOutcome::Timed,
        },
    }
}

/// Microseconds left on the timer after `elapsed_us`; negative once it ran out (overtime).
#[must_use]
pub fn remaining_us(timer_us: Micros, elapsed_us: Micros) -> Micros {
    timer_us - elapsed_us
}

/// The timer as the HUD writes it: `mm:ss` while time remains, `+mm:ss` in overtime. Seconds
/// round up while counting down (the last second shows `00:01`) and down while counting up.
#[must_use]
pub fn format_timer(remaining_us: Micros) -> String {
    let (sign, seconds) = if remaining_us >= 0 {
        ("", (remaining_us + SECOND - 1) / SECOND)
    } else {
        ("+", (-remaining_us) / SECOND)
    };
    format!("{sign}{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    const MINUTE: Micros = 60 * SECOND;

    #[test]
    fn a_completion_inside_the_timer_is_timed_and_at_the_limit_too() {
        let timer = Some(25 * MINUTE);
        assert_eq!(classify(RunEnd::Completed, 10 * MINUTE, timer), RunOutcome::Timed);
        assert_eq!(classify(RunEnd::Completed, 25 * MINUTE, timer), RunOutcome::Timed);
    }

    #[test]
    fn a_completion_after_the_timer_is_overtime() {
        let timer = Some(25 * MINUTE);
        assert_eq!(
            classify(RunEnd::Completed, 25 * MINUTE + 1, timer),
            RunOutcome::Overtime
        );
    }

    #[test]
    fn a_dungeon_without_a_timer_is_always_timed() {
        assert_eq!(classify(RunEnd::Completed, i64::MAX, None), RunOutcome::Timed);
    }

    #[test]
    fn failure_and_abandonment_ignore_the_clock() {
        for duration in [0, MINUTE, 99 * MINUTE] {
            for timer in [None, Some(MINUTE)] {
                assert_eq!(classify(RunEnd::Failed, duration, timer), RunOutcome::Failed);
                assert_eq!(
                    classify(RunEnd::Abandoned, duration, timer),
                    RunOutcome::Abandoned
                );
            }
        }
    }

    #[test]
    fn keys_round_trip_and_only_completions_count_for_boards() {
        for outcome in RunOutcome::ALL {
            assert_eq!(RunOutcome::from_key(outcome.key()), Some(outcome));
        }
        assert_eq!(RunOutcome::from_key("won"), None);
        let counted: Vec<_> = RunOutcome::ALL
            .into_iter()
            .filter(|o| o.counts_for_boards())
            .collect();
        assert_eq!(counted, vec![RunOutcome::Timed, RunOutcome::Overtime]);
        assert_eq!(RunOutcome::legacy(true), RunOutcome::Timed);
        assert_eq!(RunOutcome::legacy(false), RunOutcome::Failed);
    }

    #[test]
    fn the_timer_counts_down_then_up() {
        let timer = 25 * MINUTE;
        assert_eq!(format_timer(remaining_us(timer, 0)), "25:00");
        assert_eq!(format_timer(remaining_us(timer, 1)), "25:00");
        assert_eq!(format_timer(remaining_us(timer, timer - SECOND / 2)), "00:01");
        assert_eq!(format_timer(remaining_us(timer, timer)), "00:00");
        assert_eq!(format_timer(remaining_us(timer, timer + 65 * SECOND)), "+01:05");
        assert_eq!(format_timer(remaining_us(timer, timer + 1)), "+00:00");
    }

    #[test]
    fn the_catalog_timers_become_microseconds() {
        let catalog = crate::catalog::Catalog::builtin().expect("catalog");
        let crypt = catalog.dungeon("graveyard_crypt").expect("crypt");
        assert_eq!(
            timer_us(crypt),
            crypt.timer_seconds.map(|seconds| i64::from(seconds) * SECOND)
        );
        assert!(timer_us(crypt).is_some_and(|timer| timer > 0));
    }
}
