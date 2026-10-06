//! FireMage's two resources: Heat and Ultimate Energy.
//!
//! ```text
//! Heat  0 ------ 30 ------------ 70 ------------ 100
//!       x1.00    x1.15           x1.30           consumed -> Inferno State, x1.50, 10 s
//!                                +5% haste/crit  +7.5% haste/crit
//! ```
//!
//! Heat is combat *momentum*, not a cost. Nothing spends it, no ability consumes it, and
//! filling it is the good outcome: the class becomes stronger the longer a fight runs. The
//! only thing that empties it is reaching a hundred, which trades the whole bar for ten
//! seconds of Inferno State.
//!
//! Out of combat it eventually decays, so a fight does not start on the back of the last
//! one. In combat it never does — a bar that drained during a lull would mean the class
//! only ever reached its thresholds while executing perfectly.

use super::Timer;

/// Most Heat a FireMage can hold.
pub const HEAT_MAXIMUM: f32 = 100.0;

/// Heat at which the first damage bonus begins.
pub const HEAT_WARM_THRESHOLD: f32 = 30.0;

/// Heat at which the second damage bonus and the stat bonus begin.
pub const HEAT_HOT_THRESHOLD: f32 = 70.0;

/// Fire damage bonus in the 30-69 band.
pub const HEAT_WARM_BONUS: f32 = 0.15;

/// Fire damage bonus in the 70-99 band.
pub const HEAT_HOT_BONUS: f32 = 0.30;

/// Haste or critical strike granted in the 70-99 band.
pub const HEAT_HOT_STAT: f32 = 0.05;

/// Fire damage bonus while the Inferno State runs.
pub const INFERNO_DAMAGE_BONUS: f32 = 0.50;

/// Haste or critical strike granted while the Inferno State runs.
pub const INFERNO_STAT: f32 = 0.075;

/// How long the Inferno State lasts, before talents.
pub const INFERNO_DURATION: f32 = 10.0;

/// Seconds out of combat before Heat starts to decay.
pub const HEAT_COMBAT_TIMEOUT: f32 = 10.0;

/// How fast Heat decays once it is allowed to, per second.
pub const HEAT_DECAY_PER_SECOND: f32 = 5.0;

/// Which band Heat is in. What the bar draws and what the tooltips name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatTier {
    /// 0-29: nothing yet.
    Cold,
    /// 30-69: +15 % fire damage.
    Warm,
    /// 70-99: +30 % fire damage and a stat bonus.
    Hot,
    /// The Inferno State: +50 % fire damage and a larger stat bonus.
    Inferno,
}

impl HeatTier {
    /// Uppercase name; the UI font has no lowercase glyphs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cold => "COLD",
            Self::Warm => "WARM",
            Self::Hot => "HOT",
            Self::Inferno => "INFERNO",
        }
    }
}

/// Heat, its bands, and the Inferno State reaching the top buys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heat {
    value: f32,
    inferno: Timer,
    /// Seconds since the mage was last in combat.
    idle: f32,
}

impl Default for Heat {
    fn default() -> Self {
        Self::new()
    }
}

