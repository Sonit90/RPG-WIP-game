//! The simulation's random source.
//!
//! FireMage is full of chances: Firebolt's second Ignite stack, Firestorm's 20%, a critical
//! strike. All of them go through this, and this is part of the simulation state — so a
//! replay of the same commands from the same seed produces the same fight, which is what the
//! determinism tests assert about everything else in the crate.
//!
//! xorshift64*, not the standard library: `std` has no seeded generator, and a fight that
//! could not be reproduced would make every damage number an anecdote.

/// A seeded, reproducible generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(0x05EE_D1CE_F1AE)
    }
}

impl Rng {
    /// A generator from a seed. Zero is replaced: xorshift is stuck at it.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0.0..1.0`.
    pub fn next_f32(&mut self) -> f32 {
        // Top 24 bits over 2^24: exactly representable, and never returns 1.0.
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }

    /// Whether an event with probability `chance` happens.
    ///
    /// A chance of zero never fires and a chance of one always does, without consuming
    /// differently-shaped randomness for the two cases — a roll is a roll.
    pub fn chance(&mut self, chance: f32) -> bool {
        let roll = self.next_f32();
        chance > 0.0 && roll < chance
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
    fn the_same_seed_produces_the_same_sequence() {
        let mut a = Rng::new(1234);
        let mut b = Rng::new(1234);
        for _ in 0..1_000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn floats_stay_inside_the_unit_interval() {
        let mut rng = Rng::new(99);
        for _ in 0..10_000 {
            let value = rng.next_f32();
            assert!((0.0..1.0).contains(&value), "{value}");
        }
    }

    #[test]
    fn a_certainty_always_fires_and_an_impossibility_never_does() {
        let mut rng = Rng::new(7);
        for _ in 0..1_000 {
            assert!(rng.chance(1.0));
            assert!(!rng.chance(0.0));
        }
    }

    #[test]
    fn a_half_chance_lands_near_half() {
        let mut rng = Rng::new(42);
        let hits = (0..10_000).filter(|_| rng.chance(0.5)).count();
        assert!((4_500..5_500).contains(&hits), "{hits} of 10000");
    }

    #[test]
    fn a_zero_seed_still_generates() {
        let mut rng = Rng::new(0);
        assert_ne!(rng.next_u64(), 0);
    }
}
