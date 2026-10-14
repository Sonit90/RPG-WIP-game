//! The party: its state machine, its ready check, and who may do what to it.
//!
//! ```text
//!            all accept          a member inserts      key owner starts      countdown ends
//! ReadyCheck ──────────▶ ChoosingKey ──────────▶ Ready ────────────▶ Starting ─────────▶ InDungeon
//!     │                    │      ▲   key owner     │                   │                   │
//!     │ decline/timeout/   │      └── withdraws ────┤                   │ (a member leaves: │ success / wipe
//!     │ disconnect         │ member leaves/kicked   │                   │  the run          ▼
//!     ▼                    ▼ before the start       ▼                   │  continues)  Completed / Failed
//! Disbanded ◀──────────────┴────────────────────────┴───────────────────┘ everyone gone
//! ```
//!
//! There is no leader while the party is choosing a key: whoever inserts their own key becomes
//! the leader, and withdrawing it gives the leadership up again.
//!
//! States are one enum and every transition goes through [`PartyState::can_become`], so there
//! is no combination of booleans in which a party is both "starting" and "disbanded".
//!
//! Who may do what is [`authorize`]: one function, given the party as plain data, the actor
//! and the action. The module calls it before every party mutation and refuses with its error.
//! Being the leader is a field the server set (the key owner before the run, [`successor`]
//! from the server's own random roll during it); nothing a client sends can make it so.

use crate::error::RealmError;
use crate::role::Role;
use crate::{Micros, SECOND};

/// Where a party is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartyState {
    /// Formed by the matchmaker; waiting for every member to accept.
    ReadyCheck,
    /// Everyone accepted; waiting for a member to insert their dungeon key. No leader yet.
    ChoosingKey,
    /// A key is inserted and its owner leads; waiting for the leader to start.
    Ready,
    /// The instance exists and is counting down.
    Starting,
    /// The barrier is open and the run is under way.
    InDungeon,
    /// The dungeon was completed.
    Completed,
    /// The run failed or was abandoned.
    Failed,
    /// The party no longer exists for its members.
    Disbanded,
}

impl PartyState {
    /// Every state.
    pub const ALL: [Self; 8] = [
        Self::ReadyCheck,
        Self::ChoosingKey,
        Self::Ready,
        Self::Starting,
        Self::InDungeon,
        Self::Completed,
        Self::Failed,
        Self::Disbanded,
    ];

    /// Whether a party in this state may move to `next`.
    #[must_use]
    pub fn can_become(self, next: Self) -> bool {
        use PartyState::{
            ChoosingKey, Completed, Disbanded, Failed, InDungeon, Ready, ReadyCheck, Starting,
        };
        matches!(
            (self, next),
            (ReadyCheck, ChoosingKey | Disbanded)
                | (ChoosingKey, Ready | Disbanded)
                | (Ready, ChoosingKey | Starting | Disbanded)
                | (Starting, InDungeon | Failed | Disbanded)
                | (InDungeon, Completed | Failed | Disbanded)
                | (Completed | Failed, Disbanded)
        )
    }

    /// Whether the party is over.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Disbanded)
    }

    /// Whether the party is inside (or entering) an instance.
    #[must_use]
    pub fn is_in_instance(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::InDungeon | Self::Completed | Self::Failed
        )
    }

    /// The stable written name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::ReadyCheck => "ready_check",
            Self::ChoosingKey => "choosing_key",
            Self::Ready => "ready",
            Self::Starting => "starting",
            Self::InDungeon => "in_dungeon",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Disbanded => "disbanded",
        }
    }
}

/// A member's answer to the ready check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadyResponse {
    /// Not answered yet.
    Pending,
    /// Confirmed.
    Accepted,
    /// Declined.
    Declined,
}

/// Whether a member is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    /// Connected.
    Present,
    /// Lost their connection; held for the reconnect grace period.
    TemporarilyDisconnected,
}

/// One member, as the rules need to see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberView {
    /// Their account.
    pub account: String,
    /// The role they queued as.
    pub role: Role,
    /// Their ready-check answer.
    pub ready: ReadyResponse,
    /// Connected or not.
    pub presence: Presence,
    /// Whether their snapshotted key opens this party's dungeon at a valid level.
    pub has_key_for_dungeon: bool,
}

/// A party, as the rules need to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyView {
    /// Where it is in its life.
    pub state: PartyState,
    /// The leader's account, once there is one.
    pub leader: Option<String>,
    /// The account whose key is inserted, once one is.
    pub key_owner: Option<String>,
    /// Its members.
    pub members: Vec<MemberView>,
}

