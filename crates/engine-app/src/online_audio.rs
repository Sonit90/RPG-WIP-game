//! The online front-end, heard: which sound a button makes and which changes of the realm's
//! state are worth a sound of their own.
//!
//! ```text
//! FrontEndAction ─▶ action_sound ─────────────────────▶ a click, a confirm, a cancel
//! RealmMirror ─▶ NetSnapshot ─▶ NetSoundWatch::observe ─▶ queued, match found, party formed,
//!                                                        key in, 5-4-3-2-1, GO, done, failed
//! ```
//!
//! Both halves are driven by *events* - a button was pressed, a state changed - never polled
//! for a sound: the watch remembers the previous frame's state and speaks only on the edge, so
//! a ready check that stays open for thirty seconds is one chime, not thirty.

use engine_audio::SoundId;
use engine_presentation::ui::frontend::FrontEndAction;
use engine_realm::instance::{CountdownDisplay, InstanceState};
use engine_realm::party::PartyState;

/// What pressing a front-end button sounds like.
///
/// Confirming a choice is a select, going back or leaving is a cancel, accepting the ready
/// check is its own bright chime, and everything else is a click. The joining of the queue and
/// the insertion of a key are not here: they are heard when the realm *answers*, so a refusal
/// does not sound like a success.
#[must_use]
pub fn action_sound(action: &FrontEndAction) -> SoundId {
    use FrontEndAction as A;
    match action {
        A::Accept => SoundId::ReadyAccept,
        A::Back
        | A::Decline
        | A::LeaveQueue
        | A::LeaveParty
        | A::Logout
        | A::Quit
        | A::ChangeCharacter
        | A::UnstageSalvage(_)
        | A::WithdrawKey
        | A::Kick(_) => SoundId::UiCancel,
        A::Login { .. }
        | A::Register { .. }
        | A::ContinueFromTitle
        | A::SelectCharacter(_)
        | A::ConfirmCharacter
        | A::SelectDungeon(_)
        | A::ConfirmDungeon
        | A::SelectRole(_)
        | A::SelectEnhance(_)
        | A::StartDungeon
        | A::Exchange(_)
        | A::Enhance
        | A::SalvageTray => SoundId::UiSelect,
        A::JoinQueue
        | A::InsertKey
        | A::OpenDungeonFinder
        | A::OpenEquipment
        | A::OpenWorkshop
        | A::ReturnToHub
        | A::StageSalvage(_)
        | A::WorkshopMode(_)
        | A::DowngradeKey
        | A::CleanseKey => SoundId::UiClick,
    }
}

/// The parts of the realm's state the watch listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetSnapshot {
    /// Whether the account is in the queue.
    pub queued: bool,
    /// The party, if there is one: its id and where it is in its life.
    pub party: Option<(u64, PartyState)>,
    /// Whether a key has been inserted into the lock.
    pub key_in: bool,
    /// The instance's state, if there is an instance.
    pub instance: Option<InstanceState>,
    /// What the run's countdown shows.
    pub countdown: Option<CountdownDisplay>,
}

/// Remembers the last [`NetSnapshot`] and says what changed.
#[derive(Debug, Default)]
pub struct NetSoundWatch {
    last: NetSnapshot,
    /// The party whose ready check has already been announced.
    announced: Option<u64>,
}

