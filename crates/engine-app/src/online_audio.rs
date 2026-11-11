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
use engine_game::character::ActiveSets;
use engine_game::hero::Hero;
use engine_presentation::ui::frontend::FrontEndAction;
use engine_realm::api::{HistoryEntry, SharpenOutcomeName};
use engine_realm::instance::{CountdownDisplay, InstanceState};
use engine_realm::outcome::RunOutcome;
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
        | A::ReforgeCancel
        | A::GemUpgradeCancel
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
        | A::SelectSharpen(_)
        | A::StartDungeon
        | A::Exchange(_)
        | A::Sharpen
        | A::Reforge
        | A::GemUpgrade
        | A::SocketGem(_)
        | A::UnsocketGem(_)
        | A::SalvageTray => SoundId::UiSelect,
        A::JoinQueue
        | A::InsertKey
        | A::OpenDungeonFinder
        | A::OpenEquipment
        | A::OpenWorkshop
        | A::OpenHistory
        | A::OpenLeaderboards
        | A::OpenCommunity
        | A::Community(_)
        | A::HistorySelect(_)
        | A::HistoryOlder
        | A::HistoryNewest
        | A::BoardDungeon(_)
        | A::BoardKey(_)
        | A::BoardKind(_)
        | A::BoardPage(_)
        | A::ReturnToHub
        | A::StageSalvage(_)
        | A::WorkshopMode(_)
        | A::ReforgeMode
        | A::ReforgeAsk
        | A::GemsMode
        | A::GemsPick(_)
        | A::GemUpgradeAsk
        | A::DowngradeKey
        | A::CleanseKey => SoundId::UiClick,
    }
}

/// What a sharpening attempt sounds like: a bright ring when it takes, a crunch when the item
/// breaks.
#[must_use]
pub fn sharpen_sound(outcome: SharpenOutcomeName) -> SoundId {
    match outcome {
        SharpenOutcomeName::Success => SoundId::SharpenSuccess,
        SharpenOutcomeName::Broken => SoundId::ItemBreak,
    }
}

/// What a reforge result sounds like, by the new base quality against the old: the jackpot
/// sting for a 110, the epic ring for a 100 or better, a lesser chime for any improvement, the
/// error tone for a worse roll, and nothing for the same number again.
#[must_use]
pub fn reforge_sound(old_quality: u8, new_quality: u8) -> Option<SoundId> {
    if new_quality == 110 {
        Some(SoundId::LootJackpot)
    } else if new_quality >= 100 && new_quality > old_quality {
        Some(SoundId::LootEpic)
    } else if new_quality > old_quality {
        Some(SoundId::LootRare)
    } else if new_quality < old_quality {
        Some(SoundId::UiError)
    } else {
        None
    }
}

/// What a gem being set (`gold_paid` zero) or prised out (it costs gold) sounds like.
#[must_use]
pub fn gem_slot_sound(gold_paid: u64) -> SoundId {
    if gold_paid > 0 {
        SoundId::GemUnsocket
    } else {
        SoundId::GemSocket
    }
}

/// What a gem upgrade sounds like: three rising notes when it takes, the crunch of lost gems
/// when it does not.
#[must_use]
pub fn gem_upgrade_sound(success: bool) -> SoundId {
    if success {
        SoundId::GemUpgrade
    } else {
        SoundId::ItemBreak
    }
}

/// Whether the newest run in `runs` (newest first) is a personal best: a completion that beat
/// every earlier completion of the same dungeon at the same key level. A first clear is not a
/// best, it is the only one.
#[must_use]
pub fn is_personal_best(runs: &[HistoryEntry]) -> bool {
    let Some((newest, earlier)) = runs.split_first() else {
        return false;
    };
    let done = |run: &HistoryEntry| matches!(run.outcome, RunOutcome::Timed | RunOutcome::Overtime);
    if !done(newest) {
        return false;
    }
    let mut rivals = earlier
        .iter()
        .filter(|run| {
            done(run) && run.dungeon_id == newest.dungeon_id && run.key_level == newest.key_level
        })
        .peekable();
    rivals.peek().is_some() && rivals.all(|run| newest.duration_us < run.duration_us)
}

/// Notices a set bonus switching on as equipment changes. The first look at a hero is the
/// baseline (logging in dressed in a set is not an event); after that any set gaining a tier
/// is one chord. Losing a tier is silent.
#[derive(Debug, Default)]
pub struct SetBonusWatch {
    hero: Option<Hero>,
    tiers: Vec<(String, usize)>,
}