impl Heat {
    /// Cold, out of combat, no state running.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: 0.0,
            inferno: Timer::idle(),
            idle: 0.0,
        }
    }

    /// Sets the heat value (clamped to [0, 100]): an authoritative correction. Inferno and
    /// the idle clock are left as they are.
    pub fn set(&mut self, value: f32) {
        self.value = if value.is_finite() {
            value.clamp(0.0, 100.0)
        } else {
            0.0
        };
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

    /// Which band Heat is in. The Inferno State outranks the value.
    #[must_use]
    pub fn tier(&self) -> HeatTier {
        if self.inferno.is_active() {
            return HeatTier::Inferno;
        }
        if self.value >= HEAT_HOT_THRESHOLD {
            HeatTier::Hot
        } else if self.value >= HEAT_WARM_THRESHOLD {
            HeatTier::Warm
        } else {
            HeatTier::Cold
        }
    }

    /// Whether the Inferno State is running.
    #[must_use]
    pub fn is_inferno(&self) -> bool {
        self.inferno.is_active()
    }

    /// Seconds of Inferno State left. Zero when it is not running.
    #[must_use]
    pub fn inferno_remaining(&self) -> f32 {
        self.inferno.remaining()
    }

    /// Whether the mage counts as in combat, which is what freezes the decay.
    #[must_use]
    pub fn in_combat(&self) -> bool {
        self.idle < HEAT_COMBAT_TIMEOUT
    }

    /// The fire damage multiplier the current band contributes.
    #[must_use]
    pub fn damage_multiplier(&self) -> f32 {
        match self.tier() {
            HeatTier::Cold => 1.0,
            HeatTier::Warm => 1.0 + HEAT_WARM_BONUS,
            HeatTier::Hot => 1.0 + HEAT_HOT_BONUS,
            HeatTier::Inferno => 1.0 + INFERNO_DAMAGE_BONUS,
        }
    }

    /// The haste or critical strike the current band grants.
    #[must_use]
    pub fn stat_bonus(&self) -> f32 {
        match self.tier() {
            HeatTier::Cold | HeatTier::Warm => 0.0,
            HeatTier::Hot => HEAT_HOT_STAT,
            HeatTier::Inferno => INFERNO_STAT,
        }
    }

    /// Adds Heat. Returns whether this started the Inferno State.
    ///
    /// Reaching a hundred spends the whole bar at once: Heat drops to zero and the state
    /// starts. Reaching a hundred *again* while the state is already running does not
    /// re-trigger it — the state is a reward for a cycle, not a stacking buff — and the
    /// Heat gathered during it is cleared when it ends.
    pub fn gain(&mut self, amount: f32, inferno_duration: f32) -> bool {
        if amount <= 0.0 || !amount.is_finite() {
            return false;
        }
        self.value = (self.value + amount).min(HEAT_MAXIMUM);
        if self.value >= HEAT_MAXIMUM && !self.inferno.is_active() {
            self.value = 0.0;
            self.inferno.start(inferno_duration.max(0.0));
            return true;
        }
        false
    }

    /// Notes that the mage is in combat, which holds the decay off.
    pub fn note_combat(&mut self) {
        self.idle = 0.0;
    }

    /// Advances the Inferno State and the out-of-combat decay.
    ///
    /// Returns whether the Inferno State ended on this call.
    pub fn tick(&mut self, seconds: f32) -> bool {
        let ended = self.inferno.tick(seconds);
        if ended {
            // The state was bought with the whole bar; whatever accumulated during it goes
            // with the state rather than carrying into the next cycle.
            self.value = 0.0;
        }
        self.idle += seconds;
        if self.idle >= HEAT_COMBAT_TIMEOUT && !self.inferno.is_active() {
            self.value = (self.value - HEAT_DECAY_PER_SECOND * seconds).max(0.0);
        }
        ended
    }

    /// Back to cold. For a reset.
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

/// Most Ultimate Energy a FireMage can hold, and the cost of the Ultimate.
pub const ENERGY_MAXIMUM: f32 = 100.0;

/// Passive Ultimate Energy per second: one every two seconds.
pub const ENERGY_PASSIVE_PER_SECOND: f32 = 0.5;

/// Ultimate Energy, which fills passively and from casting.
///
/// Kept exactly as it was while the Ultimate is disabled: the class has no Ultimate concept
/// yet, and deleting the resource would be a change to an architecture that is meant to
/// survive this rework untouched.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct UltimateEnergy {
    value: f32,
}

impl UltimateEnergy {
    /// Sets the stored energy (clamped): an authoritative correction.
    pub fn set(&mut self, value: f32) {
        self.value = if value.is_finite() {
            value.clamp(0.0, 100.0)
        } else {
            0.0
        };
    }

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

