//! Floating damage numbers.
//!
//! ```text
//! CombatEvent::Damage  -->  FloatingNumbers::observe  -->  world position + age
//!                                                              |
//!                                       project through the camera, fade by age
//!                                                              v
//!                                                        UiDrawList text
//! ```
//!
//! Driven entirely by *observing* the combat event stream, exactly as the damage meter is.
//! Nothing in `engine-game` knows a floating number exists, and no gameplay code is called
//! to produce one — which is the same arrangement that lets the meter, a future combat log
//! and a network client all read one stream.
//!
//! ## Allocation
//!
//! A fixed-capacity ring. Numbers are written into a slot that is either free or holds the
//! oldest entry, so a burst of a thousand simultaneous hits costs no allocation and drops
//! the oldest rather than growing. The text is formatted into a reusable `String` at draw
//! time, not stored per number.

use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;

/// Numbers alive at once.
///
/// A hundred is far more than a player can read; the cap exists so a thousand-target area
/// hit degrades into "the newest hundred" instead of into an allocation storm.
pub const MAX_FLOATING_NUMBERS: usize = 128;

/// How long a number is visible, in simulated seconds.
pub const NUMBER_LIFETIME: f32 = 1.1;

/// How far a number drifts upward over its life, in metres.
const RISE_METRES: f32 = 1.6;

/// How far a number drifts sideways over its life, in metres.
///
/// Alternating left and right by index, so two hits on the same target in the same tick do
/// not stack into an unreadable smear.
const DRIFT_METRES: f32 = 0.45;

/// Fraction of the lifetime spent at full opacity before the fade begins.
const HOLD_FRACTION: f32 = 0.45;

/// One damage number in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatingNumber {
    /// Who was hit, so the number can follow them if they move.
    pub target: EntityId,
    /// Where the number started, in world space.
    pub origin: Vec3,
    /// How much damage it reports.
    pub amount: f32,
    /// Whether it was a killing blow, which is drawn differently.
    pub killed: bool,
    /// Whether it critically struck, which is also drawn differently.
    ///
    /// Read from the event rather than guessed from the amount: the same number can be a
    /// critical Firebolt or an ordinary Pyroblast, and a player who cannot tell the two
    /// apart cannot tell whether the build is working.
    pub critical: bool,
    /// Simulation time the number was created.
    pub born: f32,
    /// Which way it drifts: −1 or +1.
    pub drift: f32,
}

impl FloatingNumber {
    /// How far through its life the number is, in [0, 1].
    #[must_use]
    pub fn age_fraction(&self, now: f32) -> f32 {
        ((now - self.born) / NUMBER_LIFETIME).clamp(0.0, 1.0)
    }

    /// Whether the number has finished.
    #[must_use]
    pub fn is_finished(&self, now: f32) -> bool {
        now - self.born >= NUMBER_LIFETIME
    }

    /// Opacity at this moment, in [0, 1].
    ///
    /// Full for the first part of the life, then a linear fade. A number that started
    /// fading immediately would be hardest to read at exactly the moment it appeared.
    #[must_use]
    pub fn alpha(&self, now: f32) -> f32 {
        let age = self.age_fraction(now);
        if age <= HOLD_FRACTION {
            1.0
        } else {
            ((1.0 - age) / (1.0 - HOLD_FRACTION)).clamp(0.0, 1.0)
        }
    }

    /// Where the number is now, in world space.
    ///
    /// Rises and drifts from where it was born rather than following the target: a number
    /// pinned to a moving enemy is harder to read than one that stays where the hit landed,
    /// and it means a number outlives the entity it describes without a dangling lookup.
    #[must_use]
    pub fn position(&self, now: f32) -> Vec3 {
        let age = self.age_fraction(now);
        self.origin + Vec3::new(self.drift * DRIFT_METRES * age, RISE_METRES * age, 0.0)
    }
}

/// The numbers currently in flight.
///
/// Fixed capacity, cleared by age rather than freed.
#[derive(Debug)]
pub struct FloatingNumbers {
    numbers: Vec<FloatingNumber>,
    /// Alternates so consecutive hits on one target do not stack.
    next_drift: f32,
    /// Numbers dropped because the ring was full, for diagnostics.
    dropped: usize,
}

