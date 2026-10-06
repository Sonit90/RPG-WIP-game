//! Which queued tickets become a party.
//!
//! # The rule
//!
//! A party is exactly one Tank, one Healer and one DPS who queued for the same dungeon, from
//! three different accounts, whose gear scores are close enough. "Close enough" starts at
//! ±10 % and widens with how long the *oldest* member has waited:
//!
//! ```text
//! tolerance
//!   20% ┤                 ┌──────────────  max_tolerance
//!       │               ╱
//!       │             ╱     linear between expand_start and expand_end
//!   10% ┤────────────╯                     min_tolerance
//!       └────────────┬────┬──────────────▶ seconds waited
//!                   30   60
//! ```
//!
//! # The algorithm
//!
//! Per dungeon, eligible tickets are taken oldest first (ties broken by ticket id). Each one
//! in turn is the **anchor**: its tolerance is the one that applies. A candidate of each other
//! role must be within that tolerance of the anchor, and the trio's whole spread —
//! `(max − min) / min` — must be within it too, so two candidates on opposite sides of the
//! anchor cannot stretch a party to twice the tolerance. Of every trio that qualifies, the one
//! with the smallest spread wins; ties go to the trio that has waited longest, then to the
//! lowest ticket ids. Matched tickets leave the pool and the next unmatched anchor is tried.
//!
//! Oldest-first is what stops anyone waiting forever: the longest-waiting player is always
//! the first to be offered a party, and their widening tolerance is what eventually admits
//! one. Smallest-spread is what keeps parties fair when the queue is busy.
//!
//! Everything is a pure function of the tickets and `now`, iterated in a fixed order, so two
//! runs over the same queue form the same parties. A role with nobody in it forms nothing:
//! there is no partial party, no role replacement and no bot.

use std::collections::{BTreeMap, BTreeSet};

use crate::role::Role;
use crate::{Micros, SECOND};

/// How tolerance widens with time. Stored in the module's config table, so it can be tuned on
/// a live server without a deploy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchmakingConfig {
    /// Tolerance before any widening, as a fraction (0.10 is ±10 %).
    pub min_tolerance: f64,
    /// The widest it gets.
    pub max_tolerance: f64,
    /// How long the anchor waits before it starts widening.
    pub expand_start: Micros,
    /// When it reaches [`MatchmakingConfig::max_tolerance`].
    pub expand_end: Micros,
    /// How many candidates of each role, nearest in gear score, an anchor considers. Bounds
    /// the work per anchor on a very busy queue without changing who is preferred.
    pub candidate_limit: usize,
}

impl Default for MatchmakingConfig {
    fn default() -> Self {
        Self {
            min_tolerance: 0.10,
            max_tolerance: 0.20,
            expand_start: 30 * SECOND,
            expand_end: 60 * SECOND,
            candidate_limit: 32,
        }
    }
}

impl MatchmakingConfig {
    /// The tolerance after waiting `waited`.
    #[must_use]
    pub fn tolerance(&self, waited: Micros) -> f64 {
        let min = self.min_tolerance.max(0.0);
        let max = self.max_tolerance.max(min);
        if waited <= self.expand_start {
            return min;
        }
        let span = self.expand_end - self.expand_start;
        if span <= 0 || waited >= self.expand_end {
            return max;
        }
        // Both are positive and span is not zero.
        #[allow(
            clippy::cast_precision_loss,
            reason = "microsecond spans are far below 2^52"
        )]
        let t = (waited - self.expand_start) as f64 / span as f64;
        min + (max - min) * t
    }

    /// Whether the configuration is usable: tolerances in [0, 1], ordered, a sane window.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.min_tolerance)
            && (0.0..=1.0).contains(&self.max_tolerance)
            && self.min_tolerance <= self.max_tolerance
            && self.expand_start >= 0
            && self.expand_end >= self.expand_start
            && self.candidate_limit > 0
    }
}

