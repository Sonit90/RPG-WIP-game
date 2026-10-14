//! Gear profiles: what a character is wearing when a fight is simulated.
//!
//! ```text
//! GearProfile --stat_block(hero)--> StatBlock --Simulation::set_stat_bonus--> the real pipeline
//! ```
//!
//! A profile is *not* a bag of percentages. It is a stat block in the game's own units -
//! ratings, with the main stat folded into the hero's own - handed to the simulation through
//! [`engine_game::sim::Simulation::set_stat_bonus`], which is the same door gear takes: the
//! rating curve ([`engine_game::character::stat::secondary_fraction`]), the main-stat to
//! spell-power bridge and every class's use of them are the game's, not this crate's.
//!
//! # Where the numbers come from
//!
//! The item catalog ([`engine_game::character::item`]) has thirteen equipment slots and, at
//! the current progression cap, one source of stats: the generated boss-loot pool, 110
//! Uncommon pieces of item level [`LOOT_ITEM_LEVEL`]. Every piece grants exactly two stats,
//! [`LOOT_STAT_RATING`] rating each, chosen from {Haste, Critical Strike, Main Stat, Spirit,
//! Versatility}. So a character's gear is fully described by *how many of the 26 stat slots
//! (13 pieces x 2 stats) go to which stat*, which is what [`StatSlots`] is, and by how many
//! pieces are worn at all.
//!
//! * **None** - nothing worn. Base stats only.
//! * **Medium** - [`MEDIUM_PIECES`] of the 13 slots filled (about half), at the catalog's own
//!   quality (x1.00). Four variants: balanced, primary-heavy, haste-lean and crit-lean, each
//!   spending the same 14 stat slots differently. That spread is the point: a class that is
//!   only balanced for one way of dressing is not balanced.
//! * **Top** - all 13 slots filled, every piece rolled for the best-available stats (the main
//!   stat on each piece, the second stat split between Haste and Critical Strike), at the
//!   [`TOP_QUALITY`] ceiling of x1.10. The game has no quality scaling yet, so the 10% is the
//!   headroom the design assumes a best-rolled piece of the cap's item level can reach; it is
//!   one constant to change when real quality tiers arrive.
//!
//! Versatility and Spirit are left at zero in every profile. Versatility is a flat damage
//! multiplier that would shift all three classes alike, and Spirit is a random proc that adds
//! noise without telling the designer anything about class balance.

use engine_game::character::item::{LOOT_ITEM_LEVEL, LOOT_STAT_RATING};
use engine_game::character::stat::{Stat, StatBlock};
use engine_game::hero::Hero;

/// How many of the thirteen equipment slots a medium profile fills.
pub const MEDIUM_PIECES: u8 = 7;

/// How many equipment slots a character has.
pub const FULL_PIECES: u8 = 13;

/// The quality ceiling top gear is rolled at, as a multiple of a catalog piece's ratings.
pub const TOP_QUALITY: f32 = 1.10;

/// Which rung of the progression a profile stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GearTier {
    /// Nothing equipped.
    None,
    /// About half the slots, at catalog quality.
    Medium,
    /// Every slot, at the quality ceiling.
    Top,
}

impl GearTier {
    /// Every tier, lowest first.
    pub const ALL: [Self; 3] = [Self::None, Self::Medium, Self::Top];

    /// The written name the command line takes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Medium => "medium",
            Self::Top => "top",
        }
    }

    /// Parses `none`, `medium` or `top`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.as_str() == text)
    }
}

/// How the stat slots of the worn pieces are spent.
///
/// Counts of *stat slots*, not pieces: a piece contributes two. The total must equal
/// `2 * pieces`, which a test holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatSlots {
    /// Slots spent on the main stat (Intellect or Strength, whichever the hero uses).
    pub main: u8,
    /// Slots spent on Haste.
    pub haste: u8,
    /// Slots spent on Critical Strike.
    pub crit: u8,
}

/// One way of dressing a character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearProfile {
    /// Which rung it stands for.
    pub tier: GearTier,
    /// The variant within the rung, for the report and the command line.
    pub variant: &'static str,
    /// How many of the thirteen slots are filled.
    pub pieces: u8,
    /// Multiplier on every rating the pieces grant.
    pub quality: f32,
    /// How the stat slots are spent.
    pub slots: StatSlots,
}

