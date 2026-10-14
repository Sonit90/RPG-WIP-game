//! Builds: a set of talents, a Fire Mastery specialisation, and a rotation to play them.
//!
//! [`Build`] is the FireMage's; [`ClassBuild`] wraps any class's build, and [`builds_for`] is
//! the table of what the matrix runs.
//!
//! The talent definitions are [`engine_game::firemage::TALENTS`] and nothing else. This
//! module decides which *combinations* to try; it does not restate what any of them do.

use engine_game::character::stat::{Stat, StatBlock, rating_for_fraction};
use engine_game::firemage::{
    DEFAULT_TALENT_BUDGET, FireMastery, Gear, TALENT_COUNT, TalentGrid, TalentId,
};
use engine_game::sim::Simulation;

use crate::ai::{DruidRotation, Rotation, RotationAi, Style, WarriorRotation};
use crate::class::Class;

/// One FireMage build: talents, specialisation, rotation style and gear.
#[derive(Debug, Clone, PartialEq)]
pub struct Build {
    /// What to call it in the report.
    pub name: String,
    /// Which talents are bought.
    pub talents: Vec<TalentId>,
    /// Which stat Fire Mastery specialises into.
    pub mastery: FireMastery,
    /// Which rotation the AI plays.
    pub style: Style,
    /// What the character is wearing.
    pub gear: Gear,
}

/// Haste on a fully geared haste character, and crit on a fully geared crit one.
///
/// A range rather than a number: the class is balanced across the band a geared character
/// actually sits in, and a build that only works at the top of it is not balanced.
pub const GEARED_PRIMARY: [f32; 2] = [0.40, 0.50];

/// The off-stat a fully geared character carries whichever build they play.
pub const GEARED_SECONDARY: f32 = 0.20;

/// The middle of the geared band, which the headline numbers use.
#[must_use]
pub fn geared_primary_mid() -> f32 {
    (GEARED_PRIMARY[0] + GEARED_PRIMARY[1]) * 0.5
}

/// Gear for a haste character: `primary` haste and the secondary in crit.
#[must_use]
pub fn haste_gear(primary: f32) -> Gear {
    Gear::totalling(primary, GEARED_SECONDARY)
}

/// Gear for a crit character: `primary` crit and the secondary in haste.
#[must_use]
pub fn crit_gear(primary: f32) -> Gear {
    Gear::totalling(GEARED_SECONDARY, primary)
}

impl Build {
    /// A build with a name, a talent set and a specialisation.
    #[must_use]
    pub fn new(name: &str, talents: &[TalentId], mastery: FireMastery, style: Style) -> Self {
        // Gear follows the specialisation unless the caller says otherwise: a haste
        // character wears haste. Nothing is balanced at base stats, because nothing is
        // played at base stats.
        let gear = match mastery {
            FireMastery::Haste => haste_gear(geared_primary_mid()),
            FireMastery::Crit => crit_gear(geared_primary_mid()),
        };
        Self {
            name: name.to_owned(),
            talents: talents.to_vec(),
            mastery,
            style,
            gear,
        }
    }

    /// The same build in different gear.
    #[must_use]
    pub fn wearing(&self, gear: Gear) -> Self {
        Self {
            gear,
            ..self.clone()
        }
    }

    /// The grid gameplay reads.
    #[must_use]
    pub fn talents(&self) -> TalentGrid {
        TalentGrid::from_talents(&self.talents)
    }

    /// What the talents cost.
    #[must_use]
    pub fn cost(&self) -> u32 {
        self.talents
            .iter()
            .map(|talent| talent.definition().cost)
            .sum()
    }

