//! Temporary effects that outlive the ability that applied them.
//!
//! A status is what makes Milestone 13's synergy possible: Ignite leaves [`Burning`] on a
//! target, and Detonate reads it without either ability knowing about the other.
//!
//! ```text
//! Ignite -> Burning (6 s, 8 damage a second)
//!               |
//!               +--> ticks damage on its own
//!               |
//!               +--> Detonate consumes what is left for a burst
//! ```
//!
//! [`Burning`]: StatusKind::Burning
//!
//! Statuses do not apply damage themselves. Ticking produces [`StatusOutcome`] values that
//! the simulation turns into damage and into [`crate::event::CombatEvent`]s, which is what
//! keeps a status tick indistinguishable from a direct hit as far as telemetry is
//! concerned — it is attributed to the ability that applied it, and to the caster.

use crate::ability::AbilityId;
use crate::combat::DamageType;
use crate::entity::EntityId;

/// Seconds between the damage ticks of a lasting status.
///
/// One second, so a burn's remaining duration and its remaining damage are the same number
/// scaled: what is left is readable off the overlay without arithmetic.
pub const STATUS_TICK_INTERVAL: f32 = 1.0;

/// Statuses the simulation knows.
///
/// One kind so far, because one is what proves the mechanism. The set is an enum rather
/// than a string or a registry id so a `match` over every status is exhaustive and adding
/// one is a compile error everywhere it must be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusKind {
    /// Damage over time. Consumed by Detonate.
    Burning,
}

impl StatusKind {
    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Burning => "BURNING",
        }
    }

    /// The kind of damage this status deals when it ticks.
    #[must_use]
    pub fn damage_type(self) -> DamageType {
        match self {
            Self::Burning => DamageType::Fire,
        }
    }
}

/// Why a status ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusEnd {
    /// Its duration ran out.
    Expired,
    /// An ability consumed it.
    Consumed,
    /// It was removed wholesale — a dummy reset, or the bearer dying.
    Cleared,
}

/// One active status on one entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    /// Which status this is.
    pub kind: StatusKind,
    /// Who applied it. Damage it deals is credited to them.
    pub source: EntityId,
    /// Which ability applied it. Damage it deals is attributed to that ability, so the
    /// meter shows "IGNITE 58" rather than an anonymous damage-over-time bucket.
    pub ability: AbilityId,
    /// Seconds left.
    pub remaining: f32,
    /// Full duration, kept so the overlay can draw a fraction.
    pub duration: f32,
    /// Damage each tick deals.
    pub damage_per_tick: f32,
    /// Seconds until the next damage tick.
    timer: f32,
}

impl Status {
    /// A freshly applied status.
    #[must_use]
    pub fn new(
        kind: StatusKind,
        source: EntityId,
        ability: AbilityId,
        duration: f32,
        damage_per_tick: f32,
    ) -> Self {
        let duration = duration.max(0.0);
        Self {
            kind,
            source,
            ability,
            remaining: duration,
            duration,
            damage_per_tick: damage_per_tick.max(0.0),
            timer: STATUS_TICK_INTERVAL,
        }
    }

    /// How much of the duration is left, in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.remaining / self.duration).clamp(0.0, 1.0)
    }
}

/// Something a status did during one simulation step.
///
/// Returned rather than applied, so the status system stays a pure function of time and
/// the simulation remains the only place that changes health.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StatusOutcome {
    /// The status dealt its periodic damage.
    Tick {
        /// Which status ticked.
        kind: StatusKind,
        /// Who is credited.
        source: EntityId,
        /// Which ability is credited.
        ability: AbilityId,
        /// How much damage to apply.
        damage: f32,
    },
    /// The status ended.
    Ended {
        /// Which status ended.
        kind: StatusKind,
        /// Why.
        reason: StatusEnd,
    },
}

/// The statuses on one entity.
///
/// A `Vec` with a reused allocation rather than a fixed array: the set is empty for most
/// entities most of the time, one entry deep in practice, and growing it later must not be
/// a structural change. Clearing keeps the capacity, so an arena that has been reset a
/// hundred times allocates as much as one that has never been.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StatusSet {
    entries: Vec<Status>,
}