/// One queued character.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueTicket {
    /// The ticket's id. Unique; the final tie-break.
    pub id: u64,
    /// Who queued. A party never holds one account twice.
    pub account: String,
    /// Which dungeon.
    pub dungeon_id: String,
    /// As which role.
    pub role: Role,
    /// Authoritative gear score, from the persistence API.
    pub gear_score: u32,
    /// When it was queued. Kept across a cancelled ready check so a re-queued player keeps
    /// their place.
    pub enqueued_at: Micros,
    /// False while the player is disconnected or already in a ready check.
    pub eligible: bool,
}

/// Three tickets that make a party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedParty {
    /// The dungeon they queued for.
    pub dungeon_id: String,
    /// Ticket ids in role order: Tank, Healer, DPS.
    pub tickets: [u64; 3],
}

/// The relative difference between two gear scores, against the smaller.
fn relative_gap(a: u32, b: u32) -> f64 {
    let (low, high) = if a <= b { (a, b) } else { (b, a) };
    f64::from(high - low) / f64::from(low.max(1))
}

/// Every party the queue can form right now, in the order they were found.
#[must_use]
pub fn find_matches(
    tickets: &[QueueTicket],
    now: Micros,
    config: &MatchmakingConfig,
) -> Vec<ProposedParty> {
    // BTreeMap: dungeons are visited in a fixed order, which is part of determinism.
    let mut by_dungeon: BTreeMap<&str, Vec<&QueueTicket>> = BTreeMap::new();
    for ticket in tickets.iter().filter(|ticket| ticket.eligible) {
        by_dungeon
            .entry(ticket.dungeon_id.as_str())
            .or_default()
            .push(ticket);
    }

    let mut parties = Vec::new();
    for (dungeon_id, mut pool) in by_dungeon {
        pool.sort_by_key(|ticket| (ticket.enqueued_at, ticket.id));
        let mut used: BTreeSet<u64> = BTreeSet::new();

        for anchor in &pool {
            if used.contains(&anchor.id) {
                continue;
            }
            let tolerance = config.tolerance(now - anchor.enqueued_at);
            if let Some(trio) = best_trio(anchor, &pool, &used, tolerance, config) {
                used.extend(trio.iter().map(|ticket| ticket.id));
                let mut ids = [0_u64; 3];
                for ticket in trio {
                    ids[ticket.role.index()] = ticket.id;
                }
                parties.push(ProposedParty {
                    dungeon_id: dungeon_id.to_owned(),
                    tickets: ids,
                });
            }
        }
    }
    parties
}