    /// Whether the bar is full.
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
    fn the_three_bands_are_where_the_design_puts_them() {
        let mut heat = Heat::new();
        assert_eq!(heat.tier(), HeatTier::Cold);
        assert!((heat.damage_multiplier() - 1.0).abs() < 1e-6);

        heat.gain(29.0, INFERNO_DURATION);
        assert_eq!(heat.tier(), HeatTier::Cold, "29 is still cold");

        heat.gain(1.0, INFERNO_DURATION);
        assert_eq!(heat.tier(), HeatTier::Warm);
        assert!((heat.damage_multiplier() - 1.15).abs() < 1e-6);
        assert_eq!(heat.stat_bonus(), 0.0, "the warm band grants no stat");

        heat.gain(40.0, INFERNO_DURATION);
        assert_eq!(heat.tier(), HeatTier::Hot);
        assert!((heat.damage_multiplier() - 1.30).abs() < 1e-6);
        assert!((heat.stat_bonus() - HEAT_HOT_STAT).abs() < 1e-6);
    }

    #[test]
    fn reaching_a_hundred_spends_the_bar_and_starts_the_inferno_state() {
        let mut heat = Heat::new();
        assert!(!heat.gain(99.0, INFERNO_DURATION));
        assert!(!heat.is_inferno());

        assert!(heat.gain(1.0, INFERNO_DURATION), "the state did not start");
        assert_eq!(heat.value(), 0.0, "heat was not consumed");
        assert!(heat.is_inferno());
        assert_eq!(heat.tier(), HeatTier::Inferno);
        assert!((heat.damage_multiplier() - 1.50).abs() < 1e-6);
        assert!((heat.stat_bonus() - INFERNO_STAT).abs() < 1e-6);
        assert!((heat.inferno_remaining() - INFERNO_DURATION).abs() < 1e-6);
    }

    #[test]
    fn heat_gathered_during_the_state_does_not_carry_past_it() {
        let mut heat = Heat::new();
        heat.gain(100.0, INFERNO_DURATION);
        heat.gain(80.0, INFERNO_DURATION);
        assert!(heat.is_inferno(), "it re-triggered instead of accumulating");

        assert!(heat.tick(INFERNO_DURATION + 0.1), "the state never ended");
        assert_eq!(heat.value(), 0.0);
        assert_eq!(heat.tier(), HeatTier::Cold);
    }

    #[test]
    fn a_hundred_reached_during_the_state_does_not_restart_it() {
        let mut heat = Heat::new();
        heat.gain(100.0, INFERNO_DURATION);
        heat.tick(5.0);
        heat.note_combat();
        assert!(!heat.gain(100.0, INFERNO_DURATION), "it re-triggered");
        assert!((heat.inferno_remaining() - 5.0).abs() < 0.01);
    }

    #[test]
    fn heat_does_not_decay_in_combat_and_does_out_of_it() {
        let mut heat = Heat::new();
        heat.gain(60.0, INFERNO_DURATION);

        for _ in 0..100 {
            heat.note_combat();
            heat.tick(0.1);
        }
        assert!((heat.value() - 60.0).abs() < 1e-4, "it decayed in combat");

        // Out of combat it holds for the timeout, then drains.
        heat.tick(HEAT_COMBAT_TIMEOUT - 1.0);
        assert!((heat.value() - 60.0).abs() < 1e-4, "it decayed too early");
        heat.tick(1.0);
        assert!(heat.value() < 60.0, "it never decayed");
        heat.tick(100.0);
        assert_eq!(heat.value(), 0.0);
    }

    #[test]
    fn energy_still_trickles_in_because_the_ultimate_system_is_untouched() {
        let mut energy = UltimateEnergy::new();
        energy.tick(2.0);
        assert!((energy.value() - 1.0).abs() < 1e-5);
        energy.gain(200.0);
        assert!(energy.is_full());
        assert!(energy.spend_all());
        assert_eq!(energy.value(), 0.0);
    }
}