impl PartyView {
    /// The member with this account.
    #[must_use]
    pub fn member(&self, account: &str) -> Option<&MemberView> {
        self.members.iter().find(|member| member.account == account)
    }

    /// Whether `account` leads it.
    #[must_use]
    pub fn is_leader(&self, account: &str) -> bool {
        self.leader.as_deref() == Some(account)
    }

    /// Whether it has exactly one member of each role.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.members.len() == Role::ALL.len()
            && Role::ALL
                .into_iter()
                .all(|role| self.members.iter().filter(|m| m.role == role).count() == 1)
    }
}

/// Something a member asks the party to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartyAction {
    /// Answer the ready check.
    RespondReady,
    /// Hand leadership to another member.
    TransferLeader {
        /// The new leader's account.
        target: String,
    },
    /// Remove another member.
    Kick {
        /// The member's account.
        target: String,
    },
    /// Leave.
    Leave,
    /// Start the dungeon.
    Start,
    /// Put the actor's own key in.
    InsertKey,
    /// Take the inserted key back out.
    WithdrawKey,
}

/// Whether `actor` may do `action` to `party`.
///
/// # Errors
///
/// An authorization error when the actor is not a member, or the action is leader-only and
/// they do not lead; a party error when the party's state or membership forbids it.
pub fn authorize(party: &PartyView, actor: &str, action: &PartyAction) -> Result<(), RealmError> {
    if party.member(actor).is_none() {
        return Err(RealmError::unauthorized("You are not in that party."));
    }
    if party.state.is_terminal() {
        return Err(RealmError::party("That party no longer exists."));
    }
    let leader_only = |what: &str| -> Result<(), RealmError> {
        if party.is_leader(actor) {
            Ok(())
        } else {
            Err(RealmError::unauthorized(format!(
                "Only the party leader can {what}."
            )))
        }
    };
    match action {
        PartyAction::RespondReady => {
            if party.state != PartyState::ReadyCheck {
                return Err(RealmError::party("There is no ready check to answer."));
            }
            Ok(())
        }
        PartyAction::TransferLeader { target } => {
            leader_only("transfer leadership")?;
            if !matches!(party.state, PartyState::Starting | PartyState::InDungeon) {
                return Err(RealmError::party("Leadership cannot change right now."));
            }
            if target == actor {
                return Err(RealmError::party("You already lead the party."));
            }
            if party.member(target).is_none() {
                return Err(RealmError::party("That player is not in your party."));
            }
            Ok(())
        }
        PartyAction::Kick { target } => {
            leader_only("remove members")?;
            if !matches!(party.state, PartyState::Ready | PartyState::InDungeon) {
                return Err(RealmError::party("Members cannot be removed right now."));
            }
            if target == actor {
                return Err(RealmError::party("Use Leave to leave the party."));
            }
            if party.member(target).is_none() {
                return Err(RealmError::party("That player is not in your party."));
            }
            Ok(())
        }
        PartyAction::Leave => {
            if party.state == PartyState::ReadyCheck {
                return Err(RealmError::party("Decline the ready check to leave."));
            }
            Ok(())
        }
        PartyAction::Start => {
            leader_only("start the dungeon")?;
            validate_start(party)
        }
        PartyAction::InsertKey => match party.state {
            PartyState::ChoosingKey => {
                if party.member(actor).is_some_and(|m| m.has_key_for_dungeon) {
                    Ok(())
                } else {
                    Err(RealmError::party("You have no key for this dungeon."))
                }
            }
            PartyState::Ready => Err(RealmError::party("A key is already inserted.")),
            _ => Err(RealmError::party("No key can be inserted right now.")),
        },
        PartyAction::WithdrawKey => {
            if party.state != PartyState::Ready {
                return Err(RealmError::party("There is no key to withdraw."));
            }
            if party.key_owner.as_deref() != Some(actor) {
                return Err(RealmError::unauthorized(
                    "Only the key's owner can withdraw it.",
                ));
            }
            Ok(())
        }
    }
}