    /// The talent names, joined for a report line.
    #[must_use]
    pub fn talent_names(&self) -> String {
        if self.talents.is_empty() {
            return "NONE".to_owned();
        }
        self.talents
            .iter()
            .map(|talent| talent.definition().name)
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// A short label: the style, the specialisation and the point cost.
    #[must_use]
    pub fn label(&self) -> String {
        format!(
            "{} / {} / {} PTS / {:.0}% HASTE {:.0}% CRIT",
            self.style.as_str(),
            self.mastery.as_str(),
            self.cost(),
            self.gear.haste * 100.0,
            (self.gear.crit_chance + engine_game::firemage::BASE_CRIT_CHANCE) * 100.0
        )
    }
}

/// The Detonation build the design describes, for the like-for-like comparison.
#[must_use]
pub fn designed_haste_build() -> Build {
    Build::new(
        "DESIGNED HASTE",
        &[
            TalentId::RapidCombustion,
            TalentId::BurningEmbers,
            TalentId::ExtendedFlame,
            TalentId::MoltenMeteor,
            TalentId::FurnaceMastery,
        ],
        FireMastery::Haste,
        Style::Haste,
    )
}

/// The Ignite-maintenance build the design describes.
#[must_use]
pub fn designed_crit_build() -> Build {
    Build::new(
        "DESIGNED CRIT",
        &[
            TalentId::MeteoricImpact,
            TalentId::SpellfireFeedback,
            TalentId::BurningPrecision,
            TalentId::InfernalCapacity,
            TalentId::ExtendedFlame,
        ],
        FireMastery::Crit,
        Style::Crit,
    )
}

/// Every maximal talent set that fits `budget`.
///
/// *Maximal* means no unbought talent would still fit: a build with points left over is
/// strictly worse than the same build with something else added, so enumerating those would
/// multiply the search by nothing. Twelve talents give 4096 subsets; the budget and this
/// rule cut that to a few hundred, which is what makes an exhaustive search affordable.
#[must_use]
pub fn maximal_combinations(budget: u32) -> Vec<Vec<TalentId>> {
    let costs: Vec<u32> = TalentId::ALL
        .iter()
        .map(|talent| talent.definition().cost)
        .collect();
    let mut out = Vec::new();

    for mask in 0u32..(1 << TALENT_COUNT) {
        let mut cost = 0;
        for (index, talent_cost) in costs.iter().enumerate() {
            if mask & (1 << index) != 0 {
                cost += talent_cost;
            }
        }
        if cost > budget {
            continue;
        }
        // Maximal: nothing left out would still fit.
        let extensible = (0..TALENT_COUNT)
            .any(|index| mask & (1 << index) == 0 && cost + costs[index] <= budget);
        if extensible {
            continue;
        }
        out.push(
            (0..TALENT_COUNT)
                .filter(|index| mask & (1 << index) != 0)
                .map(|index| TalentId::ALL[index])
                .collect(),
        );
    }
    out
}

/// Every build the combination search will run: each maximal set, both ways round.
///
/// Both specialisations and both rotations for every set, because which rotation a set
/// wants is exactly what the search is meant to discover rather than assume. Pairing haste
/// with the Detonation rotation and crit with the maintenance rotation is the *hypothesis*;
/// the other two pairings are how it gets tested.
#[must_use]
pub fn search_space(budget: u32) -> Vec<Build> {
    let mut builds = Vec::new();
    for talents in maximal_combinations(budget) {
        for (mastery, style) in [
            (FireMastery::Haste, Style::Haste),
            (FireMastery::Crit, Style::Crit),
            (FireMastery::Haste, Style::Crit),
            (FireMastery::Crit, Style::Haste),
        ] {
            let mut build = Build::new("", &talents, mastery, style);
            build.name = build.talent_names();
            builds.push(build);
        }
    }
    builds
}

/// The default budget, for a caller that has no opinion.
#[must_use]
pub fn default_budget() -> u32 {
    DEFAULT_TALENT_BUDGET
}

impl Build {
    /// The same build under another name, for the report's "Default / Build A / Build B".
    #[must_use]
    pub fn named(&self, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..self.clone()
        }
    }

    /// This build's built-in gear as the stat block the game's pipeline takes.
    ///
    /// The talent-search and gear-band studies state their gear as percentages of haste and
    /// crit on a character sheet ([`Gear::totalling`]). The game converts a rating to a
    /// fraction through `secondary_fraction`; this is its inverse, so a percentage becomes
    /// the rating that produces it and then travels the real pipeline like any other gear.
    #[must_use]
    pub fn gear_stats(&self) -> StatBlock {
        let mut stats = StatBlock::zero();
        stats.add(Stat::Intellect, self.gear.spell_power);
        stats.add(Stat::Haste, rating_for_fraction(self.gear.haste));
        stats.add(
            Stat::CriticalStrike,
            rating_for_fraction(self.gear.crit_chance),
        );
        stats
    }
}

/// A Warrior build: nothing to choose yet.
///
/// The Warrior has no talent system. The type exists so the build table has a row to put
/// "Default" in now and "Build A" in later: adding a talent system to the class is adding
/// fields here, and nothing else in the simulator changes.
#[derive(Debug, Clone, PartialEq)]
pub struct WarriorBuild {
    /// What to call it in the report.
    pub name: String,
}

