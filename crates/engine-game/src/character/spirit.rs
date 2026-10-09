//! Spirit: the chance to bank a point of Ultimate, and the stacking buff that comes with it.
//!
//! ```text
//! a cast lands ──► roll against Spirit ──► +1 Ultimate point
//!                          │                and one stack of the Spirit buff
//!                          │
//!                          └─► each stack: +2 % main stat, for 10 seconds
//!                              1 stack  +2 %
//!                              2 stacks +4 %
//!                              3 stacks +6 %
//! ```
//!
//! # It is defined, and it does not fire
//!
//! Every hero starts with a Spirit rating of zero, so a character in starting gear cannot
//! proc: [`proc_chance`] of zero is zero, and the roll is strictly less-than. Displaying
//! Spirit on the character sheet therefore cannot cause a proc, which is the property the
//! sheet has to have — a stat is a number, not a trigger. Only gear that grants Spirit (see
//! the loot pool in [`super::item`]) turns the mechanic on.
//!
//! # Each stack keeps its own clock
//!
//! Stacks do not share a timer and applying one does not refresh the others: a stack lasts
//! ten seconds from the moment it was gained, so a burst of four leaves one at a time rather
//! than all four at once. That is what "stacks, each lasting 10 seconds" means, and it is
//! why this is an array of remaining times rather than a count and one timer.

use super::stat::{Stat, StatBlock, secondary_fraction};

/// How long one stack of the Spirit buff lasts, in seconds.
pub const SPIRIT_BUFF_DURATION: f32 = 10.0;

/// How much of the main stat one stack is worth, as a fraction.
///
/// Two per cent per stack: one stack is +2 %, two are +4 %, three are +6 %.
pub const SPIRIT_MAIN_STAT_PER_STACK: f32 = 0.02;

/// Ultimate points one proc hands over.
pub const SPIRIT_ULTIMATE_POINTS: u32 = 1;

/// How many stacks may be held at once.
///
/// Ten, which at [`SPIRIT_MAIN_STAT_PER_STACK`] is +20 % — a ceiling rather than a tuning
/// decision. It exists so the buff is a bounded array with no allocation and no unbounded
/// multiplier, not because a player is expected to reach it.
pub const SPIRIT_MAX_STACKS: usize = 10;

/// The chance one cast procs Spirit, as a fraction, for a given Spirit rating.
///
/// The shared secondary-stat curve [`secondary_fraction`], `x / (x + 100)`, so Spirit is
/// comparable with the other three ratings. Zero rating is zero chance, which is what keeps
/// the mechanic inert on a character wearing nothing that grants it.
#[must_use]
pub fn proc_chance(spirit_rating: f32) -> f32 {
    if !spirit_rating.is_finite() || spirit_rating <= 0.0 {
        return 0.0;
    }
    secondary_fraction(spirit_rating)
}

/// What one proc handed over.
///
/// Returned rather than applied, because Ultimate Energy belongs to the class and the buff
/// belongs here. The caller adds the points; this type keeps the stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpiritProc {
    /// Ultimate points to add.
    pub ultimate_points: u32,
    /// How many stacks are running after this one landed.
    pub stacks: u8,
}

/// The stacking main-stat buff Spirit grants, and its clocks.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SpiritSurge {
    /// Seconds left on each stack. Zero means the entry is not a stack.
    remaining: [f32; SPIRIT_MAX_STACKS],
}

