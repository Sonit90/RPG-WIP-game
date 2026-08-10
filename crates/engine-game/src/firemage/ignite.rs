//! Ignite: a stacking burn that never explodes on its own.
//!
//! ```text
//! apply -> stacks + 1 (up to the maximum), duration refreshed
//! at maximum -> further applications do nothing until Fire Detonation consumes them
//! ```
//!
//! The maximum is **not** a constant here. It is passed in on every call, because the
//! Infernal Capacity talent raises it and every part of the game - the detonation
//! requirement, the tooltips, the stack display - has to agree about what it currently is.
//! A `const MAX_STACKS: u8 = 5` anywhere in the codebase would be a bug waiting for that
//! talent to be bought.
//!
//! Reaching the maximum triggers nothing. That is the class's central decision: the payoff
//! is spent by the player with Fire Detonation, not collected automatically by the engine.

use crate::entity::EntityId;

/// Ignite stacks a target starts with when a fresh burn lands.
pub const IGNITE_BASE_MAX_STACKS: u8 = 5;

/// How long a stack lasts before the whole burn falls off, in seconds.
pub const IGNITE_BASE_DURATION: f32 = 8.0;

/// Damage one stack deals per second.
///
/// The burn is a floor of damage, not the class's output: the abilities are. It exists so a
/// stack is worth something before it is detonated.
pub const IGNITE_DAMAGE_PER_STACK_PER_SECOND: f32 = 12.0;

/// How often the burn deals its damage, in seconds.
pub const IGNITE_TICK_INTERVAL: f32 = 1.0;

/// One target's burn.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Ignite {
    /// How many stacks are on the target. Zero means it is not burning.
    pub stacks: u8,
    /// Seconds before the burn falls off entirely.
    pub remaining: f32,
    /// Seconds until the next damage tick.
    pub until_tick: f32,
}

impl Ignite {
    /// Whether the target is burning at all.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.stacks > 0 && self.remaining > 0.0
    }

    /// Whether the target is at `maximum` stacks - the Fire Detonation requirement.
    #[must_use]
    pub fn is_at_maximum(&self, maximum: u8) -> bool {
        self.is_active() && self.stacks >= maximum
    }

    /// How much damage one tick of this burn deals.
    #[must_use]
    pub fn tick_damage(&self) -> f32 {
        f32::from(self.stacks) * IGNITE_DAMAGE_PER_STACK_PER_SECOND * IGNITE_TICK_INTERVAL
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
            *burn = Ignite {
                stacks: 0,
                remaining: 0.0,
                until_tick: IGNITE_TICK_INTERVAL,
            };
        }
        let before = burn.stacks;
        burn.stacks = burn.stacks.saturating_add(count).min(maximum);
        burn.remaining = duration;
        burn.stacks - before
    }

    /// Removes the burn and returns how many stacks were on it.
    ///
    /// `retain` leaves that many stacks behind, which is what the Lingering Embers talent
    /// does after a detonation. The consumed count is what the explosion is scaled by, so a
    /// retained stack is not counted twice.
    pub fn consume(&mut self, target: EntityId, retain: u8, duration: f32) -> u8 {
        let slot = target.index();
        let Some(burn) = self.burns.get_mut(slot) else {
            return 0;
        };
        let consumed = burn.stacks;
        if consumed == 0 {
            return 0;
        }
        if retain > 0 {
            *burn = Ignite {
                stacks: retain.min(consumed),
                remaining: duration,
                until_tick: IGNITE_TICK_INTERVAL,
            };
        } else {
            *burn = Ignite::default();
        }
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

    /// Advances every burn, collecting the ticks that fired into `out`.
    ///
    /// Damage is not dealt here: this module owns what a burn *is*, and the simulation owns
    /// what damage *does* - events, the meter, deaths. A tick is reported, not applied.
    pub fn tick(&mut self, seconds: f32, out: &mut Vec<IgniteTick>) {
        for (slot, burn) in self.burns.iter_mut().enumerate() {
            if !burn.is_active() {
                continue;
            }
            burn.remaining -= seconds;
            burn.until_tick -= seconds;
            if burn.until_tick <= 0.0 {
                burn.until_tick += IGNITE_TICK_INTERVAL;
                out.push(IgniteTick {
                    slot,
                    stacks: burn.stacks,
                    damage: burn.tick_damage(),
                });
            }
            if burn.remaining <= 0.0 {
                *burn = Ignite::default();
            }
        }
    }
}

