//! Books in the power-scaling simulation.
//!
//! ```text
//! p2_books = p2_gems gear  +  the class's talent books  +  the best option of each enhancement book
//! ```
//!
//! Books are not stats on the sheet. A talent book is a talent point (the plan buys the best
//! talent it can afford), and an enhancement option is a hidden talent node: both are applied to the
//! simulation's talent grid exactly as the realm applies them (`TalentGrid::grant_index`), so the
//! measured change is the game's own `<Hero>TalentEffects` at work.
//!
//! The study at the end of the report ([`study`]) measures every option of every enhancement book
//! alone on `p2_gems`, which is what justifies [`shipped_plan`]'s choices: the best option of each
//! book for the class, by single-target and eight-target damage.

use std::fmt::Write as _;

use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::talent::books::HeroBooks;
use engine_game::talent::{TalentGrid, hero_talents};

use crate::Options;
use crate::build::builds_for;
use crate::class::Class;
use crate::gear::{GearProfile, GearTier, Worn};
use crate::matrix::{AOE8, SCENARIOS, ST60};
use crate::scenario::run_scenario;

/// The talent a class's talent books are spent on (3 books, 3 points: one three-point talent),
/// when the build does not own it already.
#[must_use]
pub fn book_talent(class: Class) -> &'static str {
    match class {
        Class::Warrior => "unbroken_momentum",
        Class::Druid => "swift_wrath",
        Class::Mage => "burning_embers",
    }
}

/// Which books a character has and which options are in force.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BookPlan {
    /// The hidden node keys of the options in force.
    pub nodes: Vec<String>,
    /// A talent the books' points buy.
    pub talent: Option<String>,
}

/// **The shipped plan** for a class: its talent books' talent and the best option of each
/// enhancement book, as the study at the end of the report measures them.
#[must_use]
pub fn shipped_plan(class: Class) -> BookPlan {
    let nodes: &[&str] = match class {
        // The stomp is the Warrior's damage button; the execute is its finisher.
        Class::Warrior => &[
            "bk_crushing_quake",
            "bk_battering_charge",
            "bk_headsmans_mark",
        ],
        Class::Druid => &["bk_searing_bloom", "bk_astral_mend", "bk_deep_peace"],
        Class::Mage => &["bk_twin_pyre", "bk_impact_crater", "bk_slow_burn"],
    };
    BookPlan {
        nodes: nodes.iter().map(|key| (*key).to_owned()).collect(),
        talent: Some(book_talent(class).to_owned()),
    }
}

/// Puts `plan` on a simulation whose hero and talents are already set.
pub fn apply(simulation: &mut Simulation, hero: Hero, plan: &BookPlan) {
    let mut grid: TalentGrid = simulation.talent_grid_of(hero);
    let table = hero_talents(hero);
    // The books' points: enough for the one talent, which is bought if the build lacks it.
    grid.grant_points(3);
    if let Some(index) = plan.talent.as_deref().and_then(|key| table.index_of(key)) {
        let _ = grid.purchase_index(index);
    }
    for key in &plan.nodes {
        if let Some(index) = table.index_of(key) {
            grid.grant_index(index);
        }
    }
    match hero {
        Hero::FireMage => simulation.mage_mut().set_talents(grid),
        _ => simulation.set_talents(grid),
    }
}

/// One option's measured gain over `p2_gems`.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionMeasure {
    /// The class.
    pub class: Class,
    /// The book's name.
    pub book: String,
    /// The option's name.
    pub option: String,
    /// The option's node key.
    pub node: String,
    /// Single-target gain, a fraction.
    pub single: f64,
    /// Eight-target gain, a fraction.
    pub area: f64,
}