/// Whether the party can start: ready, led by its key owner, complete, everyone accepted and
/// connected.
///
/// # Errors
///
/// A party error naming the first reason it cannot.
pub fn validate_start(party: &PartyView) -> Result<(), RealmError> {
    if party.state != PartyState::Ready {
        return Err(RealmError::party("The party is not ready to start."));
    }
    match (&party.key_owner, &party.leader) {
        (Some(owner), Some(leader)) if owner == leader => {}
        (None, _) => return Err(RealmError::party("The party has no key inserted.")),
        _ => {
            return Err(RealmError::party(
                "Only the key's owner can start the dungeon.",
            ));
        }
    }
    if !party.is_complete() {
        return Err(RealmError::party(
            "The party needs a Tank, a Healer and a DPS to start.",
        ));
    }
    if party
        .members
        .iter()
        .any(|member| member.ready != ReadyResponse::Accepted)
    {
        return Err(RealmError::party("Not every member has confirmed."));
    }
    if party
        .members
        .iter()
        .any(|member| member.presence != Presence::Present)
    {
        return Err(RealmError::party(
            "A party member is disconnected. Wait for them to return.",
        ));
    }
    Ok(())
}

/// How a ready check stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadyCheckOutcome {
    /// Someone has not answered and there is time left.
    Waiting,
    /// Everyone accepted.
    AllAccepted,
    /// It failed. `requeue` accepted and stay queued (keeping their original place); `remove`
    /// declined, timed out or disconnected and leave the queue.
    Cancelled {
        /// Accounts that go back into the queue.
        requeue: Vec<String>,
        /// Accounts that leave the queue.
        remove: Vec<String>,
    },
}

/// Evaluates a ready check at `now` against its `deadline`.
///
/// A decline or a disconnect cancels at once — there is no point waiting for the others. A
/// disconnected member is never treated as having accepted. At the deadline, whoever has not
/// answered is removed and whoever accepted is re-queued.
#[must_use]
pub fn evaluate_ready_check(
    members: &[MemberView],
    now: Micros,
    deadline: Micros,
) -> ReadyCheckOutcome {
    let failed = |member: &MemberView| {
        member.ready == ReadyResponse::Declined
            || member.presence != Presence::Present
            || (now >= deadline && member.ready == ReadyResponse::Pending)
    };
    if members.iter().any(failed) {
        let (remove, requeue): (Vec<&MemberView>, Vec<&MemberView>) =
            members.iter().partition(|member| failed(member));
        return ReadyCheckOutcome::Cancelled {
            requeue: requeue.iter().map(|m| m.account.clone()).collect(),
            remove: remove.iter().map(|m| m.account.clone()).collect(),
        };
    }
    if members
        .iter()
        .all(|member| member.ready == ReadyResponse::Accepted)
    {
        ReadyCheckOutcome::AllAccepted
    } else {
        ReadyCheckOutcome::Waiting
    }
}

/// The answer a member ends up with after answering `answer` when they had `current`.
///
/// Answering the same thing twice is harmless (a double click, a retried request); changing
/// an answer once given is refused, so a member cannot accept, let the party start forming
/// and then flip to a decline.
///
/// # Errors
///
/// A party error if the member already gave a different answer.
pub fn apply_ready_response(
    current: ReadyResponse,
    answer: ReadyResponse,
) -> Result<ReadyResponse, RealmError> {
    match (current, answer) {
        (_, ReadyResponse::Pending) => Err(RealmError::protocol("That is not an answer.")),
        (ReadyResponse::Pending, answer) => Ok(answer),
        (current, answer) if current == answer => Ok(current),
        _ => Err(RealmError::party(
            "You have already answered the ready check.",
        )),
    }
}

/// Picks a leader from `candidates` using the server's random `roll`. `None` if empty.
///
/// The roll comes from the module's seeded RNG (`ctx.rng()`), never from a client.
#[must_use]
pub fn pick_leader(candidates: &[String], roll: u64) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    let len = u64::try_from(candidates.len()).ok()?;
    let index = usize::try_from(roll % len).ok()?;
    candidates.get(index).cloned()
}

/// Who leads after `leaving` goes: a random remaining member, connected ones preferred.
#[must_use]
pub fn successor(party: &PartyView, leaving: &str, roll: u64) -> Option<String> {
    let remaining: Vec<&MemberView> = party
        .members
        .iter()
        .filter(|member| member.account != leaving)
        .collect();
    let present: Vec<String> = remaining
        .iter()
        .filter(|member| member.presence == Presence::Present)
        .map(|member| member.account.clone())
        .collect();
    if present.is_empty() {
        let all: Vec<String> = remaining.iter().map(|m| m.account.clone()).collect();
        pick_leader(&all, roll)
    } else {
        pick_leader(&present, roll)
    }
}