/// One burn dealing its damage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IgniteTick {
    /// World slot of the burning entity.
    pub slot: usize,
    /// Stacks at the moment it ticked.
    pub stacks: u8,
    /// Damage the tick deals before any modifier.
    pub damage: f32,
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

        // Further applications land nothing, and nothing explodes.
        assert_eq!(tracker.apply(target, 1, MAX, DURATION), 0);
        assert_eq!(tracker.stacks(target), MAX);
        assert_eq!(tracker.apply(target, 3, MAX, DURATION), 0);
        assert_eq!(tracker.stacks(target), MAX);
    }

    #[test]
    fn the_maximum_is_whatever_the_caller_says_it_is() {
        // The Infernal Capacity property: nothing here knows the number five.
        let mut tracker = IgniteTracker::new();
        let target = entity(1);
        for _ in 0..10 {
            tracker.apply(target, 1, 6, DURATION);
        }
        assert_eq!(tracker.stacks(target), 6);
        assert!(tracker.is_at_maximum(target, 6));
        assert!(tracker.is_at_maximum(target, 5), "six is at least five");

        let mut small = IgniteTracker::new();
        for _ in 0..10 {
            small.apply(target, 1, 2, DURATION);
        }
        assert_eq!(small.stacks(target), 2);
    }

    #[test]
    fn applying_several_stacks_at_once_clamps_at_the_maximum() {
        let mut tracker = IgniteTracker::new();
        let target = entity(2);
        assert_eq!(tracker.apply(target, 3, MAX, DURATION), 3);
        assert_eq!(tracker.apply(target, 3, MAX, DURATION), 2, "clamped");
        assert_eq!(tracker.stacks(target), MAX);
    }

    #[test]
    fn a_new_stack_refreshes_the_duration() {
        let mut tracker = IgniteTracker::new();
        let target = entity(4);
        tracker.apply(target, 1, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(5.0, &mut ticks);
        assert!(tracker.get(target).remaining < 4.0);

        tracker.apply(target, 1, MAX, DURATION);
        assert_eq!(tracker.get(target).remaining, DURATION);
    }

    #[test]
    fn a_burn_at_the_cap_still_has_its_duration_refreshed() {
        // Otherwise a target held at maximum stacks would fall off mid-rotation, which is
        // the opposite of what stacking it up is for.
        let mut tracker = IgniteTracker::new();
        let target = entity(5);
        for _ in 0..MAX {
            tracker.apply(target, 1, MAX, DURATION);
        }
        let mut ticks = Vec::new();
        tracker.tick(4.0, &mut ticks);

        assert_eq!(
            tracker.apply(target, 1, MAX, DURATION),
            0,
            "no stack landed"
        );
        assert_eq!(tracker.get(target).remaining, DURATION, "but it refreshed");
    }

    #[test]
    fn a_burn_expires_on_its_own_and_leaves_nothing_behind() {
        let mut tracker = IgniteTracker::new();
        let target = entity(6);
        tracker.apply(target, 2, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(DURATION + 0.1, &mut ticks);
        assert!(!tracker.is_ignited(target));
        assert_eq!(tracker.stacks(target), 0);
    }

    #[test]
    fn ticks_fire_on_the_interval_and_scale_with_the_stack_count() {
        let mut tracker = IgniteTracker::new();
        let target = entity(7);
        tracker.apply(target, 3, MAX, DURATION);

        let mut ticks = Vec::new();
        tracker.tick(IGNITE_TICK_INTERVAL, &mut ticks);
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].slot, 7);
        assert_eq!(ticks[0].stacks, 3);
        assert!((ticks[0].damage - 3.0 * IGNITE_DAMAGE_PER_STACK_PER_SECOND).abs() < 1e-3);

        // Half an interval later, nothing has fired again.
        ticks.clear();
        tracker.tick(IGNITE_TICK_INTERVAL * 0.5, &mut ticks);
        assert!(ticks.is_empty());
    }

    #[test]
    fn consuming_takes_every_stack_and_reports_how_many_there_were() {
        let mut tracker = IgniteTracker::new();
        let target = entity(8);
        tracker.apply(target, MAX, MAX, DURATION);

        assert_eq!(tracker.consume(target, 0, DURATION), MAX);
        assert_eq!(tracker.stacks(target), 0);
        assert!(!tracker.is_ignited(target));
        assert_eq!(tracker.consume(target, 0, DURATION), 0, "twice");
    }

    #[test]
    fn consuming_can_leave_a_stack_behind() {
        // Lingering Embers. The consumed count is still the full burn, so the explosion is
        // not quietly weakened by the stack that stays.
        let mut tracker = IgniteTracker::new();
        let target = entity(9);
        tracker.apply(target, MAX, MAX, DURATION);

        assert_eq!(tracker.consume(target, 1, DURATION), MAX);
        assert_eq!(tracker.stacks(target), 1);
        assert_eq!(tracker.get(target).remaining, DURATION);
    }

    #[test]
    fn burns_are_independent_between_targets() {
        let mut tracker = IgniteTracker::new();
        tracker.apply(entity(0), MAX, MAX, DURATION);
        tracker.apply(entity(4), 1, MAX, DURATION);

        assert_eq!(tracker.stacks(entity(0)), MAX);
        assert_eq!(tracker.stacks(entity(4)), 1);
        assert_eq!(tracker.stacks(entity(2)), 0, "an untouched slot");

        tracker.consume(entity(0), 0, DURATION);
        assert_eq!(tracker.stacks(entity(4)), 1, "the other burn was consumed");
    }

    #[test]
    fn clearing_removes_burns_without_reporting_ticks() {
        let mut tracker = IgniteTracker::new();
        tracker.apply(entity(1), 2, MAX, DURATION);
        tracker.clear(entity(1));
        assert!(!tracker.is_ignited(entity(1)));

        tracker.apply(entity(2), 2, MAX, DURATION);
        tracker.clear_all();
        assert!(!tracker.is_ignited(entity(2)));

        let mut ticks = Vec::new();
        tracker.tick(10.0, &mut ticks);
        assert!(ticks.is_empty());
    }

    #[test]
    fn nothing_happens_automatically_at_the_maximum() {
        // The class's central rule, asserted where it lives: reaching the cap produces no
        // tick, no event and no removal. Only Fire Detonation empties a burn.
        let mut tracker = IgniteTracker::new();
        let target = entity(3);
        let mut ticks = Vec::new();
        for _ in 0..MAX {
            tracker.apply(target, 1, MAX, DURATION);
        }
        assert!(tracker.is_at_maximum(target, MAX));
        assert!(ticks.is_empty());

        tracker.tick(0.0, &mut ticks);
        assert!(tracker.is_at_maximum(target, MAX), "it detonated itself");
    }
}
