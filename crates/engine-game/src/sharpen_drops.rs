//! Pre-sharpened drops: how a freshly rolled item gets its first sharpening levels.
//!
//! A drop's sharpening is its own draw, made after the item and its base quality are chosen and
//! independent of both ([`SharpeningDrops::roll`]). The odds are a *curve* of weights, one per
//! level (`weights[i]` is the weight of `+i`), strictly decreasing so every higher level is
//! rarer than the one below it. Which curve applies depends on where the item came from
//! ([`DropSource`]), optionally the dungeon and the key level ([`SharpeningDrops::resolve`]);
//! the key level also shifts mass upward ([`key_shifted`]) without ever breaking the
//! strictly-decreasing rule.
//!
//! Like the rest of loot this is plain data: `engine-realm` reads `loot.json`'s
//! `"sharpening_drops"` block, validates it with [`validate_weights`] and builds this.

use crate::character::SHARPENING_CEILING;
use crate::firemage::Rng;

/// Where a drop came from. Selects the sharpening curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropSource {
    /// An ordinary monster.
    Mob,
    /// An elite monster.
    Elite,
    /// A boss's chest.
    Boss,
    /// Any other chest.
    Chest,
    /// Special rewards (events, future content).
    Special,
}

impl DropSource {
    /// Every source.
    pub const ALL: [Self; 5] = [
        Self::Mob,
        Self::Elite,
        Self::Boss,
        Self::Chest,
        Self::Special,
    ];

    /// The key `loot.json` uses.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Mob => "mob",
            Self::Elite => "elite",
            Self::Boss => "boss",
            Self::Chest => "chest",
            Self::Special => "special",
        }
    }

    /// The source named by `key`.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.key() == key)
    }
}

/// The ratio a key-shifted curve is capped at between neighbours, so it stays strictly
/// decreasing whatever the key level.
const MAX_SHIFTED_RATIO: f64 = 0.97;

/// One curve override: applies to `source`, optionally one `dungeon`, from `min_key_level` up.
#[derive(Debug, Clone, PartialEq)]
pub struct SharpenCurve {
    /// The drop source it applies to.
    pub source: DropSource,
    /// The dungeon id it is limited to, if any.
    pub dungeon: Option<String>,
    /// The lowest key level it applies at (0: any).
    pub min_key_level: u32,
    /// Weight per sharpening level, strictly decreasing.
    pub weights: Vec<f32>,
}

/// Every sharpening-drop number the loot rolls need.
#[derive(Debug, Clone, PartialEq)]
pub struct SharpeningDrops {
    /// A copy sharpened to at least this is marked in the interface.
    pub highlight_from: u8,
    /// The curve when nothing more specific matches.
    pub default: Vec<f32>,
    /// The overrides.
    pub curves: Vec<SharpenCurve>,
    /// Per key level above the first, how much the upward shift grows.
    pub key_shift_per_level: f32,
    /// The most any one level's weight is multiplied by.
    pub key_shift_max: f32,
}

/// Checks a weight curve: non-empty, at most `max_len` long, every weight finite and positive,
/// each strictly less than the one before.
///
/// # Errors
///
/// What is wrong with it, in words.
pub fn validate_weights(weights: &[f32], max_len: usize) -> Result<(), String> {
    if weights.is_empty() {
        return Err("a curve has no weights".to_owned());
    }
    if weights.len() > max_len {
        return Err(format!(
            "a curve has {} weights, more than the {max_len} levels allowed",
            weights.len()
        ));
    }
    if let Some(bad) = weights.iter().find(|w| !(w.is_finite() && **w > 0.0)) {
        return Err(format!("a curve has the non-positive weight {bad}"));
    }
    if let Some(pair) = weights.windows(2).find(|pair| pair[1] >= pair[0]) {
        return Err(format!(
            "a curve is not strictly decreasing ({} then {})",
            pair[0], pair[1]
        ));
    }
    Ok(())
}

