//! A dungeon instance's life: lifecycle, countdown, barrier, and when it is cleaned up.
//!
//! ```text
//! Created ─▶ Loading ─▶ Ready ─▶ Countdown ─▶ Active ─┬─▶ Completed ─┐
//!                                                     └─▶ Failed ────┤
//!   (any state, when nobody can come back) ───────────────────────────┴─▶ CleaningUp ─▶ Destroyed
//! ```
//!
//! Every party gets its own instance; two parties in the same dungeon share nothing but the
//! dungeon's *definition*. The barrier is a function of the state ([`barrier_closed`]) rather
//! than a flag beside it, so it cannot be open during a countdown or shut during a run.
//!
//! The countdown is a server timestamp ([`Countdown::ends_at`]). Clients render
//! [`Countdown::display`] from their estimate of the server's clock; none of them counts down
//! on its own, so three clients show the same number and the barrier opens when the server
//! says, not when the fastest client reaches zero.

use crate::{Micros, SECOND};

/// Where an instance is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceState {
    /// Rows exist; nothing is placed yet.
    Created,
    /// The level and the party's snapshots are being placed.
    Loading,
    /// Everyone is placed behind the barrier.
    Ready,
    /// The leader started; counting down to the barrier lifting.
    Countdown,
    /// The barrier is open and the run is under way.
    Active,
    /// The dungeon was completed.
    Completed,
    /// The run failed.
    Failed,
    /// Being torn down; no input is accepted.
    CleaningUp,
    /// Gone. Its rows are deleted in the same transaction that enters this state.
    Destroyed,
}

impl InstanceState {
    /// Every state, in lifecycle order.
    pub const ALL: [Self; 9] = [
        Self::Created,
        Self::Loading,
        Self::Ready,
        Self::Countdown,
        Self::Active,
        Self::Completed,
        Self::Failed,
        Self::CleaningUp,
        Self::Destroyed,
    ];

    /// Whether an instance in this state may move to `next`.
    #[must_use]
    pub fn can_become(self, next: Self) -> bool {
        use InstanceState::{
            Active, CleaningUp, Completed, Countdown, Created, Destroyed, Failed, Loading, Ready,
        };
        match (self, next) {
            (Created, Loading)
            | (Loading, Ready)
            | (Ready, Countdown)
            | (Countdown, Active)
            | (Active, Completed | Failed)
            | (CleaningUp, Destroyed) => true,
            (Destroyed | CleaningUp, _) => false,
            (_, CleaningUp) => true,
            _ => false,
        }
    }

    /// Whether players may move (inside the start area until the barrier opens).
    #[must_use]
    pub fn accepts_movement(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Countdown | Self::Active | Self::Completed | Self::Failed
        )
    }

    /// Whether combat is allowed.
    #[must_use]
    pub fn allows_combat(self) -> bool {
        self == Self::Active
    }

    /// The stable written name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Countdown => "countdown",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::CleaningUp => "cleaning_up",
            Self::Destroyed => "destroyed",
        }
    }
}

/// Whether the starting barrier stands in `state`.
#[must_use]
pub fn barrier_closed(state: InstanceState) -> bool {
    matches!(
        state,
        InstanceState::Created
            | InstanceState::Loading
            | InstanceState::Ready
            | InstanceState::Countdown
    )
}

/// What a countdown shows right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountdownDisplay {
    /// A whole number of seconds left, 1 or more.
    Seconds(u32),
    /// The barrier has lifted; shown for a moment.
    Go,
    /// Nothing to show.
    Hidden,
}

/// How long "GO" stays on screen after the barrier lifts.
pub const GO_DISPLAY: Micros = SECOND;

/// A countdown, by its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Countdown {
    /// When the barrier lifts, in server time.
    pub ends_at: Micros,
}

impl Countdown {
    /// A countdown of `seconds` starting at `now`.
    #[must_use]
    pub fn starting(now: Micros, seconds: u32) -> Self {
        Self {
            ends_at: now + i64::from(seconds) * SECOND,
        }
    }

