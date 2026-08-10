//! FireMage's two resources: Heat and Ultimate Energy.
//!
//! ```text
//! Heat     0 ------------------ 80 --- 100 -> Overheat: decays, +15% fire damage
//! Energy   0 --------------------------- 100 -> Cataclysm is ready
//! ```
//!
//! Both are data-driven: every rate and threshold below is a `pub const` in one place, and
//! nothing else in the game restates them.
//!
//! Heat is a *reward* for casting, not a cost: nothing spends it, it fills as the class
//! plays, and filling it is the good outcome. Overheat is therefore not a punishment either
//! - it is a damage bonus that runs while the bar drains back down.

/// Most Heat a FireMage can hold.
pub const HEAT_MAXIMUM: f32 = 100.0;

/// Heat at which Overheat begins.
pub const HEAT_OVERHEAT_THRESHOLD: f32 = 100.0;

/// Heat at which the bar starts reading as dangerous, for the UI and for talents.
pub const HEAT_HIGH_THRESHOLD: f32 = 80.0;

/// How fast Heat drains while Overheat is running, per second.
pub const HEAT_OVERHEAT_DECAY_PER_SECOND: f32 = 12.0;

/// Fire damage bonus while Overheat is running.
pub const OVERHEAT_DAMAGE_BONUS: f32 = 0.15;

/// Most Ultimate Energy a FireMage can hold, and the cost of Cataclysm.
pub const ENERGY_MAXIMUM: f32 = 100.0;

/// Passive Ultimate Energy per second: one every two seconds.
pub const ENERGY_PASSIVE_PER_SECOND: f32 = 0.5;

/// Heat, and whether it has boiled over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heat {
    value: f32,
    overheated: bool,
}

impl Default for Heat {
    fn default() -> Self {
        Self::new()
    }
}

impl Heat {
    /// Cold.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: 0.0,
            overheated: false,
        }
    }

    /// Current Heat, `0.0..=100.0`.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Heat as a fraction of the maximum, for a bar.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.value / HEAT_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether Overheat is running.
    #[must_use]
    pub fn is_overheated(&self) -> bool {
        self.overheated
    }

    /// Whether Heat is high enough for the bar to read as hot.
    #[must_use]
    pub fn is_high(&self) -> bool {
        self.value >= HEAT_HIGH_THRESHOLD
    }

    /// Adds Heat. Returns whether this crossed into Overheat.
    ///
    /// The return value is the trigger for everything that happens "on reaching 100 Heat" -
    /// Volatile Heat's empowered spell, Living Inferno's haste - so those talents fire once
    /// per boil-over rather than once per frame spent at the cap.
    pub fn gain(&mut self, amount: f32) -> bool {
        if amount <= 0.0 || !amount.is_finite() {
            return false;
        }
        self.value = (self.value + amount).min(HEAT_MAXIMUM);
        if !self.overheated && self.value >= HEAT_OVERHEAT_THRESHOLD {
            self.overheated = true;
            return true;
        }
        false
    }

    /// Advances the decay. Overheat ends when the bar empties.
    pub fn tick(&mut self, seconds: f32) {
        if !self.overheated {
            return;
        }
        self.value = (self.value - HEAT_OVERHEAT_DECAY_PER_SECOND * seconds).max(0.0);
        if self.value <= 0.0 {
            self.overheated = false;
        }
    }

    /// The fire damage multiplier Heat currently contributes.
    #[must_use]
    pub fn damage_multiplier(&self) -> f32 {
        if self.overheated {
            1.0 + OVERHEAT_DAMAGE_BONUS
        } else {
            1.0
        }
    }

    /// Back to cold. For a reset.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

/// Ultimate Energy, which fills passively and from casting.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct UltimateEnergy {
    value: f32,
}

impl UltimateEnergy {
    /// Empty.
    #[must_use]
    pub const fn new() -> Self {
        Self { value: 0.0 }
    }

