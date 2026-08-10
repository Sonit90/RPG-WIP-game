//! Builds: a set of talents, a Fire Mastery specialisation, and a rotation to play them.
//!
//! The talent definitions are [`engine_game::firemage::TALENTS`] and nothing else. This
//! module decides which *combinations* to try; it does not restate what any of them do.

use engine_game::firemage::{
    DEFAULT_TALENT_BUDGET, FireMastery, Gear, TALENT_COUNT, TalentGrid, TalentId,
};

use crate::ai::Style;

/// One build the simulator can run.
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
