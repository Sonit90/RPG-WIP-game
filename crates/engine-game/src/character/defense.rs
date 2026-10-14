//! What happens to an attack between being aimed at a character and reaching their health.
//!
//! ```text
//! incoming damage
//!       |
//!       v
//!   DefenseProfile::roll   dodge -> parry -> block -> hit      (physical only)
//!       |
//!       v
//!   DefenseProfile::mitigate   avoided entirely, blocked for less, armour off the rest
//!       |
//!       v
//!   health
//! ```
//!
//! # Why this is one type and not four checks in the damage pipeline
//!
//! Dodge, Parry, Block and Armour are four stats that answer one question — *how much of
//! this attack lands* — and the answer has an order. Rolling them at the call site would be
//! four chances for a caller to roll two of them for the same swing, and the Warrior's whole
//! payouts (Battle Trance's heal, Spell Reflect's cooldown) are made **once per successful
//! defensive event**: a duplicate roll is a duplicate payment. So the roll happens exactly once, here, and the caller is
//! handed a [`DefenseOutcome`] it can neither reproduce nor re-ask for.
//!
//! # Only one outcome per attack
//!
//! The three avoidance rolls are a single chained roll against one random number, in the
//! order dodge, parry, block. An attack is dodged *or* parried *or* blocked *or* it hits;
//! there is no such thing as a parried block. That is what lets
//! Battle Trance and Spell Reflect be fed straight from the outcome without a de-duplication
//! table.
//!
//! # What is rollable
//!
//! Physical damage only. A dodge is a body moving out of the way of something swung, and a
//! shield is between the character and a weapon; neither is an answer to a bolt of fire.
//! Magic is answered by Spell Reflect and by the flat reductions, which is the Warrior's
//! design and is why the class wants both kinds of mitigation rather than one.

use super::stat::{Stat, StatBlock, armor_reduction};
use crate::combat::DamageType;
use crate::firemage::rng::Rng;

/// How much of a blocked attack is stopped, as a fraction.
///
/// Thirty per cent: a block is worth much less than a dodge, which is what makes Block a
/// *frequency* stat rather than a bigger avoidance stat. The Warrior wants many small
/// defensive events, because each one is a Battle Trance heal and a Spell Reflect cooldown tick.
pub const BLOCK_REDUCTION: f32 = 0.30;

/// The most of an attack that avoidance may ever answer for, as a fraction.
///
/// Dodge, Parry and Block are summed and clamped to this together rather than capped one at
/// a time: three stats at forty per cent each would otherwise be an attack that can never
/// land, and a tank nothing can hit is a tank nothing has to heal.
pub const AVOIDANCE_CAP: f32 = 0.80;

/// What happened to one incoming attack.
///
/// Exactly one of these per attack, produced by exactly one roll. Everything downstream —
/// the damage that lands, the Battle Trance heal, the
/// Spell Reflect cooldown, the sound and the floating text — reads this one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefenseOutcome {
    /// It landed in full.
    Hit,
    /// It was blocked: it lands for [`BLOCK_REDUCTION`] less.
    Blocked,
    /// It was parried: none of it lands.
    Parried,
    /// It was dodged: none of it lands.
    Dodged,
}

impl DefenseOutcome {
    /// Whether this is a successful defensive event.
    ///
    /// **The one definition of a defensive event.** Block, Parry and Dodge; not a
    /// hit, and not the mere possession of the stats that could have produced one.
    #[must_use]
    pub fn is_defensive(self) -> bool {
        !matches!(self, Self::Hit)
    }

    /// Whether the attack was avoided outright.
    #[must_use]
    pub fn is_avoided(self) -> bool {
        matches!(self, Self::Parried | Self::Dodged)
    }

    /// Short uppercase name, for floating text and the developer readout.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "HIT",
            Self::Blocked => "BLOCK",
            Self::Parried => "PARRY",
            Self::Dodged => "DODGE",
        }
    }
}

/// Everything about a character that decides what an incoming attack does to them.
///
/// Derived from a [`StatBlock`] on every use and never stored, for the reason the whole
/// character module gives: a stored profile is a snapshot, and the Warrior's armour changes
/// on the tick War Stomp's armour bonus appears.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefenseProfile {
    /// Flat armour, after every percentage bonus.
    pub armor: f32,
    /// Chance to block, as a fraction.
    pub block: f32,
    /// Chance to parry, as a fraction.
    pub parry: f32,
    /// Chance to dodge, as a fraction.
    pub dodge: f32,
    /// How much a block stops, as a fraction. [`BLOCK_REDUCTION`].
    pub block_reduction: f32,
    /// Whether critical strikes against this character are turned into ordinary hits.
    ///
    /// Shield Block's promise. A property of the defender rather than a roll the attacker
    /// skips, so a future second source of it is one more thing setting this flag.
    pub immune_to_critical: bool,
    /// A flat fraction taken off everything that lands, whatever it is made of.
    ///
    /// Last Stand's ten (or fifteen) per cent. Applied after armour and after a block, so
    /// it is worth the same against a spell as against a swing — which is the point of a
    /// cooldown pressed when something enormous is incoming.
    pub damage_reduction: f32,
}