impl SetBonusWatch {
    /// Looks at what `hero` wears now. Returns the cue if a set gained a tier since the last
    /// look at the same hero.
    pub fn observe(&mut self, hero: Hero, worn: &ActiveSets) -> Option<SoundId> {
        let now: Vec<(String, usize)> = worn
            .iter()
            .map(|active| (active.set.id.clone(), active.tiers_active()))
            .collect();
        let known = self.hero == Some(hero);
        let gained = known
            && now.iter().any(|(id, tiers)| {
                let before = self
                    .tiers
                    .iter()
                    .find(|(known_id, _)| known_id == id)
                    .map_or(0, |(_, tiers)| *tiers);
                *tiers > before
            });
        self.hero = Some(hero);
        self.tiers = now;
        gained.then_some(SoundId::SetBonus)
    }
}

/// Where the run's timer stands, for the edges worth a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimerPhase {
    /// No timer (not in a run, or a map without a clock).
    #[default]
    None,
    /// Time remains.
    Running,
    /// Under a minute remains.
    Urgent,
    /// The clock ran out.
    Overtime,
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
    /// Where the run's timer stands while the run is active.
    pub timer: TimerPhase,
}

/// Remembers the last [`NetSnapshot`] and says what changed.
#[derive(Debug, Default)]
pub struct NetSoundWatch {
    last: NetSnapshot,
    /// The party whose ready check has already been announced.
    announced: Option<u64>,
    /// The last timer phase seen while the run was active: what the run finished against.
    run_phase: TimerPhase,
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

        // The run's timer: a warning once when a minute is left, and a sting as it runs out.
        if last.timer == TimerPhase::Running && now.timer == TimerPhase::Urgent {
            out.push(SoundId::TimerWarning);
        }
        if matches!(last.timer, TimerPhase::Running | TimerPhase::Urgent)
            && now.timer == TimerPhase::Overtime
        {
            out.push(SoundId::ModifierResultWeak);
        }
        if now.timer != TimerPhase::None {
            self.run_phase = now.timer;
        }