/// The best trio around `anchor`, if any trio qualifies.
fn best_trio<'a>(
    anchor: &'a QueueTicket,
    pool: &[&'a QueueTicket],
    used: &BTreeSet<u64>,
    tolerance: f64,
    config: &MatchmakingConfig,
) -> Option<[&'a QueueTicket; 3]> {
    let others: Vec<Role> = Role::ALL
        .into_iter()
        .filter(|role| *role != anchor.role)
        .collect();
    let [first_role, second_role] = others[..] else {
        return None;
    };

    let candidates = |role: Role| -> Vec<&'a QueueTicket> {
        let mut found: Vec<&QueueTicket> = pool
            .iter()
            .copied()
            .filter(|ticket| {
                ticket.role == role
                    && !used.contains(&ticket.id)
                    && ticket.account != anchor.account
                    && relative_gap(ticket.gear_score, anchor.gear_score) <= tolerance
            })
            .collect();
        // Nearest first, then oldest, then id: the limit keeps the best ones.
        found.sort_by(|a, b| {
            relative_gap(a.gear_score, anchor.gear_score)
                .total_cmp(&relative_gap(b.gear_score, anchor.gear_score))
                .then(a.enqueued_at.cmp(&b.enqueued_at))
                .then(a.id.cmp(&b.id))
        });
        found.truncate(config.candidate_limit);
        found
    };

    let firsts = candidates(first_role);
    let seconds = candidates(second_role);

    let mut best: Option<(f64, Micros, [u64; 3], [&QueueTicket; 3])> = None;
    for first in &firsts {
        for second in &seconds {
            if first.account == second.account {
                continue;
            }
            let scores = [anchor.gear_score, first.gear_score, second.gear_score];
            let low = scores.iter().copied().min().unwrap_or(0);
            let high = scores.iter().copied().max().unwrap_or(0);
            let spread = relative_gap(low, high);
            if spread > tolerance {
                continue;
            }
            let waited = anchor.enqueued_at + first.enqueued_at + second.enqueued_at;
            let mut ids = [anchor.id, first.id, second.id];
            ids.sort_unstable();
            let better = match &best {
                None => true,
                Some((best_spread, best_waited, best_ids, _)) => spread
                    .total_cmp(best_spread)
                    .then(waited.cmp(best_waited))
                    .then(ids.cmp(best_ids))
                    .is_lt(),
            };
            if better {
                best = Some((spread, waited, ids, [anchor, *first, *second]));
            }
        }
    }
    best.map(|(_, _, _, trio)| trio)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    const DUNGEON: &str = "graveyard_crypt";

    fn ticket(id: u64, role: Role, gear_score: u32, enqueued_at: Micros) -> QueueTicket {
        QueueTicket {
            id,
            account: format!("account-{id}"),
            dungeon_id: DUNGEON.into(),
            role,
            gear_score,
            enqueued_at,
            eligible: true,
        }
    }

    fn config() -> MatchmakingConfig {
        MatchmakingConfig::default()
    }

    #[test]
    fn exact_one_one_one_matches() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 100, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        let parties = find_matches(&tickets, SECOND, &config());
        assert_eq!(
            parties,
            vec![ProposedParty {
                dungeon_id: DUNGEON.into(),
                tickets: [1, 2, 3]
            }]
        );
    }

    #[test]
    fn a_missing_role_forms_nothing() {
        // Two DPS and a tank, no healer: wait.
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Dps, 100, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        assert!(find_matches(&tickets, 600 * SECOND, &config()).is_empty());
    }

    #[test]
    fn within_ten_percent_matches_immediately() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 110, 0),
            ticket(3, Role::Dps, 105, 0),
        ];
        assert_eq!(find_matches(&tickets, 0, &config()).len(), 1);
    }

    #[test]
    fn beyond_ten_percent_is_rejected_at_first() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 115, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        assert!(find_matches(&tickets, 10 * SECOND, &config()).is_empty());
    }

    #[test]
    fn tolerance_widens_gradually() {
        let config = config();
        assert!((config.tolerance(0) - 0.10).abs() < 1e-9);
        assert!((config.tolerance(30 * SECOND) - 0.10).abs() < 1e-9);
        assert!((config.tolerance(45 * SECOND) - 0.15).abs() < 1e-9);
        assert!((config.tolerance(60 * SECOND) - 0.20).abs() < 1e-9);
        assert!((config.tolerance(3600 * SECOND) - 0.20).abs() < 1e-9);
        // Monotonic, and never a jump.
        let mut last = config.tolerance(0);
        for second in 0..120 {
            let now = config.tolerance(second * SECOND);
            assert!(now >= last);
            assert!(now - last <= 0.004 + 1e-9);
            last = now;
        }
    }

    #[test]
    fn a_fifteen_percent_gap_matches_once_the_anchor_has_waited() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 115, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        assert!(find_matches(&tickets, 40 * SECOND, &config()).is_empty());
        assert_eq!(find_matches(&tickets, 45 * SECOND, &config()).len(), 1);
    }

    #[test]
    fn twenty_percent_is_the_ceiling() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 121, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        assert!(find_matches(&tickets, 86_400 * SECOND, &config()).is_empty());
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 120, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        assert_eq!(find_matches(&tickets, 86_400 * SECOND, &config()).len(), 1);
    }

    #[test]
    fn the_spread_is_bounded_not_just_each_gap() {
        // 92 and 108 are each within 10% of 100 but 17% from each other.
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 92, 0),
            ticket(3, Role::Dps, 108, 0),
        ];
        assert!(find_matches(&tickets, 0, &config()).is_empty());
    }

    #[test]
    fn closer_gear_is_preferred() {
        let tickets = [
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 108, 0),
            ticket(3, Role::Healer, 101, 1),
            ticket(4, Role::Dps, 100, 0),
        ];
        let parties = find_matches(&tickets, 2, &config());
        assert_eq!(parties.len(), 1);
        assert_eq!(parties[0].tickets, [1, 3, 4]);
    }

    #[test]
    fn the_oldest_ticket_is_served_first() {
        // Two tanks fit; the one waiting longer gets the party.
        let tickets = [
            ticket(1, Role::Tank, 100, 5 * SECOND),
            ticket(2, Role::Tank, 100, 0),
            ticket(3, Role::Healer, 100, 6 * SECOND),
            ticket(4, Role::Dps, 100, 7 * SECOND),
        ];
        let parties = find_matches(&tickets, 10 * SECOND, &config());
        assert_eq!(parties.len(), 1);
        assert_eq!(parties[0].tickets, [2, 3, 4]);
    }

    #[test]
    fn several_parties_form_at_once() {
        let mut tickets = Vec::new();
        for i in 0..3 {
            let base = 100 + i * 50;
            tickets.push(ticket(u64::from(i) * 3 + 1, Role::Tank, base, 0));
            tickets.push(ticket(u64::from(i) * 3 + 2, Role::Healer, base, 0));
            tickets.push(ticket(u64::from(i) * 3 + 3, Role::Dps, base, 0));
        }
        let parties = find_matches(&tickets, 0, &config());
        assert_eq!(parties.len(), 3);
        let mut all: Vec<u64> = parties.iter().flat_map(|party| party.tickets).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 9, "no ticket is in two parties");
    }

    #[test]
    fn dungeons_never_mix() {
        let mut tickets = vec![
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 100, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        tickets[2].dungeon_id = "another".into();
        assert!(find_matches(&tickets, 0, &config()).is_empty());
        tickets.push(QueueTicket {
            dungeon_id: "another".into(),
            ..ticket(4, Role::Tank, 100, 0)
        });
        tickets.push(QueueTicket {
            dungeon_id: "another".into(),
            ..ticket(5, Role::Healer, 100, 0)
        });
        tickets.push(ticket(6, Role::Dps, 100, 0));
        let parties = find_matches(&tickets, 0, &config());
        assert_eq!(parties.len(), 2);
        for party in &parties {
            for id in party.tickets {
                let ticket = tickets
                    .iter()
                    .find(|ticket| ticket.id == id)
                    .expect("matched ids exist");
                assert_eq!(ticket.dungeon_id, party.dungeon_id);
            }
        }
    }

    #[test]
    fn ineligible_tickets_are_skipped() {
        // A disconnected player, or one already in a ready check, is not offered a party.
        let mut tickets = vec![
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 100, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        tickets[1].eligible = false;
        assert!(find_matches(&tickets, 0, &config()).is_empty());
    }

    #[test]
    fn one_account_never_fills_two_roles() {
        let mut tickets = vec![
            ticket(1, Role::Tank, 100, 0),
            ticket(2, Role::Healer, 100, 0),
            ticket(3, Role::Dps, 100, 0),
        ];
        tickets[2].account = tickets[0].account.clone();
        assert!(find_matches(&tickets, 0, &config()).is_empty());
    }

    #[test]
    fn matching_is_deterministic_and_order_independent() {
        let mut tickets = Vec::new();
        for id in 0..60_u64 {
            let role = Role::ALL[usize::try_from(id % 3).expect("small")];
            let gear = 80 + u32::try_from((id * 37) % 60).expect("small");
            tickets.push(ticket(
                id + 1,
                role,
                gear,
                i64::try_from(id).expect("small") * SECOND,
            ));
        }
        let now = 90 * SECOND;
        let first = find_matches(&tickets, now, &config());
        tickets.reverse();
        let second = find_matches(&tickets, now, &config());
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn zero_gear_scores_are_handled() {
        let tickets = [
            ticket(1, Role::Tank, 0, 0),
            ticket(2, Role::Healer, 0, 0),
            ticket(3, Role::Dps, 0, 0),
        ];
        assert_eq!(find_matches(&tickets, 0, &config()).len(), 1);
    }

    #[test]
    fn config_validation() {
        assert!(config().is_valid());
        let backwards = MatchmakingConfig {
            min_tolerance: 0.3,
            max_tolerance: 0.2,
            ..config()
        };
        assert!(!backwards.is_valid());
    }
}