impl SpiritSurge {
    /// No stacks.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            remaining: [0.0; SPIRIT_MAX_STACKS],
        }
    }

    /// How many stacks are running.
    #[must_use]
    pub fn stacks(&self) -> u8 {
        // Saturating rather than `as`: the cap is ten, so the cast is exact, and saying so
        // keeps it exact if the cap ever changes.
        u8::try_from(self.remaining.iter().filter(|left| **left > 0.0).count()).unwrap_or(u8::MAX)
    }

    /// Whether any stack is running.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.stacks() > 0
    }

    /// Seconds left on the stack that expires next. Zero when there are none.
    #[must_use]
    pub fn next_expiry(&self) -> f32 {
        self.remaining
            .iter()
            .copied()
            .filter(|left| *left > 0.0)
            .fold(0.0_f32, |soonest, left| {
                if soonest == 0.0 {
                    left
                } else {
                    soonest.min(left)
                }
            })
    }

    /// Adds one stack at full duration, if there is room. Answers the stack count after.
    ///
    /// At the cap the shortest-lived stack is refreshed rather than the application being
    /// dropped, so a player at ten stacks who keeps proccing keeps ten rather than watching
    /// them fall off under a full uptime.
    pub fn apply(&mut self) -> u8 {
        let free = self.remaining.iter().position(|left| *left <= 0.0);
        match free {
            Some(index) => self.remaining[index] = SPIRIT_BUFF_DURATION,
            None => {
                if let Some(shortest) = self
                    .remaining
                    .iter_mut()
                    .min_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal))
                {
                    *shortest = SPIRIT_BUFF_DURATION;
                }
            }
        }
        self.stacks()
    }

    /// Rolls for a proc against `spirit_rating`, using `roll` from `[0, 1)`.
    ///
    /// The roll is a parameter rather than a random source of its own: every chance in this
    /// project is rolled against the simulation's seeded generator, so a replay of the same
    /// commands produces the same procs.
    pub fn roll(&mut self, spirit_rating: f32, roll: f32) -> Option<SpiritProc> {
        if roll >= proc_chance(spirit_rating) {
            return None;
        }
        Some(SpiritProc {
            ultimate_points: SPIRIT_ULTIMATE_POINTS,
            stacks: self.apply(),
        })
    }

    /// Advances every stack's clock by `delta` seconds and drops the ones that ran out.
    pub fn tick(&mut self, delta: f32) {
        if !delta.is_finite() || delta <= 0.0 {
            return;
        }
        for left in &mut self.remaining {
            if *left > 0.0 {
                *left = (*left - delta).max(0.0);
            }
        }
    }

    /// Drops every stack. What a hero swap and a fight reset do.
    pub fn clear(&mut self) {
        self.remaining = [0.0; SPIRIT_MAX_STACKS];
    }

    /// What the main stat is multiplied by right now. One when nothing is running.
    #[must_use]
    pub fn main_stat_multiplier(&self) -> f32 {
        1.0 + f32::from(self.stacks()) * SPIRIT_MAIN_STAT_PER_STACK
    }

    /// Applies the buff to `stats`, scaling `main_stat` and nothing else.
    ///
    /// **The one place the buff reaches a number.** The character sheet, a damage
    /// calculation and a tooltip all go through it, so none of them can disagree about what
    /// three stacks are worth.
    pub fn apply_to(&self, stats: &mut StatBlock, main_stat: Stat) {
        let multiplier = self.main_stat_multiplier();
        if multiplier != 1.0 {
            stats.scale(main_stat, multiplier);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_spirit_can_never_proc() {
        let mut surge = SpiritSurge::new();
        // Every roll a generator can produce, including the one at the very bottom.
        for roll in [0.0, f32::EPSILON, 0.5, 0.999_999] {
            assert_eq!(surge.roll(0.0, roll), None);
        }
        assert_eq!(surge.stacks(), 0);
        assert_eq!(proc_chance(0.0), 0.0);
        assert_eq!(proc_chance(-50.0), 0.0);
    }

    #[test]
    fn a_rating_buys_a_chance_and_the_chance_is_bounded() {
        assert!((proc_chance(2.0) - 2.0 / 102.0).abs() < 1e-6);
        assert!((proc_chance(100.0) - 0.5).abs() < 1e-6);
        assert!(proc_chance(1.0e9) <= 1.0);
    }

    #[test]
    fn a_proc_hands_over_a_point_and_a_stack() {
        let mut surge = SpiritSurge::new();
        let proc = surge.roll(100.0, 0.0);
        assert_eq!(
            proc,
            Some(SpiritProc {
                ultimate_points: SPIRIT_ULTIMATE_POINTS,
                stacks: 1,
            })
        );
    }

    #[test]
    fn each_stack_is_worth_two_per_cent_of_the_main_stat() {
        let mut surge = SpiritSurge::new();
        assert_eq!(surge.main_stat_multiplier(), 1.0);
        for expected in 1..=3 {
            assert_eq!(surge.apply(), expected);
        }
        assert!((surge.main_stat_multiplier() - 1.06).abs() < 1e-6);

        let mut stats = StatBlock::of(Stat::Intellect, 100.0);
        stats.set(Stat::Stamina, 50.0);
        surge.apply_to(&mut stats, Stat::Intellect);
        assert!((stats.get(Stat::Intellect) - 106.0).abs() < 1e-4);
        assert_eq!(stats.get(Stat::Stamina), 50.0, "only the main stat scales");
    }

    #[test]
    fn a_stack_lasts_ten_seconds_from_when_it_was_gained() {
        let mut surge = SpiritSurge::new();
        surge.apply();
        surge.tick(4.0);
        surge.apply();
        assert_eq!(surge.stacks(), 2);

        // Six more seconds finishes the first and leaves the second with four.
        surge.tick(6.0);
        assert_eq!(surge.stacks(), 1);
        assert!((surge.next_expiry() - 4.0).abs() < 1e-5);

        surge.tick(SPIRIT_BUFF_DURATION);
        assert_eq!(surge.stacks(), 0);
        assert!(!surge.is_active());
    }

    #[test]
    fn stacks_are_capped_and_the_cap_refreshes_rather_than_drops() {
        let mut surge = SpiritSurge::new();
        for _ in 0..SPIRIT_MAX_STACKS {
            surge.apply();
        }
        assert_eq!(usize::from(surge.stacks()), SPIRIT_MAX_STACKS);

        surge.tick(5.0);
        surge.apply();
        assert_eq!(usize::from(surge.stacks()), SPIRIT_MAX_STACKS);
        // The one that was refreshed is the one that had least left.
        assert!((surge.next_expiry() - 5.0).abs() < 1e-5);
    }

    #[test]
    fn clearing_drops_everything() {
        let mut surge = SpiritSurge::new();
        surge.apply();
        surge.clear();
        assert_eq!(surge.stacks(), 0);
        assert_eq!(surge.next_expiry(), 0.0);
    }
}
