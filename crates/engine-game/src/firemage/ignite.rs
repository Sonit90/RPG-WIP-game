//! Ignite: a long stacking burn that is a *bank* of damage, and never explodes on its own.
//!
//! ```text
//! apply -> stacks + 1 (up to the maximum), duration refreshed
//! at maximum -> further applications do nothing until Detonate consumes them
//! ```
//!
//! The maximum is **not** a constant here. It is passed in on every call, because the
//! Infernal Capacity talent raises it and every part of the game - the Detonate requirement,
//! the tooltips, the stack display - has to agree about what it currently is. A
//! `const MAX_STACKS: u8 = 5` anywhere in the codebase would be a bug waiting for that
//! talent to be bought.
//!
//! ## No snapshot, structurally
//!
//! A burn stores three numbers: how many stacks, how long it has left, and how far it is
//! through its current tick **as a fraction**. It does not store haste, critical strike
//! chance, spell power or any damage multiplier, because there is nowhere to put them. The
//! interval is handed in on every [`IgniteTracker::tick`] and the damage is computed by the
//! simulation from the stats of the moment the tick fires, so a burn applied at 10 % haste
//! ticks at 30 % haste the instant the mage has it, mid-burn, with no reapplication.
//!
//! Keeping the phase as a fraction rather than as seconds is what makes that work: a burn
//! 40 % of the way to its next tick stays 40 % of the way there when the interval changes,
//! rather than jumping forwards or backwards.

use crate::entity::EntityId;

/// Maximum Ignite stacks before talents.
pub const IGNITE_BASE_MAX_STACKS: u8 = 5;

/// How long a burn lasts before it falls off, in seconds.
pub const IGNITE_BASE_DURATION: f32 = 30.0;

/// Seconds between ticks, before haste.
pub const IGNITE_BASE_TICK_INTERVAL: f32 = 2.0;

/// Spell power one stack deals per tick.
///
/// One stack over a full base duration is `0.20 x 15 ticks = 300 %` spell power; five
/// stacks are `1500 %`. That is the bank Detonate cashes in.
pub const IGNITE_SPELL_POWER_PER_STACK: f32 = 0.24;

/// One target's burn.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Ignite {
    /// How many stacks are on the target. Zero means it is not burning.
    pub stacks: u8,
    /// Seconds before the burn falls off entirely.
    pub remaining: f32,
    /// How far through the current tick it is, in `0.0..1.0`.
    ///
    /// A fraction rather than seconds, so a change of haste rescales the interval without
    /// moving the burn's position within it.
    pub phase: f32,
}

impl Ignite {
    /// Whether the target is burning at all.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.stacks > 0 && self.remaining > 0.0
    }

    /// Whether the target is at `maximum` stacks - the Detonate requirement.
    #[must_use]
    pub fn is_at_maximum(&self, maximum: u8) -> bool {
        self.is_active() && self.stacks >= maximum
    }

    /// Spell power one tick of this burn is worth, before every multiplier.
    #[must_use]
    pub fn tick_coefficient(&self, damage_scale: f32) -> f32 {
        f32::from(self.stacks) * IGNITE_SPELL_POWER_PER_STACK * damage_scale
    }

    /// How many ticks are left at the current interval, fractions included.
    ///
    /// Fractional on purpose: a burn with 1.4 ticks left is worth 1.4 ticks, so the value of
    /// detonating it decays smoothly instead of in steps.
    #[must_use]
    pub fn ticks_remaining(&self, interval: f32) -> f32 {
        if !self.is_active() || interval <= 0.0 {
            return 0.0;
        }
        self.remaining / interval
    }

    /// Spell power the rest of this burn would deal if it ran out.
    ///
    /// **What Detonate converts.** Remaining, not theoretical: a burn three seconds from
    /// falling off is worth three seconds, which is what makes the timing of a detonation a
    /// decision rather than a formality.
    #[must_use]
    pub fn remaining_coefficient(&self, interval: f32, damage_scale: f32) -> f32 {
        self.tick_coefficient(damage_scale) * self.ticks_remaining(interval)
    }
}

/// Every target's Ignite, indexed by world slot.
///
/// A slot-indexed vector rather than a map: the world already numbers its entities densely,
/// a burn is twelve bytes, and a `HashMap<EntityId, Ignite>` would allocate to hold what a
/// dozen dummies fit in a cache line.
#[derive(Debug, Default, Clone)]
pub struct IgniteTracker {
    burns: Vec<Ignite>,
}

impl IgniteTracker {
    /// An empty tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The burn on `target`, which may be no burn at all.
    #[must_use]
    pub fn get(&self, target: EntityId) -> Ignite {
        self.burns.get(target.index()).copied().unwrap_or_default()
    }

    /// How many stacks `target` is carrying.
    #[must_use]
    pub fn stacks(&self, target: EntityId) -> u8 {
        self.get(target).stacks
    }

