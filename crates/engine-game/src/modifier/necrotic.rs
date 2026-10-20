//! Necrotic Pools: what a dead enemy leaves on the ground.
//!
//! ```text
//!   monster dies ──▶ warning (telegraph) ──▶ pool (damages players standing in it) ──▶ gone
//! ```
//!
//! The geometry, the clock and the bounded list are [`super::plague::PlagueField`]'s: a pool is
//! the same disc on the ground as a plague cloud, so the one set of events
//! (`PlagueArmed`, `PlagueExploded`, `PlagueCloudEnded`) draws both and a client needs no new
//! code to show it. What differs is only the numbers, and that a pool infects nobody.

/// What one damage tick of a pool takes, as a percentage of maximum health: the per-second
/// rate over the shared tick interval, scaled by magnitude and capped.
#[must_use]
pub fn tick_pct(pct_per_sec: f32, interval: f32, magnitude: f32, cap_pct: f32) -> f32 {
    (pct_per_sec * interval * magnitude).clamp(0.0, cap_pct.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_is_the_rate_over_the_interval_scaled_and_capped() {
        assert!((tick_pct(4.0, 0.5, 1.0, 12.0) - 2.0).abs() < 1e-6);
        assert!((tick_pct(4.0, 0.5, 2.0, 12.0) - 4.0).abs() < 1e-6);
        assert_eq!(tick_pct(40.0, 0.5, 2.0, 12.0), 12.0);
        assert_eq!(tick_pct(4.0, 0.5, -1.0, 12.0), 0.0);
    }
}