    /// Whether it has finished at `now`.
    #[must_use]
    pub fn finished(&self, now: Micros) -> bool {
        now >= self.ends_at
    }

    /// What to show at `now`: 5, 4, 3, 2, 1, GO.
    ///
    /// Rounded up, so the first second shows the full count and "1" is on screen for the last
    /// whole second rather than "0".
    #[must_use]
    pub fn display(&self, now: Micros) -> CountdownDisplay {
        let left = self.ends_at - now;
        if left > 0 {
            let seconds = (left + SECOND - 1) / SECOND;
            return CountdownDisplay::Seconds(u32::try_from(seconds).unwrap_or(u32::MAX));
        }
        if -left < GO_DISPLAY {
            CountdownDisplay::Go
        } else {
            CountdownDisplay::Hidden
        }
    }
}

/// Instance tunables, stored in the module's config table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceConfig {
    /// How long a disconnected member's place is held.
    pub reconnect_grace: Micros,
    /// How long a finished instance stays up so its members can read the result.
    pub finished_linger: Micros,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            reconnect_grace: 300 * SECOND,
            finished_linger: 120 * SECOND,
        }
    }
}

/// A member, as the janitor sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberPresence {
    /// `None` while connected; when they disconnected otherwise.
    pub disconnected_at: Option<Micros>,
}

impl MemberPresence {
    /// Whether their place has expired at `now`.
    #[must_use]
    pub fn expired(&self, now: Micros, config: &InstanceConfig) -> bool {
        self.disconnected_at
            .is_some_and(|since| now - since >= config.reconnect_grace)
    }
}

/// Whether an instance should be torn down at `now`.
///
/// An instance is kept while anyone is connected or could still reconnect. It is torn down
/// when every remaining member's grace has expired (or nobody is left), or when a finished
/// run has lingered long enough. A disconnect alone never ends a run.
#[must_use]
pub fn should_clean_up(
    state: InstanceState,
    members: &[MemberPresence],
    finished_at: Option<Micros>,
    now: Micros,
    config: &InstanceConfig,
) -> bool {
    if matches!(state, InstanceState::CleaningUp | InstanceState::Destroyed) {
        return false;
    }
    if members.iter().all(|member| member.expired(now, config)) {
        return true;
    }
    matches!(state, InstanceState::Completed | InstanceState::Failed)
        && finished_at.is_some_and(|at| now - at >= config.finished_linger)
}

/// An instance's uid as text: 32 lower-case hex digits. Persistence keys carry this, never the
/// realm's auto-increment id, because realm data is wiped on a schema change and the ids
/// restart; the uid is 128 random bits drawn when the instance is created.
#[must_use]
pub fn uid_hex(uid: u128) -> String {
    format!("{uid:032x}")
}

/// The idempotency key of one player's result in one run: `"<uid>:<account id>"`.
#[must_use]
pub fn run_result_id(uid: u128, account_id: &str) -> String {
    format!("{}:{account_id}", uid_hex(uid))
}