    /// Whether `target` is burning.
    #[must_use]
    pub fn is_ignited(&self, target: EntityId) -> bool {
        self.get(target).is_active()
    }

    /// Whether `target` is at the current maximum.
    #[must_use]
    pub fn is_at_maximum(&self, target: EntityId, maximum: u8) -> bool {
        self.get(target).is_at_maximum(maximum)
    }

    /// Applies `count` stacks, refreshing the duration. Returns how many actually landed.
    ///
    /// Landing fewer than asked is the ordinary case at the cap, and it is *not* a failure:
    /// the duration still refreshes, because the target was hit by something burning.
    pub fn apply(&mut self, target: EntityId, count: u8, maximum: u8, duration: f32) -> u8 {
        let slot = target.index();
        if self.burns.len() <= slot {
            self.burns.resize(slot + 1, Ignite::default());
        }
        let burn = &mut self.burns[slot];
        if !burn.is_active() {
            *burn = Ignite::default();
        }
        let before = burn.stacks;
        burn.stacks = burn.stacks.saturating_add(count).min(maximum);
        burn.remaining = duration;
        burn.stacks - before
    }

    /// Removes the burn and returns how many stacks were on it.
    pub fn consume(&mut self, target: EntityId) -> u8 {
        let Some(burn) = self.burns.get_mut(target.index()) else {
            return 0;
        };
        let consumed = burn.stacks;
        *burn = Ignite::default();
        consumed
    }

    /// Removes the burn entirely, for a death or a reset.
    pub fn clear(&mut self, target: EntityId) {
        if let Some(burn) = self.burns.get_mut(target.index()) {
            *burn = Ignite::default();
        }
    }

    /// Removes every burn.
    pub fn clear_all(&mut self) {
        for burn in &mut self.burns {
            *burn = Ignite::default();
        }
    }

    /// Advances every burn by `seconds` at the *current* `interval`, reporting its ticks.
    ///
    /// Damage is not dealt here and is not even computed here: this module owns what a burn
    /// *is*, and the simulation owns what a tick is worth - which is the only way the damage
    /// can be evaluated against the stats of the moment rather than the stats of the
    /// application.
    pub fn tick(&mut self, seconds: f32, interval: f32, out: &mut Vec<IgniteTick>) {
        let interval = interval.max(0.01);
        for (slot, burn) in self.burns.iter_mut().enumerate() {
            if !burn.is_active() {
                continue;
            }
            burn.remaining -= seconds;
            burn.phase += seconds / interval;
            while burn.phase >= 1.0 {
                burn.phase -= 1.0;
                out.push(IgniteTick {
                    slot,
                    stacks: burn.stacks,
                });
            }
            if burn.remaining <= 0.0 {
                *burn = Ignite::default();
            }
        }
    }
}