/// A Druid build: nothing to choose yet, for the same reason as [`WarriorBuild`].
#[derive(Debug, Clone, PartialEq)]
pub struct DruidBuild {
    /// What to call it in the report.
    pub name: String,
}

/// **A build of any class.** The one type the matrix, the encounter and the report handle.
///
/// An enum over each class's own build type rather than a trait object: the report needs to
/// say which talents a build bought, and "which class" is a fact the encounter has to match
/// on anyway to apply a Mage's talents and Fire Mastery.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassBuild {
    /// A FireMage: talents, a specialisation and a rotation style.
    Mage(Build),
    /// A Warrior.
    Warrior(WarriorBuild),
    /// A Druid.
    Druid(DruidBuild),
}

impl ClassBuild {
    /// Which class this is a build of.
    #[must_use]
    pub fn class(&self) -> Class {
        match self {
            Self::Mage(_) => Class::Mage,
            Self::Warrior(_) => Class::Warrior,
            Self::Druid(_) => Class::Druid,
        }
    }

    /// What to call it in the report.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Mage(build) => &build.name,
            Self::Warrior(build) => &build.name,
            Self::Druid(build) => &build.name,
        }
    }

    /// The rotation that plays it.
    #[must_use]
    pub fn rotation(&self) -> Box<dyn Rotation> {
        match self {
            Self::Mage(build) => Box::new(RotationAi::new(build.style)),
            Self::Warrior(_) => Box::new(WarriorRotation::new()),
            Self::Druid(_) => Box::new(DruidRotation::new()),
        }
    }

    /// Puts the build's talents and specialisation on a simulation whose hero is already set.
    pub fn apply(&self, simulation: &mut Simulation) {
        if let Self::Mage(build) = self {
            simulation.mage_mut().set_talents(build.talents());
            simulation.mage_mut().mastery = build.mastery;
        }
    }

    /// The talents it bought, for a report line.
    #[must_use]
    pub fn talent_names(&self) -> String {
        match self {
            Self::Mage(build) => build.talent_names(),
            Self::Warrior(_) | Self::Druid(_) => "NONE (class has no talents yet)".to_owned(),
        }
    }
}

/// **The build table: every build of a class the matrix runs.**
///
/// Warrior and Druid have no talents, so each has its `Default` and nothing else. A class
/// that grows talents adds `Build A` and `Build B` rows here.
#[must_use]
pub fn builds_for(class: Class) -> Vec<ClassBuild> {
    match class {
        Class::Warrior => vec![ClassBuild::Warrior(WarriorBuild {
            name: "Default".to_owned(),
        })],
        Class::Druid => vec![ClassBuild::Druid(DruidBuild {
            name: "Default".to_owned(),
        })],
        Class::Mage => vec![
            // No talents: the class as it is before a single point is spent, played with the
            // Detonation rotation because that rotation needs nothing from the grid.
            ClassBuild::Mage(Build::new("Default", &[], FireMastery::Haste, Style::Haste)),
            ClassBuild::Mage(designed_haste_build().named("Build A")),
            ClassBuild::Mage(designed_crit_build().named("Build B")),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_enumerated_build_fits_the_budget_and_wastes_nothing() {
        let budget = 20;
        let combinations = maximal_combinations(budget);
        assert!(!combinations.is_empty());

        for talents in &combinations {
            let cost: u32 = talents.iter().map(|talent| talent.definition().cost).sum();
            assert!(cost <= budget, "{talents:?} costs {cost}");

            // Maximal: nothing that was *left out* would still fit in what is left over.
            let unspent = budget - cost;
            let addable = TalentId::ALL
                .into_iter()
                .filter(|talent| !talents.contains(talent))
                .any(|talent| talent.definition().cost <= unspent);
            assert!(
                !addable || talents.len() == TALENT_COUNT,
                "{talents:?} left {unspent} points that another talent would have fitted in"
            );
        }
    }

    #[test]
    fn the_designed_builds_fit_the_default_budget() {
        for build in [designed_haste_build(), designed_crit_build()] {
            assert!(
                build.cost() <= default_budget(),
                "{} costs {}",
                build.name,
                build.cost()
            );
            assert_eq!(
                build.talents().purchased_count() as usize,
                build.talents.len()
            );
        }
    }
}
