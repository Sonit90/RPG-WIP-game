//! Health, factions and damage types.
//!
//! [`Health`] is the whole of an entity's survivability: a current value, a maximum, and
//! the rules for changing them. It is deliberately not an event consumer — nothing here
//! knows what an ability is, and nothing here records a statistic. Damage arrives as a
//! number, the result comes back as a number, and the caller turns that into a
//! [`crate::event::CombatEvent`] for everyone else to observe.

use crate::entity::EntityId;

/// Whose side an entity is on.
///
/// Two factions are enough for a training arena and enough to write the targeting rules
/// against. A faction is not a team id: a future free-for-all needs a different relation,
/// and [`Faction::is_hostile_to`] is the single place that would change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Faction {
    /// The player and anything fighting alongside them.
    Player,
    /// Anything the player may attack. Training dummies are hostile so the normal targeting
    /// rules apply to them rather than a special case.
    Hostile,
    /// Scenery that takes no part in combat.
    Neutral,
}

impl Faction {
    /// Whether `self` may attack `other`.
    #[must_use]
    pub fn is_hostile_to(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Player, Self::Hostile) | (Self::Hostile, Self::Player)
        )
    }
}

/// What kind of damage was dealt.
///
/// Carried through the event stream so telemetry and a future resistance system can act on
/// it. Nothing in Phase 3 changes behaviour based on it, which is the point: adding
/// resistances later must not require changing the shape of a damage event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DamageType {
    /// Weapons and impacts.
    Physical,
    /// Burning, including damage over time from [`crate::status::StatusKind::Burning`].
    Fire,
    /// Everything magical that is not fire.
    Arcane,
    /// Life, moonlight and the things that grow. The Druid's school.
    ///
    /// Its own school rather than a second name for Arcane, because Nature's Exposure
    /// amplifies both and a debuff that amplified one school twice would be a debuff that
    /// could not be written down.
    Nature,
}

impl DamageType {
    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Physical => "PHYSICAL",
            Self::Fire => "FIRE",
            Self::Arcane => "ARCANE",
            Self::Nature => "NATURE",
        }
    }
}

/// The outcome of applying damage or healing.
///
/// Both numbers matter and neither can be recovered from the other afterwards: the meter
/// must count what actually landed, and an overkill of 500 on a 10 health dummy says
/// something a clamped 10 does not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageResult {
    /// How much health actually changed.
    pub applied: f32,
    /// How much of the request exceeded the health that was there.
    pub overkill: f32,
    /// Whether this is what took the entity to zero.
    pub killed: bool,
}

impl DamageResult {
    /// Nothing happened.
    pub const NONE: Self = Self {
        applied: 0.0,
        overkill: 0.0,
        killed: false,
    };
}

/// Current and maximum health.
///
/// Invariants, all enforced here rather than at the call sites: the current value is never
/// negative, never above the maximum, and never `NaN`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    current: f32,
    max: f32,
}

impl Health {
    /// Full health with the given maximum.
    ///
    /// A non-positive or non-finite maximum is clamped to a minimum of one point: an entity
    /// with zero maximum health is dead on spawn and every fraction computed from it is a
    /// division by zero.
    #[must_use]
    pub fn new(max: f32) -> Self {
        let max = if max.is_finite() && max > 0.0 {
            max
        } else {
            1.0
        };
        Self { current: max, max }
    }

    /// Health remaining.
    #[must_use]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Health at full.
    #[must_use]
    pub fn max(&self) -> f32 {
        self.max
    }