/// One burn dealing its damage.
///
/// Carries no damage number: what a tick is worth is decided by the simulation from the
/// stats it fires under.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IgniteTick {
    /// World slot of the burning entity.
    pub slot: usize,
    /// Stacks at the moment it ticked.
    pub stacks: u8,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn entity(index: u32) -> EntityId {
        EntityId::new(index, 1)
    }

    const MAX: u8 = IGNITE_BASE_MAX_STACKS;
    const DURATION: f32 = IGNITE_BASE_DURATION;
    const INTERVAL: f32 = IGNITE_BASE_TICK_INTERVAL;

    #[test]
    fn a_fresh_target_takes_a_stack_and_starts_burning() {
        let mut tracker = IgniteTracker::new();
        let target = entity(3);
        assert!(!tracker.is_ignited(target));

        assert_eq!(tracker.apply(target, 1, MAX, DURATION), 1);
        assert_eq!(tracker.stacks(target), 1);
        assert!(tracker.is_ignited(target));
        assert!(!tracker.is_at_maximum(target, MAX));
    }

    #[test]
    fn stacks_accumulate_up_to_the_maximum_and_stop_there() {
        let mut tracker = IgniteTracker::new();
        let target = entity(0);
        for expected in 1..=MAX {
            tracker.apply(target, 1, MAX, DURATION);
            assert_eq!(tracker.stacks(target), expected);
        }
        assert!(tracker.is_at_maximum(target, MAX));

        assert_eq!(tracker.apply(target, 1, MAX, DURATION), 0);
        assert_eq!(tracker.stacks(target), MAX);
        assert_eq!(tracker.apply(target, 3, MAX, DURATION), 0);
        assert_eq!(tracker.stacks(target), MAX);
    }

    #[test]
    fn the_maximum_is_whatever_the_caller_says_it_is() {
        let mut tracker = IgniteTracker::new();
        let target = entity(1);
        for _ in 0..10 {
            tracker.apply(target, 1, 6, DURATION);
        }
        assert_eq!(tracker.stacks(target), 6);
        assert!(tracker.is_at_maximum(target, 6));
    }

    #[test]
    fn a_burn_at_the_cap_still_has_its_duration_refreshed() {
        let mut tracker = IgniteTracker::new();
        let target = entity(5);
        tracker.apply(target, MAX, MAX, DURATION);
        let mut ticks = Vec::new();
        tracker.tick(4.0, INTERVAL, &mut ticks);

        assert_eq!(tracker.apply(target, 1, MAX, DURATION), 0, "a stack landed");
        assert_eq!(tracker.get(target).remaining, DURATION, "but it refreshed");
    }

    #[test]
    fn a_burn_expires_on_its_own_and_leaves_nothing_behind() {
        let mut tracker = IgniteTracker::new();
        let target = entity(6);
        tracker.apply(target, 2, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(DURATION + 0.1, INTERVAL, &mut ticks);
        assert!(!tracker.is_ignited(target));
    }

    #[test]
    fn ticks_fire_on_the_interval_and_report_the_stack_count() {
        let mut tracker = IgniteTracker::new();
        let target = entity(7);
        tracker.apply(target, 3, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(INTERVAL, INTERVAL, &mut ticks);
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].slot, 7);
        assert_eq!(ticks[0].stacks, 3);

        ticks.clear();
        tracker.tick(INTERVAL * 0.5, INTERVAL, &mut ticks);
        assert!(ticks.is_empty());
    }

    #[test]
    fn haste_makes_an_already_running_burn_tick_faster() {
        // The no-snapshot property, asserted where it lives. The burn was applied without
        // any haste and is handed a shorter interval mid-life; it must speed up at once.
        let mut tracker = IgniteTracker::new();
        let target = entity(2);
        tracker.apply(target, 1, MAX, DURATION);

        let mut slow = Vec::new();
        tracker.tick(4.0, 2.0, &mut slow);
        assert_eq!(slow.len(), 2, "two seconds a tick over four seconds");

        let mut fast = Vec::new();
        tracker.tick(4.0, 1.0, &mut fast);
        assert_eq!(fast.len(), 4, "the burn ignored the new interval");
    }

    #[test]
    fn a_change_of_interval_keeps_the_burns_place_within_it() {
        // Half way through a two-second tick is half way through a one-second tick, not
        // "one second in" - otherwise a haste buff would skip or repeat a tick.
        let mut tracker = IgniteTracker::new();
        let target = entity(4);
        tracker.apply(target, 1, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(1.0, 2.0, &mut ticks);
        assert!(ticks.is_empty());
        assert!((tracker.get(target).phase - 0.5).abs() < 1e-5);

        // Now at a one-second interval: half of it is left, so half a second fires it.
        tracker.tick(0.5, 1.0, &mut ticks);
        assert_eq!(ticks.len(), 1);
    }

    #[test]
    fn a_burns_worth_is_what_is_left_of_it_not_what_it_started_as() {
        let mut tracker = IgniteTracker::new();
        let target = entity(9);
        tracker.apply(target, MAX, MAX, DURATION);

        let full = tracker.get(target).remaining_coefficient(INTERVAL, 1.0);
        let expected = f32::from(MAX) * IGNITE_SPELL_POWER_PER_STACK * (DURATION / INTERVAL);
        assert!((full - expected).abs() < 1e-3, "{full} vs {expected}");

        let mut ticks = Vec::new();
        tracker.tick(DURATION * 0.5, INTERVAL, &mut ticks);
        let half = tracker.get(target).remaining_coefficient(INTERVAL, 1.0);
        assert!((half - full * 0.5).abs() < 0.5, "{half} vs {full}");
    }

    #[test]
    fn consuming_takes_every_stack_and_reports_how_many_there_were() {
        let mut tracker = IgniteTracker::new();
        let target = entity(8);
        tracker.apply(target, MAX, MAX, DURATION);

        assert_eq!(tracker.consume(target), MAX);
        assert_eq!(tracker.stacks(target), 0);
        assert_eq!(tracker.consume(target), 0, "twice");
    }

    #[test]
    fn burns_are_independent_between_targets() {
        let mut tracker = IgniteTracker::new();
        tracker.apply(entity(0), MAX, MAX, DURATION);
        tracker.apply(entity(4), 1, MAX, DURATION);

        assert_eq!(tracker.stacks(entity(0)), MAX);
        assert_eq!(tracker.stacks(entity(4)), 1);
        tracker.consume(entity(0));
        assert_eq!(tracker.stacks(entity(4)), 1);
    }

    #[test]
    fn nothing_happens_automatically_at_the_maximum() {
        // The class's central rule, asserted where it lives.
        let mut tracker = IgniteTracker::new();
        let target = entity(3);
        for _ in 0..MAX {
            tracker.apply(target, 1, MAX, DURATION);
        }
        assert!(tracker.is_at_maximum(target, MAX));
        let mut ticks = Vec::new();
        tracker.tick(0.0, INTERVAL, &mut ticks);
        assert!(tracker.is_at_maximum(target, MAX), "it detonated itself");
    }
}