/// **Every gear profile the simulator knows, in one table.**
///
/// Stat-slot arithmetic, with one rating per slot of `2.0 * quality` (see the module header):
///
/// | profile         | pieces | quality | main | haste | crit | total slots |
/// |-----------------|--------|---------|------|-------|------|-------------|
/// | none            | 0      | -       | 0    | 0     | 0    | 0           |
/// | medium balanced | 7      | 1.00    | 6    | 4     | 4    | 14          |
/// | medium primary  | 7      | 1.00    | 10   | 2     | 2    | 14          |
/// | medium haste    | 7      | 1.00    | 4    | 8     | 2    | 14          |
/// | medium crit     | 7      | 1.00    | 4    | 2     | 8    | 14          |
/// | top             | 13     | 1.10    | 13   | 7     | 6    | 26          |
///
/// Which makes the rating totals (before the rating curve `x / (x + 100)`):
///
/// * medium balanced: +12 main, 8 Haste, 8 Crit.
/// * medium primary-heavy: +20 main, 4 Haste, 4 Crit.
/// * medium haste-lean: +8 main, 16 Haste, 4 Crit.
/// * medium crit-lean: +8 main, 4 Haste, 16 Crit.
/// * top: +28.6 main, 15.4 Haste, 13.2 Crit.
///
/// Adding a profile - a Versatility set, a half-quality set - is adding a row here.
pub const PROFILES: [GearProfile; 6] = [
    GearProfile {
        tier: GearTier::None,
        variant: "none",
        pieces: 0,
        quality: 1.0,
        slots: StatSlots {
            main: 0,
            haste: 0,
            crit: 0,
        },
    },
    GearProfile {
        tier: GearTier::Medium,
        variant: "balanced",
        pieces: MEDIUM_PIECES,
        quality: 1.0,
        slots: StatSlots {
            main: 6,
            haste: 4,
            crit: 4,
        },
    },
    GearProfile {
        tier: GearTier::Medium,
        variant: "primary-heavy",
        pieces: MEDIUM_PIECES,
        quality: 1.0,
        slots: StatSlots {
            main: 10,
            haste: 2,
            crit: 2,
        },
    },
    GearProfile {
        tier: GearTier::Medium,
        variant: "haste-lean",
        pieces: MEDIUM_PIECES,
        quality: 1.0,
        slots: StatSlots {
            main: 4,
            haste: 8,
            crit: 2,
        },
    },
    GearProfile {
        tier: GearTier::Medium,
        variant: "crit-lean",
        pieces: MEDIUM_PIECES,
        quality: 1.0,
        slots: StatSlots {
            main: 4,
            haste: 2,
            crit: 8,
        },
    },
    GearProfile {
        tier: GearTier::Top,
        variant: "top",
        pieces: FULL_PIECES,
        quality: TOP_QUALITY,
        slots: StatSlots {
            main: 13,
            haste: 7,
            crit: 6,
        },
    },
];

impl GearProfile {
    /// The profiles that belong to the tiers a command line asked for, in table order.
    #[must_use]
    pub fn select(tiers: &[GearTier]) -> Vec<GearProfile> {
        PROFILES
            .iter()
            .copied()
            .filter(|profile| tiers.contains(&profile.tier))
            .collect()
    }

    /// The profile the headline balance numbers use for a tier: the balanced medium set.
    #[must_use]
    pub fn headline(tier: GearTier) -> GearProfile {
        PROFILES
            .iter()
            .copied()
            .find(|profile| {
                profile.tier == tier && (tier != GearTier::Medium || profile.variant == "balanced")
            })
            .unwrap_or(PROFILES[0])
    }

    /// What the profile grants `hero`, as the stat block the game's pipeline takes.
    ///
    /// The main stat goes to the hero's own - Intellect for the FireMage and the Druid,
    /// Strength for the Warrior - exactly as the character sheet folds an item's
    /// `Stat::MainStat`.
    #[must_use]
    pub fn stat_block(&self, hero: Hero) -> StatBlock {
        let rating = LOOT_STAT_RATING * self.quality;
        let mut stats = StatBlock::zero();
        stats.add(hero.main_stat(), f32::from(self.slots.main) * rating);
        stats.add(Stat::Haste, f32::from(self.slots.haste) * rating);
        stats.add(Stat::CriticalStrike, f32::from(self.slots.crit) * rating);
        stats
    }

    /// A short label for a table: `medium/balanced`, `top`, ...
    #[must_use]
    pub fn label(&self) -> String {
        match self.tier {
            GearTier::Medium => format!("medium/{}", self.variant),
            _ => self.variant.to_owned(),
        }
    }

    /// The ratings in words, for the report's gear table.
    #[must_use]
    pub fn describe(&self, hero: Hero) -> String {
        let stats = self.stat_block(hero);
        format!(
            "{} pieces (item level {}) x{:.2}: +{:.1} {}, {:.1} Haste ({:.1}%), {:.1} Crit ({:.1}%)",
            self.pieces,
            if self.pieces == 0 { 0 } else { LOOT_ITEM_LEVEL },
            self.quality,
            stats.get(hero.main_stat()),
            hero.main_stat().name(),
            stats.get(Stat::Haste),
            stats.percent(Stat::Haste) * 100.0,
            stats.get(Stat::CriticalStrike),
            stats.percent(Stat::CriticalStrike) * 100.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_spends_exactly_two_stat_slots_per_piece() {
        for profile in PROFILES {
            assert_eq!(
                u16::from(profile.slots.main + profile.slots.haste + profile.slots.crit),
                2 * u16::from(profile.pieces),
                "{}",
                profile.label()
            );
            assert!(profile.pieces <= FULL_PIECES);
        }
    }

    #[test]
    fn the_main_stat_lands_on_each_heros_own() {
        let top = GearProfile::headline(GearTier::Top);
        assert!(top.stat_block(Hero::Warrior).get(Stat::Strength) > 0.0);
        assert_eq!(top.stat_block(Hero::Warrior).get(Stat::Intellect), 0.0);
        assert!(top.stat_block(Hero::Druid).get(Stat::Intellect) > 0.0);
        assert!(top.stat_block(Hero::FireMage).get(Stat::Intellect) > 0.0);
    }

    #[test]
    fn the_tiers_are_ordered_by_what_they_grant() {
        let total = |profile: &GearProfile| {
            let stats = profile.stat_block(Hero::FireMage);
            stats.get(Stat::Intellect) + stats.get(Stat::Haste) + stats.get(Stat::CriticalStrike)
        };
        let none = total(&GearProfile::headline(GearTier::None));
        let top = total(&GearProfile::headline(GearTier::Top));
        assert_eq!(none, 0.0);
        for medium in PROFILES.iter().filter(|p| p.tier == GearTier::Medium) {
            assert!(
                total(medium) > none && total(medium) < top,
                "{}",
                medium.label()
            );
        }
    }
}