/// `weights` shifted upward for `key_level`: weight `i` is multiplied by
/// `min((1 + per_level x (key_level - 1))^i, max)`, then capped so that each weight stays
/// strictly below the one before. Key level 1 (or 0) is the curve unchanged.
#[must_use]
pub fn key_shifted(weights: &[f32], per_level: f32, max: f32, key_level: u32) -> Vec<f32> {
    let steps = f64::from(key_level.saturating_sub(1));
    let base = 1.0 + f64::from(per_level.max(0.0)) * steps;
    let cap = f64::from(max.max(1.0));
    let mut shifted: Vec<f64> = Vec::with_capacity(weights.len());
    for (level, weight) in weights.iter().enumerate() {
        let factor = base.powi(i32::try_from(level).unwrap_or(i32::MAX)).min(cap);
        let mut value = f64::from(*weight) * factor;
        // Only a shifted curve is capped: the unshifted one is the file's, validated at load.
        if factor > 1.0
            && let Some(previous) = shifted.last()
        {
            value = value.min(previous * MAX_SHIFTED_RATIO);
        }
        shifted.push(value);
    }
    shifted.into_iter().map(|w| w as f32).collect()
}

impl SharpeningDrops {
    /// No sharpening ever: every drop is `+0`.
    #[must_use]
    pub fn none() -> Self {
        Self {
            highlight_from: crate::character::SHARPEN_HIGHLIGHT_DEFAULT,
            default: vec![1.0],
            curves: Vec::new(),
            key_shift_per_level: 0.0,
            key_shift_max: 1.0,
        }
    }

    /// The curve for a drop: among the overrides matching `source`, `dungeon` (or no dungeon
    /// named) and `min_key_level <= key_level`, the one naming a dungeon beats one that does not
    /// and then the highest `min_key_level` wins (file order breaks a tie); else the default.
    #[must_use]
    pub fn resolve(&self, source: DropSource, dungeon: &str, key_level: u32) -> &[f32] {
        self.curves
            .iter()
            .filter(|curve| {
                curve.source == source
                    && curve.min_key_level <= key_level
                    && curve.dungeon.as_deref().is_none_or(|id| id == dungeon)
            })
            .fold(None::<&SharpenCurve>, |best, curve| match best {
                Some(held) if rank(held) >= rank(curve) => Some(held),
                _ => Some(curve),
            })
            .map_or(self.default.as_slice(), |curve| curve.weights.as_slice())
    }

    /// The weights a drop at `key_level` is rolled with: the resolved curve, key-shifted.
    #[must_use]
    pub fn weights_at(&self, source: DropSource, dungeon: &str, key_level: u32) -> Vec<f32> {
        key_shifted(
            self.resolve(source, dungeon, key_level),
            self.key_shift_per_level,
            self.key_shift_max,
            key_level,
        )
    }

    /// **Draws a sharpening level** for a drop. Exactly one number from `rng`, whatever the
    /// outcome, and no knowledge of which item it is for.
    pub fn roll(&self, source: DropSource, dungeon: &str, key_level: u32, rng: &mut Rng) -> u8 {
        let roll = rng.next_f32();
        let weights = self.weights_at(source, dungeon, key_level);
        let total: f32 = weights.iter().sum();
        let mut target = roll * total;
        let mut chosen = weights.len().saturating_sub(1);
        for (level, weight) in weights.iter().enumerate() {
            if target < *weight {
                chosen = level;
                break;
            }
            target -= weight;
        }
        u8::try_from(chosen)
            .unwrap_or(SHARPENING_CEILING)
            .min(SHARPENING_CEILING)
    }
}