impl DefenseProfile {
    /// A character with no defences at all. What every enemy in the game currently has.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            armor: 0.0,
            block: 0.0,
            parry: 0.0,
            dodge: 0.0,
            block_reduction: BLOCK_REDUCTION,
            immune_to_critical: false,
            damage_reduction: 0.0,
        }
    }

    /// The profile a character sheet produces.
    ///
    /// The ratings become percentages through the one exchange rate in
    /// [`super::stat`]; nothing here invents a second conversion.
    #[must_use]
    pub fn from_stats(stats: &StatBlock) -> Self {
        Self {
            armor: stats.get(Stat::Armor).max(0.0),
            block: stats.percent(Stat::Block).max(0.0),
            parry: stats.percent(Stat::Parry).max(0.0),
            dodge: stats.percent(Stat::Dodge).max(0.0),
            ..Self::none()
        }
    }

    /// The same, with every avoidance chance scaled and clamped into the cap.
    ///
    /// Returned as `(dodge, parry, block)` in roll order.
    #[must_use]
    pub fn avoidance(&self) -> (f32, f32, f32) {
        let dodge = self.dodge.max(0.0);
        let parry = self.parry.max(0.0);
        let block = self.block.max(0.0);
        let total = dodge + parry + block;
        if total <= AVOIDANCE_CAP || total <= 0.0 {
            return (dodge, parry, block);
        }
        let scale = AVOIDANCE_CAP / total;
        (dodge * scale, parry * scale, block * scale)
    }

    /// The total chance that an attack produces a defensive event at all.
    ///
    /// What the character sheet prints and how often the Warrior's defensive events fire.
    #[must_use]
    pub fn defensive_chance(&self) -> f32 {
        let (dodge, parry, block) = self.avoidance();
        dodge + parry + block
    }

    /// **Rolls one incoming attack.** One random number, one outcome.
    ///
    /// Only physical damage is rollable; everything else is [`DefenseOutcome::Hit`] without
    /// consuming a roll, so a fight against a caster and a fight against a brute do not
    /// diverge in the random stream by different amounts.
    pub fn roll(&self, rng: &mut Rng, damage_type: DamageType) -> DefenseOutcome {
        if !matches!(damage_type, DamageType::Physical) {
            return DefenseOutcome::Hit;
        }
        let (dodge, parry, block) = self.avoidance();
        let roll = rng.next_f32();
        if roll < dodge {
            return DefenseOutcome::Dodged;
        }
        if roll < dodge + parry {
            return DefenseOutcome::Parried;
        }
        if roll < dodge + parry + block {
            return DefenseOutcome::Blocked;
        }
        DefenseOutcome::Hit
    }

    /// What `amount` becomes after `outcome`, armour and the flat reduction.
    ///
    /// The order is fixed and load-bearing: avoidance, then the block's share, then armour
    /// (physical only), then the flat reduction. Reversing any two changes what every
    /// cooldown in the Warrior's kit is worth.
    #[must_use]
    pub fn mitigate(&self, amount: f32, outcome: DefenseOutcome, damage_type: DamageType) -> f32 {
        if !amount.is_finite() || amount <= 0.0 {
            return 0.0;
        }
        if outcome.is_avoided() {
            return 0.0;
        }
        let mut damage = amount;
        if matches!(outcome, DefenseOutcome::Blocked) {
            damage *= 1.0 - self.block_reduction.clamp(0.0, 1.0);
        }
        if matches!(damage_type, DamageType::Physical) {
            damage *= 1.0 - armor_reduction(self.armor);
        }
        damage *= 1.0 - self.damage_reduction.clamp(0.0, 1.0);
        damage.max(0.0)
    }
}

impl Default for DefenseProfile {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::super::stat::RATING_PER_PERCENT;
    use super::*;

    fn rating(percent: f32) -> f32 {
        RATING_PER_PERCENT * percent
    }

    #[test]
    fn a_bare_character_takes_everything() {
        let profile = DefenseProfile::none();
        let mut rng = Rng::new(1);
        for _ in 0..1_000 {
            let outcome = profile.roll(&mut rng, DamageType::Physical);
            assert_eq!(outcome, DefenseOutcome::Hit);
            assert_eq!(
                profile.mitigate(100.0, outcome, DamageType::Physical),
                100.0
            );
        }
    }

