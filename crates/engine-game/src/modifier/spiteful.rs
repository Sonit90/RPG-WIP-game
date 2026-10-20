//! Spiteful Casters: faster interruptible casts, and a curse on whoever they land on.
//!
//! ```text
//!   caster starts an interruptible cast ──▶ cast time ÷ (1 + speed)
//!   the cast completes ──▶ its victims gain Spite (dispellable Magic, less healing taken)
//! ```
//!
//! The rewards are the point: an interrupt stops the cast and its curse, and a dispel takes
//! the curse off after it lands. A party that does neither pays in healing.

/// The most a cast may be sped up, as a fraction: twice as fast.
pub const MAX_SPEED_UP: f32 = 1.0;

/// The most healing the curse may take, as a fraction. Healers must still be able to heal.
pub const MAX_HEAL_REDUCTION: f32 = 0.9;

/// What a cast of `duration` seconds becomes when casters are `speed_pct` per cent faster
/// (after magnitude). Never shorter than half its length, and unchanged for a non-positive
/// speed or an instant.
#[must_use]
pub fn sped_up(duration: f32, speed_pct: f32) -> f32 {
    if !(duration > 0.0 && speed_pct > 0.0) {
        return duration;
    }
    let speed = (speed_pct / 100.0).min(MAX_SPEED_UP);
    duration / (1.0 + speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cast_shortens_by_the_speed_up_and_never_past_half() {
        assert!((sped_up(2.0, 25.0) - 1.6).abs() < 1e-6);
        assert_eq!(sped_up(2.0, 0.0), 2.0);
        assert!(
            (sped_up(2.0, 500.0) - 1.0).abs() < 1e-6,
            "capped at twice as fast"
        );
        assert_eq!(sped_up(0.0, 50.0), 0.0, "an instant stays instant");
    }
}