impl NetSoundWatch {
    /// The sounds the change from the last snapshot to `now` calls for, in the order to play
    /// them. Empty while nothing changed.
    pub fn observe(&mut self, now: NetSnapshot, out: &mut Vec<SoundId>) {
        let last = self.last;

        if now.queued && !last.queued {
            out.push(SoundId::QueueJoin);
        }

        // A ready check opening is the loudest thing the front-end does: the player may be
        // looking anywhere else. Announced once per party, however long it stays open.
        if let Some((id, PartyState::ReadyCheck)) = now.party
            && self.announced != Some(id)
        {
            self.announced = Some(id);
            out.push(SoundId::MatchFound);
        }
        // Everybody accepted: the party has formed.
        if let (Some((_, PartyState::ReadyCheck)), Some((_, PartyState::ChoosingKey))) =
            (last.party, now.party)
        {
            out.push(SoundId::PartyFormed);
        }

        if now.key_in && !last.key_in {
            out.push(SoundId::KeyInsert);
        }

        match (last.countdown, now.countdown) {
            (previous, Some(CountdownDisplay::Seconds(seconds)))
                if previous != Some(CountdownDisplay::Seconds(seconds)) =>
            {
                out.push(SoundId::CountdownTick);
            }
            (previous, Some(CountdownDisplay::Go)) if previous != Some(CountdownDisplay::Go) => {
                out.push(SoundId::CountdownGo);
            }
            _ => {}
        }

        // Only the transition is a sound: coming back to a finished run (a reconnect) is not
        // a fanfare.
        if let (Some(before), Some(after)) = (last.instance, now.instance)
            && before != after
        {
            match after {
                InstanceState::Completed => out.push(SoundId::DungeonComplete),
                InstanceState::Failed => out.push(SoundId::DungeonFail),
                _ => {}
            }
        }

        self.last = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(watch: &mut NetSoundWatch, snapshot: NetSnapshot) -> Vec<SoundId> {
        let mut out = Vec::new();
        watch.observe(snapshot, &mut out);
        out
    }

    #[test]
    fn every_button_has_a_sound_and_leaving_is_not_a_click() {
        assert_eq!(action_sound(&FrontEndAction::Back), SoundId::UiCancel);
        assert_eq!(action_sound(&FrontEndAction::Decline), SoundId::UiCancel);
        assert_eq!(action_sound(&FrontEndAction::Accept), SoundId::ReadyAccept);
        assert_eq!(
            action_sound(&FrontEndAction::ConfirmCharacter),
            SoundId::UiSelect
        );
        assert_eq!(
            action_sound(&FrontEndAction::OpenWorkshop),
            SoundId::UiClick
        );
    }

    #[test]
    fn joining_the_queue_and_the_match_are_heard_once() {
        let mut watch = NetSoundWatch::default();
        assert!(heard(&mut watch, NetSnapshot::default()).is_empty());

        let queued = NetSnapshot {
            queued: true,
            ..NetSnapshot::default()
        };
        assert_eq!(heard(&mut watch, queued), [SoundId::QueueJoin]);
        assert!(
            heard(&mut watch, queued).is_empty(),
            "no repeat while queued"
        );

        let matched = NetSnapshot {
            party: Some((7, PartyState::ReadyCheck)),
            ..NetSnapshot::default()
        };
        assert_eq!(heard(&mut watch, matched), [SoundId::MatchFound]);
        assert!(
            heard(&mut watch, matched).is_empty(),
            "one pop per ready check"
        );

        let formed = NetSnapshot {
            party: Some((7, PartyState::ChoosingKey)),
            ..NetSnapshot::default()
        };
        assert_eq!(heard(&mut watch, formed), [SoundId::PartyFormed]);

        let keyed = NetSnapshot {
            key_in: true,
            party: Some((7, PartyState::Ready)),
            ..NetSnapshot::default()
        };
        assert_eq!(heard(&mut watch, keyed), [SoundId::KeyInsert]);
    }

    #[test]
    fn the_countdown_ticks_each_second_and_ends_in_go() {
        let mut watch = NetSoundWatch::default();
        let mut sounds = Vec::new();
        for display in [
            CountdownDisplay::Seconds(5),
            CountdownDisplay::Seconds(5),
            CountdownDisplay::Seconds(4),
            CountdownDisplay::Seconds(3),
            CountdownDisplay::Seconds(2),
            CountdownDisplay::Seconds(1),
            CountdownDisplay::Go,
            CountdownDisplay::Go,
        ] {
            sounds.extend(heard(
                &mut watch,
                NetSnapshot {
                    countdown: Some(display),
                    ..NetSnapshot::default()
                },
            ));
        }
        let mut expected = vec![SoundId::CountdownTick; 5];
        expected.push(SoundId::CountdownGo);
        assert_eq!(sounds, expected);
    }

    #[test]
    fn a_run_ending_is_a_fanfare_or_a_sting_but_only_on_the_transition() {
        let mut watch = NetSoundWatch::default();
        let active = NetSnapshot {
            instance: Some(InstanceState::Active),
            ..NetSnapshot::default()
        };
        let done = NetSnapshot {
            instance: Some(InstanceState::Completed),
            ..NetSnapshot::default()
        };
        let failed = NetSnapshot {
            instance: Some(InstanceState::Failed),
            ..NetSnapshot::default()
        };
        assert!(heard(&mut watch, active).is_empty());
        assert_eq!(heard(&mut watch, done), [SoundId::DungeonComplete]);
        assert!(heard(&mut watch, done).is_empty());

        let mut watch = NetSoundWatch::default();
        heard(&mut watch, active);
        assert_eq!(heard(&mut watch, failed), [SoundId::DungeonFail]);

        // Arriving on an already-finished run says nothing.
        let mut watch = NetSoundWatch::default();
        assert!(heard(&mut watch, done).is_empty());
    }
}
