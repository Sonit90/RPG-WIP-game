//! The random source every scatter in an effect is drawn from.
//!
//! Its own, rather than the class's [`engine_game::firemage::Rng`](../../engine-game): this
//! crate cannot depend on gameplay, and it must not anyway. A visual roll that consumed the
//! *gameplay* random source would make the number of sparks a fire threw change what the
//! next critical strike roll returned — presentation would be deciding damage.
//!
//! xorshift64*, which is eight bytes of state, a few instructions per value and good enough
//! for scattering sparks. It is deterministic from its seed, so a test can assert on the
//! shape of a burst.

/// A small, fast, deterministic random source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(0x5F1E_2C3D_4B5A_6978)
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
        // xorshift with a zero state returns zero forever, which would make every scatter
        // spawn at the origin with no velocity.
        let mut rng = Rng::new(0);
        let values: Vec<u64> = (0..8).map(|_| rng.next_u64()).collect();
        assert!(values.iter().any(|value| *value != 0));
    }

    #[test]
    fn values_stay_inside_the_ranges_they_promise() {
        let mut rng = Rng::new(11);
        for _ in 0..4_096 {
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit), "{unit}");
            let signed = rng.signed();
            assert!((-1.0..1.0).contains(&signed), "{signed}");
            let ranged = rng.range(2.0, 5.0);
            assert!((2.0..5.0).contains(&ranged), "{ranged}");
        }
    }

    #[test]
    fn an_inverted_range_returns_its_low_end_rather_than_a_nan() {
        let mut rng = Rng::new(3);
        assert_eq!(rng.range(5.0, 5.0), 5.0);
        assert_eq!(rng.range(5.0, 1.0), 5.0);
    }

    #[test]
    fn the_distribution_is_not_obviously_lopsided() {
        // Not a statistical test — a smoke test that the top bits are being used. A source
        // that returned only the low half would make every burst one-sided.
        let mut rng = Rng::new(29);
        let mut buckets = [0_u32; 4];
        for _ in 0..8_000 {
            let index = (rng.unit() * 4.0) as usize;
            buckets[index.min(3)] += 1;
        }
        for count in buckets {
            assert!((1_400..2_600).contains(&count), "{buckets:?}");
        }
    }
}