/// The seed the whole authoritative fight rolls from (crits, enemy decisions, loot), folded
/// from the instance's uid. Fixed for the instance's lifetime, so a rebuilt fight rolls what
/// the original would have; unpredictable before the instance exists, because the uid is.
#[must_use]
pub fn fight_seed(uid: u128) -> u64 {
    // Both halves, through SplitMix64 finalisers, so neither alone decides the seed.
    let mix = |mut z: u64| {
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the two halves are the point"
    )]
    let (high, low) = ((uid >> 64) as u64, uid as u64);
    mix(mix(low).wrapping_add(high.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_order() {
        use InstanceState::*;
        let path = [Created, Loading, Ready, Countdown, Active, Completed];
        for pair in path.windows(2) {
            assert!(
                pair[0].can_become(pair[1]),
                "{:?} -> {:?}",
                pair[0],
                pair[1]
            );
        }
        assert!(!Created.can_become(Active), "no skipping the countdown");
        assert!(!Countdown.can_become(Completed));
        for state in InstanceState::ALL {
            if state != Destroyed && state != CleaningUp {
                assert!(state.can_become(CleaningUp));
            }
            assert!(!Destroyed.can_become(state));
        }
        assert!(CleaningUp.can_become(Destroyed));
    }

    #[test]
    fn the_barrier_follows_the_state() {
        assert!(barrier_closed(InstanceState::Ready));
        assert!(barrier_closed(InstanceState::Countdown));
        assert!(!barrier_closed(InstanceState::Active));
        assert!(!InstanceState::Countdown.allows_combat());
        assert!(InstanceState::Active.allows_combat());
    }

    #[test]
    fn countdown_shows_five_to_go() {
        let countdown = Countdown::starting(0, 5);
        assert_eq!(countdown.display(0), CountdownDisplay::Seconds(5));
        assert_eq!(countdown.display(SECOND / 2), CountdownDisplay::Seconds(5));
        assert_eq!(countdown.display(SECOND), CountdownDisplay::Seconds(4));
        assert_eq!(
            countdown.display(4 * SECOND + 1),
            CountdownDisplay::Seconds(1)
        );
        assert_eq!(countdown.display(5 * SECOND), CountdownDisplay::Go);
        assert_eq!(countdown.display(6 * SECOND), CountdownDisplay::Hidden);
        assert!(!countdown.finished(5 * SECOND - 1));
        assert!(countdown.finished(5 * SECOND));
    }

    #[test]
    fn a_disconnect_does_not_end_the_run() {
        let config = InstanceConfig::default();
        let members = [
            MemberPresence {
                disconnected_at: Some(0),
            },
            MemberPresence {
                disconnected_at: None,
            },
        ];
        assert!(!should_clean_up(
            InstanceState::Active,
            &members,
            None,
            10_000 * SECOND,
            &config
        ));
    }

    #[test]
    fn everyone_gone_past_grace_cleans_up() {
        let config = InstanceConfig::default();
        let members = [
            MemberPresence {
                disconnected_at: Some(0),
            },
            MemberPresence {
                disconnected_at: Some(10 * SECOND),
            },
        ];
        assert!(!should_clean_up(
            InstanceState::Active,
            &members,
            None,
            config.reconnect_grace,
            &config
        ));
        assert!(should_clean_up(
            InstanceState::Active,
            &members,
            None,
            config.reconnect_grace + 10 * SECOND,
            &config
        ));
        assert!(should_clean_up(
            InstanceState::Active,
            &[],
            None,
            0,
            &config
        ));
    }

    #[test]
    fn finished_runs_linger_then_clean_up() {
        let config = InstanceConfig::default();
        let present = [MemberPresence {
            disconnected_at: None,
        }];
        assert!(!should_clean_up(
            InstanceState::Completed,
            &present,
            Some(0),
            config.finished_linger - 1,
            &config
        ));
        assert!(should_clean_up(
            InstanceState::Completed,
            &present,
            Some(0),
            config.finished_linger,
            &config
        ));
    }

    #[test]
    fn a_uid_names_the_run_and_seeds_the_fight() {
        let (a, b) = (0x1234_u128 << 64 | 7, 0x1234_u128 << 64 | 8);
        assert_eq!(uid_hex(a), "00000000000012340000000000000007");
        assert_eq!(
            run_result_id(a, "acct"),
            "00000000000012340000000000000007:acct"
        );
        assert_ne!(run_result_id(a, "acct"), run_result_id(b, "acct"));
        assert_eq!(fight_seed(a), fight_seed(a), "stable for one instance");
        assert_ne!(fight_seed(a), fight_seed(b), "different instances differ");
        // Either half alone changes the seed.
        assert_ne!(fight_seed(a), fight_seed(a ^ (1 << 100)));
    }
}