    /// Remaining health as a fraction of the maximum, in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        self.current / self.max
    }

    /// Whether there is any health left.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.current > 0.0
    }

    /// Applies damage and reports what landed.
    ///
    /// A negative, zero or non-finite amount does nothing rather than healing: a damage
    /// number that came out wrong upstream must not silently become a heal, which is the
    /// classic way a buff turns into an immortality bug.
    pub fn damage(&mut self, amount: f32) -> DamageResult {
        if !amount.is_finite() || amount <= 0.0 || !self.is_alive() {
            return DamageResult::NONE;
        }
        let applied = amount.min(self.current);
        self.current -= applied;
        DamageResult {
            applied,
            overkill: amount - applied,
            killed: self.current <= 0.0,
        }
    }

    /// Restores health up to the maximum and reports what landed.
    ///
    /// The dead are not healed: resurrection is a separate decision, not a side effect of
    /// an area heal landing on a corpse.
    pub fn heal(&mut self, amount: f32) -> DamageResult {
        if !amount.is_finite() || amount <= 0.0 || !self.is_alive() {
            return DamageResult::NONE;
        }
        let applied = amount.min(self.max - self.current);
        self.current += applied;
        DamageResult {
            applied,
            overkill: amount - applied,
            killed: false,
        }
    }

    /// Restores full health.
    pub fn restore(&mut self) {
        self.current = self.max;
    }

    /// Changes the maximum, keeping the same fraction of it.
    ///
    /// What a change of Stamina does. The *fraction* is preserved rather than the absolute
    /// current health, so putting a Stamina trinket on is not a free heal and taking one off
    /// is not a way to die: a character at half health is at half health either side of the
    /// change. A non-positive or non-finite maximum is refused outright, for the reason
    /// [`Health::new`] clamps one.
    pub fn set_max(&mut self, max: f32) {
        if !max.is_finite() || max <= 0.0 {
            return;
        }
        let fraction = self.fraction();
        self.max = max;
        self.current = (max * fraction).clamp(0.0, max);
    }

    /// Changes the maximum, keeping the **absolute** current health.
    ///
    /// What a temporary buff does, and the opposite of [`Health::set_max`], which is what a
    /// change of *gear* does. The distinction is load-bearing for the Warrior: Deflection
    /// moves the maximum by a fraction of a per cent every time an attack is blocked, and a
    /// fraction-preserving change would turn that into a stream of free healing on the way
    /// down and free damage on the way up. A buff that grants maximum health does not heal;
    /// one that expires does not kill, because the current value is clamped rather than
    /// scaled.
    pub fn set_max_keeping_current(&mut self, max: f32) {
        if !max.is_finite() || max <= 0.0 {
            return;
        }
        self.max = max;
        self.current = self.current.clamp(0.0, max);
    }

    /// Sets the current health directly, clamped into range.
    ///
    /// For arena setup and tests. Gameplay goes through [`Health::damage`] and
    /// [`Health::heal`] so every change produces an event.
    pub fn set_current(&mut self, value: f32) {
        self.current = if value.is_finite() {
            value.clamp(0.0, self.max)
        } else {
            0.0
        };
    }
}

/// A consumable resource abilities are paid for with.
///
/// One pool, regenerating at a constant rate. Not a mana/rage/energy hierarchy: the purpose
/// is to prove that an ability can be refused for a reason other than its cooldown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resource {
    current: f32,
    max: f32,
    regeneration_per_second: f32,
}

impl Resource {
    /// A full pool.
    #[must_use]
    pub fn new(max: f32, regeneration_per_second: f32) -> Self {
        let max = if max.is_finite() && max > 0.0 {
            max
        } else {
            1.0
        };
        Self {
            current: max,
            max,
            regeneration_per_second: regeneration_per_second.max(0.0),
        }
    }

    /// How much is available.
    #[must_use]
    pub fn current(&self) -> f32 {
        self.current
    }

    /// The pool size.
    #[must_use]
    pub fn max(&self) -> f32 {
        self.max
    }

    /// Available as a fraction of the pool, in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        self.current / self.max
    }

    /// Sets the current amount (clamped to [0, max]): an authoritative correction.
    pub fn set_current(&mut self, value: f32) {
        self.current = if value.is_finite() { value.clamp(0.0, self.max) } else { 0.0 };
    }

    /// Whether `cost` can be paid right now.
    #[must_use]
    pub fn can_pay(&self, cost: f32) -> bool {
        cost <= 0.0 || self.current >= cost
    }

    /// Pays `cost` if it is affordable.
    pub fn pay(&mut self, cost: f32) -> bool {
        if !self.can_pay(cost) {
            return false;
        }
        self.current = (self.current - cost.max(0.0)).max(0.0);
        true
    }

    /// Puts `amount` back, and reports how much actually went in.
    ///
    /// What a restoration *event* uses — Wrath's one per cent of maximum Mana — as opposed
    /// to the clock [`Resource::regenerate`] runs. What is over the pool is lost rather than
    /// banked, exactly as an overheal is.
    pub fn gain(&mut self, amount: f32) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        let before = self.current;
        self.current = (self.current + amount).min(self.max);
        self.current - before
    }

    /// Regenerates for one simulation step.
    pub fn regenerate(&mut self, seconds: f32) {
        self.current = (self.current + self.regeneration_per_second * seconds).min(self.max);
    }

    /// Refills the pool.
    pub fn restore(&mut self) {
        self.current = self.max;
    }
}

