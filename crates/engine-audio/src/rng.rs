//! The random source every variation is drawn from.
//!
//! Its own, rather than the class's [`engine_game::firemage::Rng`](../../engine_game): this
//! crate cannot depend on gameplay, and it must not anyway. A roll that picked a sound
//! variation out of the *gameplay* random source would make which explosion the player heard
//! change what the next critical strike roll returned — presentation would be deciding
//! damage.
//!
//! xorshift64*, the same generator `engine-vfx` uses and for the same reasons: eight bytes
//! of state, a few instructions per value, deterministic from its seed so a test can assert
//! which variation a sequence of hits picked.

/// A small, fast, deterministic random source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(0x4A7C_15A3_9E37_79B9)
    }
}

impl Rng {
    /// A source seeded with `seed`. A zero seed is replaced: xorshift is stuck at zero.
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

    /// The next raw value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// The next value in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits, which is exactly the mantissa an f32 can hold: taking the low
        // bits of a xorshift is taking its worst ones.
        ((self.next_u64() >> 40) as f32) / (1_u32 << 24) as f32
    }

    /// The next value in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// The next value in `[low, high)`, or `low` if the range is empty or inverted.
    pub fn range(&mut self, low: f32, high: f32) -> f32 {
        if high <= low {
            return low;
        }
        low + self.unit() * (high - low)
    }

    /// The next index below `count`, or zero if `count` is zero.
    pub fn index(&mut self, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        (self.next_u64() % count as u64) as usize
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
    fn a_seed_produces_the_same_sequence_twice() {
        let mut first = Rng::new(7);
        let mut second = Rng::new(7);
        for _ in 0..64 {
            assert_eq!(first.next_u64(), second.next_u64());
        }
    }

    #[test]
    fn a_zero_seed_still_produces_values() {
        let mut rng = Rng::new(0);
        let values: Vec<u64> = (0..8).map(|_| rng.next_u64()).collect();
        assert!(values.iter().any(|value| *value != 0));
    }

    #[test]
    fn values_stay_inside_the_ranges_they_promise() {
        let mut rng = Rng::new(11);
        for _ in 0..2_048 {
            assert!((0.0..1.0).contains(&rng.unit()));
            assert!((-1.0..1.0).contains(&rng.signed()));
            assert!((0.9..1.1).contains(&rng.range(0.9, 1.1)));
            assert!(rng.index(3) < 3);
        }
    }

    #[test]
    fn an_empty_index_range_is_zero_rather_than_a_division_by_zero() {
        let mut rng = Rng::new(3);
        assert_eq!(rng.index(0), 0);
    }
}