/// Measures every enhancement option of every class alone on the `Default` build in `p2_gems`.
#[must_use]
pub fn study(options: &Options) -> Vec<OptionMeasure> {
    let profile = GearProfile::headline(GearTier::P2Gems);
    let mut out = Vec::new();
    for class in &options.classes {
        let Some(build) = builds_for(*class).into_iter().next() else {
            continue;
        };
        let hero = class.hero();
        let sheet = profile.sheet(hero);
        let measure = |plan: BookPlan, scenario: usize| {
            let config = SCENARIOS[scenario].config(*class);
            let gear = if plan == BookPlan::default() {
                Worn::from(sheet.clone())
            } else {
                Worn::Books(Box::new(sheet.clone()), plan)
            };
            run_scenario(&config, &build, &gear, options.iterations, options.seed).dps
        };
        let base = (
            measure(BookPlan::default(), ST60),
            measure(BookPlan::default(), AOE8),
        );
        // The talent the three talent books pay for, alone.
        let talent_only = BookPlan {
            nodes: Vec::new(),
            talent: shipped_plan(*class).talent,
        };
        out.push(OptionMeasure {
            class: *class,
            book: "Talent books (3)".to_owned(),
            option: book_talent(*class).replace('_', " ").to_uppercase(),
            node: String::new(),
            single: measure(talent_only.clone(), ST60) / base.0.max(1e-9) - 1.0,
            area: measure(talent_only, AOE8) / base.1.max(1e-9) - 1.0,
        });
        let books: &HeroBooks = &engine_game::talent::catalog().books[hero.index()];
        for book in &books.enhancement {
            for option in &book.options {
                let plan = BookPlan {
                    nodes: vec![option.node.clone()],
                    talent: None,
                };
                let single = measure(plan.clone(), ST60) / base.0.max(1e-9) - 1.0;
                let area = measure(plan, AOE8) / base.1.max(1e-9) - 1.0;
                out.push(OptionMeasure {
                    class: *class,
                    book: book.name.clone(),
                    option: option.name.clone(),
                    node: option.node.clone(),
                    single,
                    area,
                });
            }
        }
    }
    out
}

/// The "Books" section.
pub fn write_section(
    out: &mut String,
    measures: &[OptionMeasure],
    scaling: Option<&crate::scaling::Scaling>,
) {
    let _ = writeln!(out, "## 8. Books\n");
    let _ = writeln!(
        out,
        "Every enhancement option alone, on the `Default` build in `p2_gems` gear, against the same \
         build without it (damage per second; a negative number is a trade, not a mistake). The \
         `p2_books` tier wears the starred option of each book, plus the talent the three talent \
         books pay for, on top of `p2_gems`.\n"
    );
    let _ = writeln!(
        out,
        "| Class | Book | Option | Single target | 8 targets | In `p2_books` |"
    );
    let _ = writeln!(out, "|---|---|---|---:|---:|:-:|");
    for measure in measures {
        let starred =
            measure.node.is_empty() || shipped_plan(measure.class).nodes.contains(&measure.node);
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:+.1}% | {:+.1}% | {} |",
            measure.class.title(),
            measure.book,
            measure.option,
            measure.single * 100.0,
            measure.area * 100.0,
            if starred { "*" } else { "" },
        );
    }
    let _ = writeln!(out);
    if let Some(scaling) = scaling {
        let _ = writeln!(
            out,
            "**What books add on top of the top gear tier** (the `Default` build, single target 60 s):\n"
        );
        let _ = writeln!(out, "| Class | `p2_gems` DPS | `p2_books` DPS | Books add |");
        let _ = writeln!(out, "|---|---:|---:|---:|");
        for class in &scaling.classes {
            let find = |tier| class.tiers.iter().find(|t| t.tier == tier).map(|t| t.dps);
            if let (Some(set), Some(books)) = (find(GearTier::P2Gems), find(GearTier::P2Books)) {
                let _ = writeln!(
                    out,
                    "| {} | {:.0} | {:.0} | {:+.1}% |",
                    class.class.title(),
                    set,
                    books,
                    (books / set.max(1e-9) - 1.0) * 100.0
                );
            }
        }
        let _ = writeln!(out);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::scenario::Encounter;

    #[test]
    fn the_plan_reaches_the_talent_grid_and_every_node_it_names_exists() {
        engine_realm::talents::install_builtin().expect("talents");
        for class in Class::ALL {
            let hero = class.hero();
            let plan = shipped_plan(class);
            assert_eq!(plan.nodes.len(), 3, "one option per enhancement book");
            let books = &engine_game::talent::catalog().books[hero.index()];
            for (book, node) in books.enhancement.iter().zip(&plan.nodes) {
                assert!(
                    book.options.iter().any(|option| &option.node == node),
                    "{node} is not an option of {}",
                    book.id
                );
            }
            let profile = GearProfile::headline(GearTier::P2Books);
            let build = builds_for(class).into_iter().next().expect("a build");
            let config = SCENARIOS[ST60].config(class);
            let encounter = Encounter::new(&config, &build, &profile.worn(hero), 1);
            let grid = encounter.simulation().talent_grid_of(hero);
            assert_eq!(grid.granted_keys(), plan.nodes, "{class:?}");
            assert!(
                grid.purchased_keys()
                    .contains(&book_talent(class).to_owned()),
                "{class:?} bought its book talent"
            );
        }
    }
}
