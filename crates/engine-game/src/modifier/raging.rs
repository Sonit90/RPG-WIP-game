//! Raging: the enrage a wounded non-boss enemy falls into.
//!
//! ```text
//!   health fraction < threshold  ──▶  damage × (1 + damage_pct · magnitude / 100)
//! ```
//!
//! A pure function of the monster's health: it is not a status, so nothing can dispel it, and
//! it ends the moment the monster is healed back over the threshold (or reset).

/// What a monster at `health_fraction` multiplies its damage by.
///
/// `threshold` is a fraction (`0.3` is thirty per cent); `damage_pct` is the bonus in whole
/// per cent, already scaled by the modifier's magnitude. Never below one, and one for a
/// monster at or above the threshold or one with no health left to enrage.
#[must_use]
pub fn damage_factor(health_fraction: f32, threshold: f32, damage_pct: f32) -> f32 {
    if health_fraction > 0.0 && health_fraction < threshold {
        1.0 + damage_pct.max(0.0) / 100.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_wounded_monster_enrages() {
        assert_eq!(damage_factor(1.0, 0.3, 50.0), 1.0);
        assert_eq!(
            damage_factor(0.3, 0.3, 50.0),
            1.0,
            "the threshold itself is not below it"
        );
        assert!((damage_factor(0.29, 0.3, 50.0) - 1.5).abs() < 1e-6);
        assert_eq!(damage_factor(0.0, 0.3, 50.0), 1.0, "a corpse is not angry");
        assert_eq!(damage_factor(0.1, 0.3, -20.0), 1.0, "never a discount");
    }
}