        // Only the transition is a sound: coming back to a finished run (a reconnect) is not
        // a fanfare. A completion against a clock also says how it went: in time, or late.
        if let (Some(before), Some(after)) = (last.instance, now.instance)
            && before != after
        {
            match after {
                InstanceState::Completed => {
                    out.push(SoundId::DungeonComplete);
                    match self.run_phase {
                        TimerPhase::None => {}
                        TimerPhase::Overtime => out.push(SoundId::RunOvertime),
                        TimerPhase::Running | TimerPhase::Urgent => out.push(SoundId::RunTimed),
                    }
                }
                InstanceState::Failed => out.push(SoundId::DungeonFail),
                _ => {}
            }
        }
        if now.instance.is_none() {
            self.run_phase = TimerPhase::None;
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
    fn sharpening_rings_when_it_takes_and_crunches_when_the_item_breaks() {
        assert_eq!(
            sharpen_sound(SharpenOutcomeName::Success),
            SoundId::SharpenSuccess
        );
        assert_eq!(
            sharpen_sound(SharpenOutcomeName::Broken),
            SoundId::ItemBreak
        );
    }

    #[test]
    fn a_reforge_is_heard_as_better_worse_or_a_jackpot() {
        assert_eq!(reforge_sound(80, 110), Some(SoundId::LootJackpot));
        assert_eq!(reforge_sound(110, 110), Some(SoundId::LootJackpot));
        assert_eq!(reforge_sound(80, 102), Some(SoundId::LootEpic));
        assert_eq!(reforge_sound(80, 90), Some(SoundId::LootRare));
        assert_eq!(reforge_sound(90, 70), Some(SoundId::UiError));
        assert_eq!(reforge_sound(90, 90), None);
    }

    #[test]
    fn gems_have_a_cue_for_socketing_unsocketing_and_upgrading() {
        assert_eq!(gem_slot_sound(0), SoundId::GemSocket);
        assert_eq!(gem_slot_sound(250), SoundId::GemUnsocket);
        assert_eq!(gem_upgrade_sound(true), SoundId::GemUpgrade);
        assert_eq!(gem_upgrade_sound(false), SoundId::ItemBreak);
    }

    #[test]
    fn the_new_online_cues_are_all_different_sounds() {
        let sounds = [
            SoundId::SharpenSuccess,
            SoundId::ItemBreak,
            SoundId::GemSocket,
            SoundId::GemUnsocket,
            SoundId::GemUpgrade,
            SoundId::BookRead,
            SoundId::BookSwitch,
            SoundId::SetBonus,
            SoundId::TimerWarning,
            SoundId::RunTimed,
            SoundId::RunOvertime,
        ];
        for (i, a) in sounds.iter().enumerate() {
            for b in &sounds[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    fn entry(dungeon: &str, key: u32, outcome: RunOutcome, seconds: i64) -> HistoryEntry {
        HistoryEntry {
            run_uid: String::new(),
            dungeon_id: dungeon.into(),
            key_level: key,
            outcome,
            duration_us: seconds * engine_realm::SECOND,
            timer_us: 0,
            applied_at_us: 0,
            party: Vec::new(),
            modifiers: Vec::new(),
            deaths: 0,
            damage_taken: 0,
            healing: 0,
            dps: 0.0,
            loot: Vec::new(),
        }
    }

    #[test]
    fn a_personal_best_beats_every_earlier_completion_of_the_same_dungeon_and_key() {
        let best = entry("crypt", 5, RunOutcome::Timed, 600);
        let older = entry("crypt", 5, RunOutcome::Overtime, 700);
        let other_key = entry("crypt", 6, RunOutcome::Timed, 300);
        let other_dungeon = entry("foundry", 5, RunOutcome::Timed, 100);
        let failed = entry("crypt", 5, RunOutcome::Failed, 10);
        assert!(is_personal_best(&[
            best.clone(),
            older.clone(),
            other_key,
            other_dungeon,
            failed
        ]));
        // Slower than an earlier run, a failure, a first clear and an empty list are not.
        assert!(!is_personal_best(&[older, best.clone()]));
        assert!(!is_personal_best(&[
            entry("crypt", 5, RunOutcome::Failed, 1),
            best.clone()
        ]));
        assert!(!is_personal_best(&[best]));
        assert!(!is_personal_best(&[]));
    }

    #[test]
    fn a_set_bonus_chimes_when_a_tier_switches_on_and_not_on_load_or_removal() {
        use engine_game::character::{ActiveSets, EquipSlot, ItemStack, active_sets_of};
        engine_realm::loot::ensure_items_registered();
        let sets = engine_game::character::registered_sets();
        let Some(set) = sets
            .iter()
            .find(|set| set.bonuses.first().is_some_and(|b| b.count == 2))
        else {
            panic!("the shipped sets have a two-piece tier");
        };
        let piece = |key: &str| {
            engine_game::character::item_by_key(key).map(|def| ItemStack::new(def.id, 1))
        };
        let worn = |count: usize| -> ActiveSets {
            let mut slots = vec![None; EquipSlot::COUNT];
            for (slot, key) in set.pieces.iter().take(count).enumerate() {
                slots[slot] = piece(key);
            }
            active_sets_of(&slots)
        };
        let mut watch = SetBonusWatch::default();
        // The first look is the baseline, however much is worn.
        assert_eq!(watch.observe(Hero::Warrior, &worn(2)), None);
        assert_eq!(watch.observe(Hero::Warrior, &worn(2)), None);
        // Taking a piece off is silent; putting it back switches the tier on again.
        assert_eq!(watch.observe(Hero::Warrior, &worn(1)), None);
        assert_eq!(
            watch.observe(Hero::Warrior, &worn(2)),
            Some(SoundId::SetBonus)
        );
        // A different hero's first look is a baseline again.
        assert_eq!(watch.observe(Hero::Druid, &worn(2)), None);
        assert_eq!(watch.observe(Hero::Druid, &worn(0)), None);
        assert_eq!(
            watch.observe(Hero::Druid, &worn(2)),
            Some(SoundId::SetBonus)
        );
    }

    #[test]
    fn the_run_timer_warns_at_a_minute_and_a_completion_says_timed_or_overtime() {
        let at = |timer| NetSnapshot {
            instance: Some(InstanceState::Active),
            timer,
            ..NetSnapshot::default()
        };
        let done = NetSnapshot {
            instance: Some(InstanceState::Completed),
            ..NetSnapshot::default()
        };
        // Timed: running, then urgent (one warning), then a completion in time.
        let mut watch = NetSoundWatch::default();
        assert!(heard(&mut watch, at(TimerPhase::Running)).is_empty());
        assert_eq!(
            heard(&mut watch, at(TimerPhase::Urgent)),
            [SoundId::TimerWarning]
        );
        assert!(
            heard(&mut watch, at(TimerPhase::Urgent)).is_empty(),
            "one warning"
        );
        assert_eq!(
            heard(&mut watch, done),
            [SoundId::DungeonComplete, SoundId::RunTimed]
        );
        // Overtime: the clock runs out (a sting), the completion is late.
        let mut watch = NetSoundWatch::default();
        heard(&mut watch, at(TimerPhase::Running));
        heard(&mut watch, at(TimerPhase::Urgent));
        assert_eq!(
            heard(&mut watch, at(TimerPhase::Overtime)),
            [SoundId::ModifierResultWeak]
        );
        assert_eq!(
            heard(&mut watch, done),
            [SoundId::DungeonComplete, SoundId::RunOvertime]
        );
        // No clock at all (an untimed community map): the plain fanfare only.
        let mut watch = NetSoundWatch::default();
        heard(&mut watch, at(TimerPhase::None));
        assert_eq!(heard(&mut watch, done), [SoundId::DungeonComplete]);
        // Arriving mid-minute does not warn.
        let mut watch = NetSoundWatch::default();
        assert!(heard(&mut watch, at(TimerPhase::Urgent)).is_empty());
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