    #[test]
    fn ratings_become_the_percentages_the_sheet_prints() {
        let mut stats = StatBlock::zero();
        stats.set(Stat::Block, rating(10.0));
        stats.set(Stat::Parry, rating(5.0));
        stats.set(Stat::Dodge, rating(5.0));
        stats.set(Stat::Armor, 1_000.0);
        let profile = DefenseProfile::from_stats(&stats);
        assert!((profile.block - 0.10).abs() < 1e-6);
        assert!((profile.parry - 0.05).abs() < 1e-6);
        assert!((profile.dodge - 0.05).abs() < 1e-6);
        assert!((profile.defensive_chance() - 0.20).abs() < 1e-6);
        assert_eq!(profile.armor, 1_000.0);
    }

    #[test]
    fn only_one_outcome_can_happen_to_one_attack() {
        // The property the whole resource rests on: a swing is dodged or parried or blocked
        // or it hits, and the frequencies follow the stats.
        let mut stats = StatBlock::zero();
        stats.set(Stat::Dodge, rating(20.0));
        stats.set(Stat::Parry, rating(20.0));
        stats.set(Stat::Block, rating(20.0));
        let profile = DefenseProfile::from_stats(&stats);
        let mut rng = Rng::new(99);
        let mut counts = [0u32; 4];
        for _ in 0..20_000 {
            match profile.roll(&mut rng, DamageType::Physical) {
                DefenseOutcome::Dodged => counts[0] += 1,
                DefenseOutcome::Parried => counts[1] += 1,
                DefenseOutcome::Blocked => counts[2] += 1,
                DefenseOutcome::Hit => counts[3] += 1,
            }
        }
        assert_eq!(counts.iter().sum::<u32>(), 20_000);
        for (index, count) in counts.iter().take(3).enumerate() {
            let share = f64::from(*count) / 20_000.0;
            assert!((share - 0.20).abs() < 0.02, "{index}: {share}");
        }
    }

    #[test]
    fn avoidance_is_scaled_into_the_cap_rather_than_truncated() {
        let mut stats = StatBlock::zero();
        stats.set(Stat::Dodge, rating(50.0));
        stats.set(Stat::Parry, rating(50.0));
        stats.set(Stat::Block, rating(50.0));
        let profile = DefenseProfile::from_stats(&stats);
        let (dodge, parry, block) = profile.avoidance();
        assert!((dodge + parry + block - AVOIDANCE_CAP).abs() < 1e-6);
        // Scaled together: the three stats keep their proportions.
        assert!((dodge - parry).abs() < 1e-6);
        assert!((parry - block).abs() < 1e-6);
    }

    #[test]
    fn magic_is_never_dodged_parried_or_blocked() {
        let mut stats = StatBlock::zero();
        stats.set(Stat::Dodge, rating(40.0));
        stats.set(Stat::Parry, rating(20.0));
        stats.set(Stat::Block, rating(20.0));
        let profile = DefenseProfile::from_stats(&stats);
        let mut rng = Rng::new(7);
        for kind in [DamageType::Fire, DamageType::Arcane] {
            for _ in 0..500 {
                assert_eq!(profile.roll(&mut rng, kind), DefenseOutcome::Hit);
            }
        }
    }

    #[test]
    fn armour_answers_physical_damage_and_leaves_magic_alone() {
        let profile = DefenseProfile {
            armor: 1_000.0,
            ..DefenseProfile::none()
        };
        let physical = profile.mitigate(100.0, DefenseOutcome::Hit, DamageType::Physical);
        let fire = profile.mitigate(100.0, DefenseOutcome::Hit, DamageType::Fire);
        assert!((physical - 50.0).abs() < 1e-4, "{physical}");
        assert!((fire - 100.0).abs() < 1e-4, "{fire}");
    }

    #[test]
    fn a_block_stops_its_share_and_avoidance_stops_all_of_it() {
        let profile = DefenseProfile::none();
        let blocked = profile.mitigate(100.0, DefenseOutcome::Blocked, DamageType::Physical);
        assert!((blocked - 70.0).abs() < 1e-4, "{blocked}");
        for outcome in [DefenseOutcome::Dodged, DefenseOutcome::Parried] {
            assert_eq!(profile.mitigate(100.0, outcome, DamageType::Physical), 0.0);
        }
    }

    #[test]
    fn the_flat_reduction_applies_to_everything_that_lands() {
        let profile = DefenseProfile {
            damage_reduction: 0.10,
            ..DefenseProfile::none()
        };
        for kind in [DamageType::Physical, DamageType::Fire, DamageType::Arcane] {
            let landed = profile.mitigate(100.0, DefenseOutcome::Hit, kind);
            assert!((landed - 90.0).abs() < 1e-4, "{kind:?}: {landed}");
        }
    }

    #[test]
    fn only_block_parry_and_dodge_are_defensive_events() {
        assert!(!DefenseOutcome::Hit.is_defensive());
        for outcome in [
            DefenseOutcome::Blocked,
            DefenseOutcome::Parried,
            DefenseOutcome::Dodged,
        ] {
            assert!(outcome.is_defensive(), "{outcome:?}");
        }
    }
}