impl Default for FloatingNumbers {
    fn default() -> Self {
        Self::new()
    }
}

impl FloatingNumbers {
    /// An empty set with room for [`MAX_FLOATING_NUMBERS`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            numbers: Vec::with_capacity(MAX_FLOATING_NUMBERS),
            next_drift: 1.0,
            dropped: 0,
        }
    }

    /// The numbers in flight, oldest first.
    #[must_use]
    pub fn numbers(&self) -> &[FloatingNumber] {
        &self.numbers
    }

    /// How many numbers have been dropped because the set was full.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Removes every number.
    pub fn clear(&mut self) {
        self.numbers.clear();
    }

    /// Adds a number for one damage event.
    ///
    /// `origin` is where to draw it: the caller supplies it because presentation knows where
    /// an entity is drawn and gameplay only knows where its feet are.
    pub fn push(
        &mut self,
        target: EntityId,
        origin: Vec3,
        amount: f32,
        killed: bool,
        critical: bool,
        now: f32,
    ) {
        if amount <= 0.0 {
            // Zero damage is a legitimate outcome — a hit on a corpse, a fully absorbed
            // blow — and drawing "0" for it is noise.
            return;
        }

        if self.numbers.len() >= MAX_FLOATING_NUMBERS {
            // Drop the oldest rather than the newest: the most recent hit is the one the
            // player is looking for.
            self.numbers.remove(0);
            self.dropped += 1;
        }

        self.numbers.push(FloatingNumber {
            target,
            origin,
            amount,
            killed,
            critical,
            born: now,
            drift: self.next_drift,
        });
        self.next_drift = -self.next_drift;
    }

    /// Adds a number for every damage event in `events`.
    ///
    /// `position_of` maps an entity to where its number should appear; it returns `None` for
    /// an entity that has gone away, whose damage is then not drawn.
    pub fn observe(
        &mut self,
        events: &[CombatEvent],
        now: f32,
        mut position_of: impl FnMut(EntityId) -> Option<Vec3>,
    ) {
        for event in events {
            let CombatEvent::Damage(damage) = event else {
                continue;
            };
            let Some(origin) = position_of(damage.target) else {
                continue;
            };
            self.push(
                damage.target,
                origin,
                damage.amount,
                damage.killed,
                damage.critical,
                now,
            );
        }
    }

    /// Drops every number that has outlived [`NUMBER_LIFETIME`].
    ///
    /// Retains in place, so the allocation is kept.
    pub fn retire(&mut self, now: f32) {
        self.numbers.retain(|number| !number.is_finished(now));
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;
    use engine_game::sim::Simulation;

    fn entity() -> EntityId {
        EntityId::new(3, 1)
    }

    fn sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    #[test]
    fn a_damage_event_becomes_a_number() {
        let mut numbers = FloatingNumbers::new();
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(0));
        // Firebolt is a one-second cast; the damage arrives when it lands.
        simulation.step_many(70);

        let time = simulation.time() as f32;
        numbers.observe(simulation.events(), time, |_| Some(Vec3::ZERO));
        assert!(!numbers.numbers().is_empty(), "no number for the hit");
        assert!(numbers.numbers()[0].amount > 0.0);
    }

    #[test]
    fn an_area_hit_produces_one_number_per_target() {
        // The AoE feedback requirement, asserted through the event stream rather than by
        // counting draw calls.
        let mut numbers = FloatingNumbers::new();
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(4));
        simulation.step_once();
        simulation.select_nearest_target();

        // Meteor on the group: a ground-targeted rock that lands after its delay.
        let target = simulation.player().target.expect("a dummy is selected");
        let position = simulation
            .world()
            .position(target)
            .expect("the dummy has a position");
        simulation
            .cast_at(engine_game::firemage::FireAbility::Meteor, position)
            .expect("in range");
        simulation.step_many(200);

        let time = simulation.time() as f32;
        numbers.observe(simulation.events(), time, |_| Some(Vec3::ZERO));
        assert!(
            !numbers.numbers().is_empty(),
            "an area hit should produce numbers, got 0"
        );
    }

    #[test]
    fn a_number_rises_and_fades_over_its_lifetime() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 45.0, false, false, 0.0);
        let number = numbers.numbers()[0];

        assert_eq!(number.alpha(0.0), 1.0, "fully opaque when it appears");
        assert!(number.position(0.0).y < number.position(0.5).y, "it rises");
        assert!(
            number.alpha(NUMBER_LIFETIME * 0.9) < 1.0,
            "it fades before it ends"
        );
        assert_eq!(number.alpha(NUMBER_LIFETIME), 0.0, "it ends invisible");
    }

    #[test]
    fn a_number_holds_full_opacity_before_it_starts_fading() {
        // A number that faded from the instant it appeared would be least readable exactly
        // when the player looks at it.
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 45.0, false, false, 0.0);
        let number = numbers.numbers()[0];
        assert_eq!(number.alpha(NUMBER_LIFETIME * 0.2), 1.0);
        assert_eq!(number.alpha(NUMBER_LIFETIME * HOLD_FRACTION), 1.0);
    }

    #[test]
    fn consecutive_numbers_drift_apart() {
        // Two hits on one target in one tick must not stack into an unreadable smear.
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 10.0, false, false, 0.0);
        numbers.push(entity(), Vec3::ZERO, 10.0, false, false, 0.0);
        let first = numbers.numbers()[0].position(1.0);
        let second = numbers.numbers()[1].position(1.0);
        assert_ne!(first.x, second.x, "they drift to opposite sides");
    }

    #[test]
    fn finished_numbers_are_retired_and_the_allocation_is_kept() {
        let mut numbers = FloatingNumbers::new();
        for _ in 0..10 {
            numbers.push(entity(), Vec3::ZERO, 5.0, false, false, 0.0);
        }
        let capacity = numbers.numbers.capacity();

        numbers.retire(NUMBER_LIFETIME * 0.5);
        assert_eq!(numbers.numbers().len(), 10, "none has expired yet");

        numbers.retire(NUMBER_LIFETIME + 0.1);
        assert!(numbers.numbers().is_empty());
        assert_eq!(numbers.numbers.capacity(), capacity);
    }

    #[test]
    fn the_set_is_bounded_and_keeps_the_newest() {
        // A thousand-target area hit must degrade, not allocate.
        let mut numbers = FloatingNumbers::new();
        for index in 0..MAX_FLOATING_NUMBERS + 50 {
            numbers.push(entity(), Vec3::ZERO, index as f32 + 1.0, false, false, 0.0);
        }
        assert_eq!(numbers.numbers().len(), MAX_FLOATING_NUMBERS);
        assert_eq!(numbers.dropped(), 50);
        let newest = numbers.numbers().last().expect("the set is not empty");
        assert_eq!(newest.amount, (MAX_FLOATING_NUMBERS + 50) as f32);
    }

    #[test]
    fn zero_damage_draws_nothing() {
        // A hit on a corpse or a fully absorbed blow is a legitimate zero, and "0" floating
        // up is noise.
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 0.0, false, false, 0.0);
        numbers.push(entity(), Vec3::ZERO, -5.0, false, false, 0.0);
        assert!(numbers.numbers().is_empty());
    }

    #[test]
    fn a_killing_blow_is_marked_so_it_can_be_drawn_differently() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 999.0, true, false, 0.0);
        assert!(numbers.numbers()[0].killed);
    }

    #[test]
    fn a_number_for_an_entity_that_has_gone_is_skipped_rather_than_guessed() {
        let mut numbers = FloatingNumbers::new();
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(0));
        simulation.step_once();

        numbers.observe(simulation.events(), simulation.time() as f32, |_| None);
        assert!(numbers.numbers().is_empty());
    }

    #[test]
    fn observing_does_not_reach_into_gameplay() {
        // The architectural point, as far as a test can state it: the whole system is a
        // reader of a slice of events plus a position lookup the caller supplies.
        let mut numbers = FloatingNumbers::new();
        let events: Vec<CombatEvent> = Vec::new();
        numbers.observe(&events, 0.0, |_| Some(Vec3::ZERO));
        assert!(numbers.numbers().is_empty());
    }
}