    /// Current Energy, `0.0..=100.0`.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Energy as a fraction of the maximum, for a bar.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.value / ENERGY_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether Cataclysm can be cast.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.value >= ENERGY_MAXIMUM
    }

    /// Adds Energy, capped at the maximum.
    pub fn gain(&mut self, amount: f32) {
        if amount <= 0.0 || !amount.is_finite() {
            return;
        }
        self.value = (self.value + amount).min(ENERGY_MAXIMUM);
    }

    /// Adds the passive trickle for `seconds`.
    pub fn tick(&mut self, seconds: f32) {
        self.gain(ENERGY_PASSIVE_PER_SECOND * seconds);
    }

    /// Spends the whole bar. Returns whether there was enough to spend.
    pub fn spend_all(&mut self) -> bool {
        if !self.is_full() {
            return false;
        }
        self.value = 0.0;
        true
    }

    /// Back to empty. For a reset.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn heat_fills_and_stops_at_the_maximum() {
        let mut heat = Heat::new();
        assert_eq!(heat.value(), 0.0);
        assert!(!heat.is_overheated());

        assert!(!heat.gain(50.0), "fifty is not a boil-over");
        assert_eq!(heat.value(), 50.0);
        assert!((heat.fraction() - 0.5).abs() < 1e-6);

        assert!(heat.gain(60.0), "reaching a hundred boils over");
        assert_eq!(heat.value(), HEAT_MAXIMUM, "heat exceeded its maximum");
        assert!(heat.is_overheated());
    }

    #[test]
    fn overheat_is_reported_once_rather_than_every_gain() {
        // Volatile Heat and Living Inferno hang off this edge; reporting it twice would
        // grant their bonus twice for one boil-over.
        let mut heat = Heat::new();
        heat.gain(100.0);
        assert!(!heat.gain(10.0), "already overheated");
        assert!(!heat.gain(10.0));
    }

    #[test]
    fn overheat_decays_and_then_ends() {
        let mut heat = Heat::new();
        heat.gain(HEAT_MAXIMUM);
        assert!((heat.damage_multiplier() - (1.0 + OVERHEAT_DAMAGE_BONUS)).abs() < 1e-6);

        heat.tick(1.0);
        assert!(heat.value() < HEAT_MAXIMUM);
        assert!(heat.is_overheated(), "it ended after one second");

        heat.tick(100.0);
        assert_eq!(heat.value(), 0.0);
        assert!(!heat.is_overheated());
        assert!((heat.damage_multiplier() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn heat_does_not_decay_before_it_boils_over() {
        // Otherwise the bar would drain during a lull and the class would never reach the
        // cap while doing anything but its fastest rotation.
        let mut heat = Heat::new();
        heat.gain(60.0);
        heat.tick(5.0);
        assert_eq!(heat.value(), 60.0);
    }

    #[test]
    fn heat_reports_when_it_is_running_hot() {
        let mut heat = Heat::new();
        heat.gain(HEAT_HIGH_THRESHOLD - 1.0);
        assert!(!heat.is_high());
        heat.gain(1.0);
        assert!(heat.is_high());
    }

    #[test]
    fn a_meaningless_heat_gain_changes_nothing() {
        let mut heat = Heat::new();
        assert!(!heat.gain(f32::NAN));
        assert!(!heat.gain(-10.0));
        assert_eq!(heat.value(), 0.0);
    }

    #[test]
    fn energy_trickles_in_at_one_every_two_seconds() {
        let mut energy = UltimateEnergy::new();
        energy.tick(2.0);
        assert!((energy.value() - 1.0).abs() < 1e-5, "{}", energy.value());

        energy.tick(18.0);
        assert!((energy.value() - 10.0).abs() < 1e-4, "{}", energy.value());
    }

    #[test]
    fn energy_fills_to_a_hundred_and_stops() {
        let mut energy = UltimateEnergy::new();
        energy.gain(80.0);
        assert!(!energy.is_full());
        energy.gain(50.0);
        assert_eq!(energy.value(), ENERGY_MAXIMUM);
        assert!(energy.is_full());
        assert!((energy.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spending_needs_a_full_bar_and_empties_it() {
        let mut energy = UltimateEnergy::new();
        energy.gain(99.0);
        assert!(!energy.spend_all(), "cataclysm cast below a hundred");
        assert!((energy.value() - 99.0).abs() < 1e-6);

        energy.gain(1.0);
        assert!(energy.spend_all());
        assert_eq!(energy.value(), 0.0);
        assert!(!energy.is_full());
    }

    #[test]
    fn clearing_returns_both_resources_to_the_start_of_a_fight() {
        let mut heat = Heat::new();
        heat.gain(100.0);
        heat.clear();
        assert_eq!(heat.value(), 0.0);
        assert!(!heat.is_overheated());

        let mut energy = UltimateEnergy::new();
        energy.gain(100.0);
        energy.clear();
        assert_eq!(energy.value(), 0.0);
    }
}