/// How specific a curve is: naming a dungeon first, then the key level it starts at.
fn rank(curve: &SharpenCurve) -> (bool, u32) {
    (curve.dungeon.is_some(), curve.min_key_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drops() -> SharpeningDrops {
        SharpeningDrops {
            highlight_from: 5,
            default: vec![100.0, 10.0, 1.0],
            curves: vec![
                SharpenCurve {
                    source: DropSource::Boss,
                    dungeon: None,
                    min_key_level: 0,
                    weights: vec![50.0, 20.0, 5.0],
                },
                SharpenCurve {
                    source: DropSource::Boss,
                    dungeon: None,
                    min_key_level: 10,
                    weights: vec![40.0, 20.0, 8.0],
                },
                SharpenCurve {
                    source: DropSource::Boss,
                    dungeon: Some("crypt".into()),
                    min_key_level: 0,
                    weights: vec![30.0, 20.0, 9.0],
                },
            ],
            key_shift_per_level: 0.05,
            key_shift_max: 6.0,
        }
    }

    #[test]
    fn curves_are_validated() {
        assert!(validate_weights(&[5.0, 2.0, 1.0], 11).is_ok());
        assert!(validate_weights(&[], 11).is_err(), "empty");
        assert!(validate_weights(&[1.0, 1.0], 11).is_err(), "flat");
        assert!(validate_weights(&[1.0, 2.0], 11).is_err(), "rising");
        assert!(validate_weights(&[3.0, 2.0, 1.0], 2).is_err(), "too long");
        assert!(validate_weights(&[3.0, -1.0], 11).is_err(), "negative");
        assert!(validate_weights(&[3.0, 0.0], 11).is_err(), "zero");
        assert!(validate_weights(&[3.0, f32::NAN], 11).is_err(), "nan");
    }

    #[test]
    fn the_most_specific_curve_wins() {
        let drops = drops();
        // No override for mobs: the default.
        assert_eq!(
            drops.resolve(DropSource::Mob, "crypt", 5),
            &[100.0, 10.0, 1.0]
        );
        // Source-only boss curve below the key threshold; the threshold curve at it.
        assert_eq!(drops.resolve(DropSource::Boss, "other", 9)[0], 50.0);
        assert_eq!(drops.resolve(DropSource::Boss, "other", 10)[0], 40.0);
        // A named dungeon beats a key-level threshold.
        assert_eq!(drops.resolve(DropSource::Boss, "crypt", 15)[0], 30.0);
    }

    #[test]
    fn a_higher_key_level_shifts_mass_upward() {
        let base = [100.0, 30.0, 9.0, 2.0];
        assert_eq!(key_shifted(&base, 0.05, 6.0, 1), base.to_vec());
        let shifted = key_shifted(&base, 0.05, 6.0, 20);
        assert_eq!(shifted[0], 100.0);
        assert!(shifted[3] / shifted[0] > base[3] / base[0]);
    }

    #[test]
    fn key_shift_stays_strictly_decreasing_for_every_key() {
        let curves: [&[f32]; 3] = [
            &[100.0, 30.0, 9.0, 2.0],
            &[1000.0, 450.0, 210.0, 100.0, 48.0, 22.0, 10.0, 4.5, 1.8, 0.5],
            &[10.0, 9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 0.5],
        ];
        for weights in curves {
            for (per_level, max) in [(0.03, 8.0), (0.2, 50.0), (1.0, 1000.0)] {
                for key in 1..=99 {
                    let shifted = key_shifted(weights, per_level, max, key);
                    assert_eq!(
                        validate_weights(&shifted, weights.len()),
                        Ok(()),
                        "key {key}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_roll_is_deterministic_and_draws_one_number() {
        let drops = drops();
        let roll = |seed| {
            let mut rng = Rng::new(seed);
            (0..64)
                .map(|_| drops.roll(DropSource::Mob, "", 1, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(roll(7), roll(7));
        assert_ne!(roll(7), roll(8));
        assert!(roll(7).iter().all(|level| *level <= 2));
        let (mut a, mut b) = (Rng::new(3), Rng::new(3));
        drops.roll(DropSource::Boss, "crypt", 40, &mut a);
        b.next_f32();
        assert_eq!(a.next_f32(), b.next_f32());
    }

    #[test]
    fn rolling_follows_the_weights() {
        let drops = drops();
        let mut rng = Rng::new(11);
        let mut counts = [0u32; 3];
        for _ in 0..20_000 {
            counts[usize::from(drops.roll(DropSource::Mob, "", 1, &mut rng))] += 1;
        }
        assert!(counts[0] > counts[1] && counts[1] > counts[2] && counts[2] > 0);
    }
}
