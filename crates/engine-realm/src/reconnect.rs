//! The party's side of a dropped connection: who is told, and how long they have.
//!
//! The server never removes a disconnected member before the grace ends (see
//! `instance::MemberPresence::expired`). Meanwhile every *other* connected member of a party in
//! a run is shown "<Name> has disconnected" with a countdown to that instant and the choice to
//! wait or leave. This module is the pure part of that: given the party as the viewer sees it
//! and the server's clock, which members to list and how many seconds each has left.
//!
//! The countdown is `disconnected_at + grace`, both written by the server on the member's row;
//! nothing here counts on its own.

use crate::party::PartyState;
use crate::{Micros, SECOND};

/// One party member, as the prompt needs to see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenMember {
    /// Their account.
    pub account: String,
    /// Connected.
    pub present: bool,
    /// When the server saw them drop, while they are away.
    pub disconnected_at: Option<Micros>,
    /// How long their place is held after that.
    pub grace: Micros,
}

/// A member the party is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// Their account.
    pub account: String,
    /// Identifies this particular drop: a member who returns and drops again gets a new key,
    /// so a "Wait" answered to the first one does not hide the second.
    pub key: String,
    /// Whole seconds left before they are removed, rounded up.
    pub seconds_left: u64,
}

/// Microseconds of grace left at `now` (never negative).
#[must_use]
pub fn grace_left(now: Micros, disconnected_at: Micros, grace: Micros) -> Micros {
    (disconnected_at + grace - now).max(0)
}

/// Whole seconds of grace left, rounded up so the last second shows 1, not 0.
#[must_use]
pub fn grace_seconds_left(now: Micros, disconnected_at: Micros, grace: Micros) -> u64 {
    let left = grace_left(now, disconnected_at, grace);
    u64::try_from((left + SECOND - 1) / SECOND).unwrap_or(0)
}

/// Whether a party in `state` shows the disconnect prompt at all: only while a run is starting
/// or under way. A finished run has nothing left to wait for.
#[must_use]
pub fn prompt_applies(state: PartyState) -> bool {
    matches!(state, PartyState::Starting | PartyState::InDungeon)
}

/// The members `viewer` should be told are away, in party order.
///
/// Empty unless the viewer is themselves a connected member of a party in a run. A member whose
/// grace has run out is not listed: the janitor is about to remove them and there is nothing
/// left to wait for. The viewer is never listed to themselves.
#[must_use]
pub fn prompt_for(
    state: PartyState,
    viewer: &str,
    members: &[SeenMember],
    now: Micros,
) -> Vec<Waiting> {
    if !prompt_applies(state) {
        return Vec::new();
    }
    let viewer_present = members
        .iter()
        .any(|member| member.account == viewer && member.present);
    if !viewer_present {
        return Vec::new();
    }
    members
        .iter()
        .filter(|member| member.account != viewer && !member.present)
        .filter_map(|member| {
            let since = member.disconnected_at?;
            (grace_left(now, since, member.grace) > 0).then(|| Waiting {
                account: member.account.clone(),
                key: format!("{}@{since}", member.account),
                seconds_left: grace_seconds_left(now, since, member.grace),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Micros = 300 * SECOND;

    fn here(account: &str) -> SeenMember {
        SeenMember {
            account: account.into(),
            present: true,
            disconnected_at: None,
            grace: GRACE,
        }
    }

    fn away(account: &str, since: Micros) -> SeenMember {
        SeenMember {
            account: account.into(),
            present: false,
            disconnected_at: Some(since),
            grace: GRACE,
        }
    }

    #[test]
    fn countdown_rounds_up_and_stops_at_zero() {
        assert_eq!(grace_seconds_left(0, 0, GRACE), 300);
        assert_eq!(grace_seconds_left(SECOND / 2, 0, GRACE), 300);
        assert_eq!(grace_seconds_left(SECOND, 0, GRACE), 299);
        assert_eq!(grace_seconds_left(GRACE - 1, 0, GRACE), 1);
        assert_eq!(grace_seconds_left(GRACE, 0, GRACE), 0);
        assert_eq!(grace_seconds_left(GRACE + 50 * SECOND, 0, GRACE), 0);
        assert_eq!(grace_left(GRACE + 1, 0, GRACE), 0);
    }

    #[test]
    fn the_countdown_follows_the_servers_timestamps_not_the_clients() {
        // Dropped at t=100s with a 60s grace: 45s left at t=115s.
        assert_eq!(
            grace_seconds_left(115 * SECOND, 100 * SECOND, 60 * SECOND),
            45
        );
    }

    #[test]
    fn present_members_see_who_is_away() {
        let members = [here("tank"), away("healer", 10 * SECOND), here("dps")];
        for viewer in ["tank", "dps"] {
            let waiting = prompt_for(PartyState::InDungeon, viewer, &members, 20 * SECOND);
            assert_eq!(waiting.len(), 1, "{viewer}");
            assert_eq!(waiting[0].account, "healer");
            assert_eq!(waiting[0].seconds_left, 290);
        }
    }

    #[test]
    fn the_dropped_member_is_not_prompted_about_themselves() {
        let members = [here("tank"), away("healer", 0), here("dps")];
        assert!(prompt_for(PartyState::InDungeon, "healer", &members, SECOND).is_empty());
        // Even if their own mirror is stale, nobody is listed to a viewer who is not present.
        let both = [away("tank", 0), away("healer", 0), here("dps")];
        assert!(prompt_for(PartyState::InDungeon, "tank", &both, SECOND).is_empty());
    }

    #[test]
    fn the_prompt_is_for_a_run_under_way_only() {
        let members = [here("tank"), away("healer", 0), here("dps")];
        for state in PartyState::ALL {
            let shown = !prompt_for(state, "tank", &members, SECOND).is_empty();
            assert_eq!(
                shown,
                matches!(state, PartyState::Starting | PartyState::InDungeon),
                "{state:?}"
            );
        }
    }

    #[test]
    fn an_expired_grace_is_not_listed() {
        let members = [here("tank"), away("healer", 0), here("dps")];
        assert_eq!(
            prompt_for(PartyState::InDungeon, "tank", &members, GRACE - 1).len(),
            1
        );
        assert!(prompt_for(PartyState::InDungeon, "tank", &members, GRACE).is_empty());
    }

    #[test]
    fn two_away_are_both_listed_in_party_order_with_their_own_clocks() {
        let members = [away("tank", 0), here("healer"), away("dps", 100 * SECOND)];
        let waiting = prompt_for(PartyState::InDungeon, "healer", &members, 150 * SECOND);
        let seen: Vec<(&str, u64)> = waiting
            .iter()
            .map(|w| (w.account.as_str(), w.seconds_left))
            .collect();
        assert_eq!(seen, [("tank", 150), ("dps", 250)]);
    }

    #[test]
    fn a_second_drop_is_a_new_prompt() {
        let first = [here("tank"), away("healer", 10 * SECOND), here("dps")];
        let second = [here("tank"), away("healer", 90 * SECOND), here("dps")];
        let a = prompt_for(PartyState::InDungeon, "tank", &first, 100 * SECOND);
        let b = prompt_for(PartyState::InDungeon, "tank", &second, 100 * SECOND);
        assert_ne!(a[0].key, b[0].key);
    }

    #[test]
    fn a_member_with_no_drop_time_is_not_listed() {
        let mut odd = away("healer", 0);
        odd.disconnected_at = None;
        let members = [here("tank"), odd, here("dps")];
        assert!(prompt_for(PartyState::InDungeon, "tank", &members, SECOND).is_empty());
    }
}