/// What happens to a party when a member leaves (or is kicked) in its current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartureEffect {
    /// Before the run: the party cannot start without a full roster and roles are never
    /// replaced, so it disbands and the others go back to the queue.
    DisbandAndRequeue,
    /// During the run: the member is removed and the rest carry on.
    ContinueWithout,
    /// The last member left: the party is over.
    Disband,
}

/// The effect of `leaving` departing `party`.
#[must_use]
pub fn departure_effect(party: &PartyView, leaving: &str) -> DepartureEffect {
    let remaining = party
        .members
        .iter()
        .filter(|member| member.account != leaving)
        .count();
    if remaining == 0 {
        return DepartureEffect::Disband;
    }
    match party.state {
        PartyState::ReadyCheck | PartyState::ChoosingKey | PartyState::Ready => {
            DepartureEffect::DisbandAndRequeue
        }
        _ => DepartureEffect::ContinueWithout,
    }
}

/// Party tunables, stored in the module's config table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyConfig {
    /// How long a ready check waits for answers.
    pub ready_check_timeout: Micros,
}

impl Default for PartyConfig {
    fn default() -> Self {
        Self {
            ready_check_timeout: 30 * SECOND,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn member(account: &str, role: Role) -> MemberView {
        MemberView {
            account: account.into(),
            role,
            ready: ReadyResponse::Accepted,
            presence: Presence::Present,
            has_key_for_dungeon: false,
        }
    }

    fn ready_party() -> PartyView {
        PartyView {
            state: PartyState::Ready,
            leader: Some("tank".into()),
            key_owner: Some("tank".into()),
            members: vec![
                member("tank", Role::Tank),
                member("healer", Role::Healer),
                member("dps", Role::Dps),
            ],
        }
    }

    /// Everyone accepted, nobody has put a key in; the tank and the dps have one to offer.
    fn choosing_party() -> PartyView {
        let mut party = ready_party();
        party.state = PartyState::ChoosingKey;
        party.leader = None;
        party.key_owner = None;
        party.members[0].has_key_for_dungeon = true;
        party.members[2].has_key_for_dungeon = true;
        party
    }

    #[test]
    fn transitions_follow_the_diagram() {
        use PartyState::*;
        assert!(ReadyCheck.can_become(ChoosingKey));
        assert!(ReadyCheck.can_become(Disbanded));
        assert!(
            !ReadyCheck.can_become(Ready),
            "a key is always chosen first"
        );
        assert!(!ReadyCheck.can_become(Starting));
        assert!(ChoosingKey.can_become(Ready));
        assert!(ChoosingKey.can_become(Disbanded));
        assert!(!ChoosingKey.can_become(Starting));
        assert!(Ready.can_become(ChoosingKey));
        assert!(Ready.can_become(Starting));
        assert!(!Ready.can_become(InDungeon));
        assert!(Starting.can_become(InDungeon));
        assert!(InDungeon.can_become(Completed));
        assert!(InDungeon.can_become(Failed));
        assert!(!Completed.can_become(InDungeon));
        for state in PartyState::ALL {
            assert!(!Disbanded.can_become(state));
            assert!(!state.can_become(state), "{state:?} cannot become itself");
        }
    }

    #[test]
    fn non_members_can_do_nothing() {
        let party = ready_party();
        for action in [
            PartyAction::RespondReady,
            PartyAction::Leave,
            PartyAction::Start,
            PartyAction::InsertKey,
            PartyAction::WithdrawKey,
            PartyAction::Kick {
                target: "dps".into(),
            },
        ] {
            let error = authorize(&party, "stranger", &action).expect_err("refused");
            assert_eq!(error.category, crate::ErrorCategory::Authorization);
        }
    }

    #[test]
    fn non_leaders_cannot_use_leader_actions() {
        let party = ready_party();
        for action in [
            PartyAction::Start,
            PartyAction::Kick {
                target: "dps".into(),
            },
            PartyAction::TransferLeader {
                target: "healer".into(),
            },
        ] {
            let error = authorize(&party, "healer", &action).expect_err("refused");
            assert_eq!(error.category, crate::ErrorCategory::Authorization);
        }
        assert!(authorize(&party, "healer", &PartyAction::Leave).is_ok());
    }

    #[test]
    fn the_leader_can_lead() {
        let party = ready_party();
        assert!(authorize(&party, "tank", &PartyAction::Start).is_ok());
        assert!(
            authorize(
                &party,
                "tank",
                &PartyAction::Kick {
                    target: "dps".into()
                }
            )
            .is_ok()
        );
        let mut running = ready_party();
        running.state = PartyState::InDungeon;
        assert!(
            authorize(
                &running,
                "tank",
                &PartyAction::TransferLeader {
                    target: "healer".into()
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn leadership_cannot_be_handed_over_while_ready() {
        let party = ready_party();
        let transfer = PartyAction::TransferLeader {
            target: "healer".into(),
        };
        assert!(authorize(&party, "tank", &transfer).is_err());
        let mut starting = ready_party();
        starting.state = PartyState::Starting;
        assert!(authorize(&starting, "tank", &transfer).is_ok());
    }

    #[test]
    fn a_member_with_a_key_can_insert_it_while_choosing() {
        let party = choosing_party();
        assert!(authorize(&party, "tank", &PartyAction::InsertKey).is_ok());
        assert!(authorize(&party, "dps", &PartyAction::InsertKey).is_ok());
        let keyless = authorize(&party, "healer", &PartyAction::InsertKey).expect_err("no key");
        assert_eq!(keyless.message, "You have no key for this dungeon.");
        let stranger = authorize(&party, "stranger", &PartyAction::InsertKey).expect_err("no");
        assert_eq!(stranger.category, crate::ErrorCategory::Authorization);
    }

    #[test]
    fn a_second_key_is_refused_once_ready() {
        let mut party = ready_party();
        party.members[2].has_key_for_dungeon = true;
        let error = authorize(&party, "dps", &PartyAction::InsertKey).expect_err("taken");
        assert_eq!(error.message, "A key is already inserted.");
        let mut checking = choosing_party();
        checking.state = PartyState::ReadyCheck;
        assert!(authorize(&checking, "tank", &PartyAction::InsertKey).is_err());
    }

    #[test]
    fn only_the_key_owner_can_withdraw_it() {
        let party = ready_party();
        assert!(authorize(&party, "tank", &PartyAction::WithdrawKey).is_ok());
        let error = authorize(&party, "healer", &PartyAction::WithdrawKey).expect_err("not owner");
        assert_eq!(error.category, crate::ErrorCategory::Authorization);
        assert!(authorize(&choosing_party(), "tank", &PartyAction::WithdrawKey).is_err());
    }

    #[test]
    fn the_party_starts_only_with_its_key_owner_leading() {
        let mut no_key = ready_party();
        no_key.key_owner = None;
        assert!(authorize(&no_key, "tank", &PartyAction::Start).is_err());
        // The leader is not the owner.
        let mut other = ready_party();
        other.key_owner = Some("healer".into());
        assert!(authorize(&other, "tank", &PartyAction::Start).is_err());
        assert!(authorize(&other, "healer", &PartyAction::Start).is_err());
        assert!(authorize(&ready_party(), "tank", &PartyAction::Start).is_ok());
        // Nobody leads while a key is being chosen, so nobody can start.
        assert!(authorize(&choosing_party(), "tank", &PartyAction::Start).is_err());
    }

    #[test]
    fn invalid_kicks_and_transfers_are_refused() {
        let party = ready_party();
        let kick_self = PartyAction::Kick {
            target: "tank".into(),
        };
        assert!(authorize(&party, "tank", &kick_self).is_err());
        let kick_stranger = PartyAction::Kick {
            target: "stranger".into(),
        };
        assert!(authorize(&party, "tank", &kick_stranger).is_err());
        let to_stranger = PartyAction::TransferLeader {
            target: "stranger".into(),
        };
        assert!(authorize(&party, "tank", &to_stranger).is_err());
        let mut checking = ready_party();
        checking.state = PartyState::ReadyCheck;
        let kick = PartyAction::Kick {
            target: "dps".into(),
        };
        assert!(authorize(&checking, "tank", &kick).is_err());
    }

    #[test]
    fn start_requires_everyone_present_and_confirmed() {
        let mut party = ready_party();
        assert!(validate_start(&party).is_ok());
        party.members[1].presence = Presence::TemporarilyDisconnected;
        assert!(validate_start(&party).is_err());
        let mut party = ready_party();
        party.members.pop();
        assert!(validate_start(&party).is_err());
        let mut party = ready_party();
        party.state = PartyState::Starting;
        assert!(validate_start(&party).is_err(), "cannot start twice");
        let mut party = ready_party();
        party.key_owner = None;
        assert!(validate_start(&party).is_err(), "needs a key");
    }

    #[test]
    fn ready_check_all_confirm() {
        let members = ready_party().members;
        assert_eq!(
            evaluate_ready_check(&members, 0, 10),
            ReadyCheckOutcome::AllAccepted
        );
    }

    #[test]
    fn ready_check_waits_then_times_out() {
        let mut members = ready_party().members;
        members[2].ready = ReadyResponse::Pending;
        assert_eq!(
            evaluate_ready_check(&members, 5, 10),
            ReadyCheckOutcome::Waiting
        );
        assert_eq!(
            evaluate_ready_check(&members, 10, 10),
            ReadyCheckOutcome::Cancelled {
                requeue: vec!["tank".into(), "healer".into()],
                remove: vec!["dps".into()],
            }
        );
    }

    #[test]
    fn a_decline_cancels_at_once() {
        let mut members = ready_party().members;
        members[0].ready = ReadyResponse::Declined;
        members[1].ready = ReadyResponse::Pending;
        assert_eq!(
            evaluate_ready_check(&members, 0, 10),
            ReadyCheckOutcome::Cancelled {
                // The healer had not answered yet, which is not their fault: they keep their
                // place in the queue alongside the dps who accepted.
                requeue: vec!["healer".into(), "dps".into()],
                remove: vec!["tank".into()],
            }
        );
    }

    #[test]
    fn a_disconnect_cancels_and_is_never_an_accept() {
        let mut members = ready_party().members;
        members[1].presence = Presence::TemporarilyDisconnected;
        assert_eq!(
            evaluate_ready_check(&members, 0, 10),
            ReadyCheckOutcome::Cancelled {
                requeue: vec!["tank".into(), "dps".into()],
                remove: vec!["healer".into()],
            }
        );
    }

    #[test]
    fn leaders_are_picked_from_the_roll() {
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(pick_leader(&names, 0).as_deref(), Some("a"));
        assert_eq!(pick_leader(&names, 4).as_deref(), Some("b"));
        assert_eq!(pick_leader(&[], 4), None);
        // Every member is reachable.
        let picked: std::collections::BTreeSet<String> = (0..30)
            .filter_map(|roll| pick_leader(&names, roll))
            .collect();
        assert_eq!(picked.len(), 3);
    }

    #[test]
    fn successor_prefers_connected_members() {
        let mut party = ready_party();
        party.members[1].presence = Presence::TemporarilyDisconnected;
        for roll in 0..10 {
            assert_eq!(successor(&party, "tank", roll).as_deref(), Some("dps"));
        }
        party.members[2].presence = Presence::TemporarilyDisconnected;
        assert!(successor(&party, "tank", 0).is_some());
        let solo = PartyView {
            members: vec![member("tank", Role::Tank)],
            ..ready_party()
        };
        assert_eq!(successor(&solo, "tank", 0), None);
    }

    #[test]
    fn departures_disband_before_the_run_and_not_during_it() {
        let party = ready_party();
        assert_eq!(
            departure_effect(&party, "dps"),
            DepartureEffect::DisbandAndRequeue
        );
        assert_eq!(
            departure_effect(&choosing_party(), "dps"),
            DepartureEffect::DisbandAndRequeue
        );
        let mut running = ready_party();
        running.state = PartyState::InDungeon;
        assert_eq!(
            departure_effect(&running, "dps"),
            DepartureEffect::ContinueWithout
        );
        let solo = PartyView {
            state: PartyState::InDungeon,
            members: vec![member("tank", Role::Tank)],
            ..ready_party()
        };
        assert_eq!(departure_effect(&solo, "tank"), DepartureEffect::Disband);
    }

    #[test]
    fn duplicate_confirmations_are_idempotent() {
        use ReadyResponse::*;
        assert_eq!(apply_ready_response(Pending, Accepted), Ok(Accepted));
        assert_eq!(apply_ready_response(Accepted, Accepted), Ok(Accepted));
        assert_eq!(apply_ready_response(Declined, Declined), Ok(Declined));
        assert!(apply_ready_response(Accepted, Declined).is_err());
        assert!(apply_ready_response(Pending, Pending).is_err());
    }

    #[test]
    fn duplicate_ready_answers_are_refused_after_the_check() {
        let party = ready_party();
        assert!(authorize(&party, "dps", &PartyAction::RespondReady).is_err());
    }
}