impl StatusSet {
    /// No statuses.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a status, refreshing one of the same kind rather than stacking it.
    ///
    /// Refresh, not stack: two burns from the same caster ticking independently doubles the
    /// damage of spamming one button, which is a balance decision this prototype has no
    /// reason to make. Returns `true` if the status was newly applied.
    pub fn apply(&mut self, status: Status) -> bool {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.kind == status.kind)
        {
            // The timer is *not* reset: a refresh extends the burn, it does not delay the
            // next tick, which would let a refresh every 0.9 s produce no damage at all.
            let timer = existing.timer;
            *existing = status;
            existing.timer = timer;
            return false;
        }
        self.entries.push(status);
        true
    }

    /// The active status of a kind, if there is one.
    #[must_use]
    pub fn get(&self, kind: StatusKind) -> Option<&Status> {
        self.entries.iter().find(|entry| entry.kind == kind)
    }

    /// Whether a status of this kind is active.
    #[must_use]
    pub fn has(&self, kind: StatusKind) -> bool {
        self.get(kind).is_some()
    }

    /// Seconds left on a status, or zero if it is not there.
    #[must_use]
    pub fn remaining(&self, kind: StatusKind) -> f32 {
        self.get(kind).map_or(0.0, |status| status.remaining)
    }

    /// Removes a status and returns it, for an ability that consumes one.
    pub fn consume(&mut self, kind: StatusKind) -> Option<Status> {
        let index = self.entries.iter().position(|entry| entry.kind == kind)?;
        Some(self.entries.remove(index))
    }

    /// Active statuses.
    pub fn iter(&self) -> impl Iterator<Item = &Status> {
        self.entries.iter()
    }

    /// How many statuses are active.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes everything, keeping the allocation, and reports each removal.
    pub fn clear_into(&mut self, out: &mut Vec<StatusOutcome>) {
        for entry in self.entries.drain(..) {
            out.push(StatusOutcome::Ended {
                kind: entry.kind,
                reason: StatusEnd::Cleared,
            });
        }
    }

    /// Removes everything without reporting it. For arena setup and tests.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Advances every status by one simulation step, appending what happened to `out`.
    ///
    /// A status that expires on the same step it ticks does both, in that order: the last
    /// second of a six-second burn is a second the target was burning, and dropping its
    /// damage would make the total depend on where the tick boundary fell.
    pub fn tick(&mut self, seconds: f32, out: &mut Vec<StatusOutcome>) {
        if seconds <= 0.0 || self.entries.is_empty() {
            return;
        }

        let mut index = 0;
        while index < self.entries.len() {
            let entry = &mut self.entries[index];
            entry.remaining = (entry.remaining - seconds).max(0.0);
            entry.timer -= seconds;
            // Snapped rather than left as a residue: see `crate::TIME_EPSILON`. Without
            // this a six-second burn ticks five times, because the sixty-first subtraction
            // of an inexact 1/60 leaves a few millionths of a second behind.
            if entry.remaining < crate::TIME_EPSILON {
                entry.remaining = 0.0;
            }

            while entry.timer <= crate::TIME_EPSILON {
                entry.timer += STATUS_TICK_INTERVAL;
                if entry.damage_per_tick > 0.0 {
                    out.push(StatusOutcome::Tick {
                        kind: entry.kind,
                        source: entry.source,
                        ability: entry.ability,
                        damage: entry.damage_per_tick,
                    });
                }
            }

            if entry.remaining <= 0.0 {
                let kind = entry.kind;
                self.entries.remove(index);
                out.push(StatusOutcome::Ended {
                    kind,
                    reason: StatusEnd::Expired,
                });
            } else {
                index += 1;
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    fn burn(duration: f32, damage: f32) -> Status {
        Status::new(
            StatusKind::Burning,
            EntityId::new(0, 1),
            AbilityId::from_slot(1).expect("slot 1 exists"),
            duration,
            damage,
        )
    }

    fn total_damage(outcomes: &[StatusOutcome]) -> f32 {
        outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                StatusOutcome::Tick { damage, .. } => Some(*damage),
                StatusOutcome::Ended { .. } => None,
            })
            .sum()
    }

    #[test]
    fn applying_a_status_makes_it_active_with_its_full_duration() {
        let mut statuses = StatusSet::new();
        assert!(statuses.apply(burn(6.0, 8.0)));

        assert!(statuses.has(StatusKind::Burning));
        assert_eq!(statuses.remaining(StatusKind::Burning), 6.0);
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            statuses
                .get(StatusKind::Burning)
                .map(super::Status::fraction),
            Some(1.0)
        );
    }

    #[test]
    fn a_status_ticks_once_a_second_for_its_whole_duration() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();

        for _ in 0..(6 * 60) {
            statuses.tick(STEP, &mut outcomes);
        }

        // Six seconds, one tick a second, eight damage a tick.
        assert!(
            (total_damage(&outcomes) - 48.0).abs() < 1e-3,
            "{}",
            total_damage(&outcomes)
        );
        assert!(!statuses.has(StatusKind::Burning), "the burn expired");
        assert!(outcomes.contains(&StatusOutcome::Ended {
            kind: StatusKind::Burning,
            reason: StatusEnd::Expired,
        }));
    }

    #[test]
    fn the_last_tick_is_not_lost_to_the_expiry_on_the_same_step() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(1.0, 5.0));
        let mut outcomes = Vec::new();
        for _ in 0..60 {
            statuses.tick(STEP, &mut outcomes);
        }
        assert!((total_damage(&outcomes) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn reapplying_refreshes_the_duration_without_stacking() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        for _ in 0..(3 * 60) {
            statuses.tick(STEP, &mut outcomes);
        }
        assert!((statuses.remaining(StatusKind::Burning) - 3.0).abs() < 1e-3);

        assert!(
            !statuses.apply(burn(6.0, 8.0)),
            "a refresh is not a new application"
        );
        assert_eq!(statuses.len(), 1, "statuses refresh rather than stack");
        assert!((statuses.remaining(StatusKind::Burning) - 6.0).abs() < 1e-3);
    }

    #[test]
    fn a_refresh_does_not_postpone_the_next_damage_tick() {
        // Refreshing every 0.9 s while resetting the timer would deal no damage at all.
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();

        for _ in 0..5 {
            for _ in 0..54 {
                statuses.tick(STEP, &mut outcomes);
            }
            statuses.apply(burn(6.0, 8.0));
        }
        assert!(
            total_damage(&outcomes) > 0.0,
            "a refresh loop dealt nothing"
        );
    }

    #[test]
    fn consuming_removes_the_status_and_hands_back_what_was_left() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        for _ in 0..(2 * 60) {
            statuses.tick(STEP, &mut outcomes);
        }

        let consumed = statuses
            .consume(StatusKind::Burning)
            .expect("the burn was still active");
        assert!((consumed.remaining - 4.0).abs() < 1e-3, "{consumed:?}");
        assert!(!statuses.has(StatusKind::Burning));
        assert_eq!(statuses.consume(StatusKind::Burning), None);
    }

    #[test]
    fn clearing_reports_every_removal() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        statuses.clear_into(&mut outcomes);

        assert!(statuses.is_empty());
        assert_eq!(
            outcomes,
            vec![StatusOutcome::Ended {
                kind: StatusKind::Burning,
                reason: StatusEnd::Cleared,
            }]
        );
    }

    #[test]
    fn a_zero_length_step_changes_nothing() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(6.0, 8.0));
        let mut outcomes = Vec::new();
        statuses.tick(0.0, &mut outcomes);
        assert!(outcomes.is_empty());
        assert_eq!(statuses.remaining(StatusKind::Burning), 6.0);
    }

    #[test]
    fn a_status_with_no_damage_still_expires() {
        let mut statuses = StatusSet::new();
        statuses.apply(burn(1.0, 0.0));
        let mut outcomes = Vec::new();
        for _ in 0..60 {
            statuses.tick(STEP, &mut outcomes);
        }
        assert!(!statuses.has(StatusKind::Burning));
        assert_eq!(total_damage(&outcomes), 0.0);
    }
}