/// Everything the combat systems need about one entity, gathered for a reader.
///
/// A `Copy` snapshot rather than a borrow: the callers are a HUD and a telemetry pass that
/// want one entity's numbers, not a lock on the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatSnapshot {
    /// Who this describes.
    pub entity: EntityId,
    /// Whose side it is on.
    pub faction: Faction,
    /// Its health.
    pub health: Health,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pool_starts_full() {
        let health = Health::new(100.0);
        assert_eq!(health.current(), 100.0);
        assert_eq!(health.max(), 100.0);
        assert_eq!(health.fraction(), 1.0);
        assert!(health.is_alive());
    }

    #[test]
    fn damage_reduces_health_by_exactly_what_it_reports() {
        let mut health = Health::new(100.0);
        let result = health.damage(30.0);
        assert_eq!(result.applied, 30.0);
        assert_eq!(result.overkill, 0.0);
        assert!(!result.killed);
        assert_eq!(health.current(), 70.0);
    }

    #[test]
    fn health_reaching_zero_is_death_and_is_reported_once() {
        let mut health = Health::new(50.0);
        let killing = health.damage(50.0);
        assert!(killing.killed);
        assert!(!health.is_alive());
        assert_eq!(health.current(), 0.0);

        // A corpse takes no further damage, and the second hit must not report a kill —
        // otherwise a death counter counts every hit on a body.
        let after = health.damage(10.0);
        assert_eq!(after, DamageResult::NONE);
        assert!(!after.killed);
    }

    #[test]
    fn overkill_is_reported_rather_than_clamped_away() {
        let mut health = Health::new(10.0);
        let result = health.damage(500.0);
        assert_eq!(result.applied, 10.0, "only ten points were there to lose");
        assert_eq!(result.overkill, 490.0);
        assert!(result.killed);
        assert_eq!(health.current(), 0.0, "health never goes negative");
    }

    #[test]
    fn invalid_damage_does_nothing_instead_of_healing() {
        // A negative damage number that healed is how a damage buff becomes immortality.
        let mut health = Health::new(100.0);
        health.damage(40.0);
        for amount in [-10.0, 0.0, f32::NAN, f32::NEG_INFINITY] {
            assert_eq!(health.damage(amount), DamageResult::NONE, "{amount}");
        }
        assert_eq!(health.current(), 60.0);
    }

    #[test]
    fn healing_stops_at_the_maximum_and_never_revives() {
        let mut health = Health::new(100.0);
        health.damage(30.0);
        let result = health.heal(80.0);
        assert_eq!(result.applied, 30.0);
        assert_eq!(result.overkill, 50.0);
        assert_eq!(health.current(), 100.0);

        health.damage(100.0);
        assert_eq!(health.heal(50.0), DamageResult::NONE, "the dead stay dead");
        assert!(!health.is_alive());
    }

    #[test]
    fn a_degenerate_maximum_is_rejected_rather_than_dividing_by_zero() {
        for max in [0.0, -5.0, f32::NAN] {
            let health = Health::new(max);
            assert!(health.max() > 0.0, "{max}");
            assert!(health.fraction().is_finite());
            assert!(health.is_alive());
        }
    }

    #[test]
    fn restoring_brings_a_dummy_back_to_full() {
        let mut health = Health::new(200.0);
        health.damage(1000.0);
        health.restore();
        assert_eq!(health.current(), 200.0);
        assert!(health.is_alive());
    }

    #[test]
    fn set_current_clamps_into_range() {
        let mut health = Health::new(100.0);
        health.set_current(250.0);
        assert_eq!(health.current(), 100.0);
        health.set_current(-5.0);
        assert_eq!(health.current(), 0.0);
        health.set_current(f32::NAN);
        assert_eq!(health.current(), 0.0);
    }

    #[test]
    fn only_opposing_factions_are_hostile() {
        assert!(Faction::Player.is_hostile_to(Faction::Hostile));
        assert!(Faction::Hostile.is_hostile_to(Faction::Player));
        assert!(!Faction::Player.is_hostile_to(Faction::Player));
        assert!(!Faction::Player.is_hostile_to(Faction::Neutral));
        assert!(!Faction::Neutral.is_hostile_to(Faction::Hostile));
    }

    #[test]
    fn a_resource_is_spent_refused_and_regenerated() {
        let mut resource = Resource::new(100.0, 10.0);
        assert!(resource.pay(30.0));
        assert_eq!(resource.current(), 70.0);

        assert!(!resource.can_pay(200.0));
        assert!(!resource.pay(200.0), "an unaffordable cost changes nothing");
        assert_eq!(resource.current(), 70.0);

        resource.regenerate(1.0);
        assert_eq!(resource.current(), 80.0);
        resource.regenerate(100.0);
        assert_eq!(resource.current(), 100.0, "regeneration stops at the pool");
    }
}
