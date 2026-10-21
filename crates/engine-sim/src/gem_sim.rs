//! The gem economy simulation (`--gem-sim`): how many runs until a max-level gem.
//!
//! One simulated player farms the same dungeon over and over and puts everything they find of
//! one gem kind through the upgrade chain, exactly as the hub rules allow it:
//!
//! ```text
//! a run:  MOBS ordinary kills, ELITES elite kills, a boss kill, a chest
//!           each rolls `engine_game::loot::roll_gem` (the shipped `gems.json` drop rules)
//!           gold income = the shipped `loot.json` gold rules, GOLD_SHARE of it spent on gems
//! between runs: while the stack holds `consume` gems and the gold pays, attempt an upgrade
//!               (lowest level first); success gives one gem a level higher, failure keeps one
//! done when the first max-level gem of the chosen kind exists
//! ```
//!
//! The drop rolls, the success chances, the gold costs and the failure rule are the shipped
//! catalog's; nothing here restates a number. The design target is about 100 runs to the first
//! max-level gem of a chosen kind; the test below holds the median to 60-150.

use std::fmt::Write as _;

use engine_game::character::ItemId;
use engine_game::firemage::Rng;
use engine_game::loot::{DropSource, LootRules, roll_gem};
use engine_realm::gems::GemCatalog;

/// The key level of the headline figure.
pub const KEY_LEVEL: u32 = 10;
/// The key levels the report covers.
pub const KEY_LEVELS: [u32; 5] = [1, 5, 10, 15, 20];
/// Ordinary kills per run (the Graveyard Crypt places 36; later dungeons trade some for elites).
pub const MOBS: u32 = 30;
/// Elite kills per run.
pub const ELITES: u32 = 6;
/// Boss kills per run.
pub const BOSSES: u32 = 1;
/// Chests per run (the boss chest).
pub const CHESTS: u32 = 1;
/// The share of a run's gold income the player spends on gems; the rest goes to sharpening,
/// reforging and the other gold sinks.
pub const GOLD_SHARE: f64 = 0.5;
/// The kind the player farms (an index into the catalog's kinds): the first, main stat.
const KIND: usize = 0;
/// A trial that has not finished by now is counted as this many runs.
const MAX_RUNS: u32 = 5_000;

/// What one simulated player went through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trial {
    /// Runs until the first max-level gem of the chosen kind.
    pub runs: u32,
    /// Upgrade attempts made.
    pub attempts: u32,
    /// Gold spent on them.
    pub gold: u64,
}

/// The gold one run pays one player on average, from the shipped loot rules.
#[must_use]
pub fn gold_per_run(rules: &LootRules) -> f64 {
    let gold = &rules.gold;
    let mean = (f64::from(gold.per_mob_min) + f64::from(gold.per_mob_max)) / 2.0;
    mean * f64::from(MOBS + ELITES) + f64::from(gold.boss_amount) * f64::from(BOSSES)
}

/// One player's farm, from `seed`.
#[must_use]
pub fn trial(rules: &LootRules, gems: &GemCatalog, key_level: u32, seed: u64) -> Trial {
    let mut rng = Rng::new(seed);
    let max = usize::from(gems.max_level);
    let ids: &[ItemId] = &rules.gems.items[KIND];
    // counts[level] = gems of the chosen kind at that level (index 0 unused).
    let mut counts = vec![0_u32; max + 1];
    let income = gold_per_run(rules) * GOLD_SHARE;
    let (mut gold, mut spent, mut attempts) = (0.0_f64, 0_u64, 0_u32);
    let consume = u32::from(gems.upgrade.consume);
    let sources = [
        (DropSource::Mob, MOBS),
        (DropSource::Elite, ELITES),
        (DropSource::Boss, BOSSES),
        (DropSource::Chest, CHESTS),
    ];
    for run in 1..=MAX_RUNS {
        for (source, events) in sources {
            for _ in 0..events {
                if let Some(id) = roll_gem(rules, source, key_level, 0, &mut rng)
                    && let Some(level) = ids.iter().position(|candidate| *candidate == id)
                {
                    counts[level + 1] += 1;
                }
            }
        }
        gold += income;
        // Lowest level first; an attempt costs the step's gold and `consume` gems, a failure
        // returns one.
        for level in 1..max {
            let Some(step) = gems.step(u8::try_from(level).unwrap_or(u8::MAX)) else {
                continue;
            };
            while counts[level] >= consume && gold >= step.gold as f64 {
                gold -= step.gold as f64;
                spent += step.gold;
                attempts += 1;
                counts[level] -= consume;
                if f64::from(rng.next_f32()) < step.success_chance {
                    counts[level + 1] += 1;
                } else {
                    counts[level] += 1;
                }
            }
        }
        if counts[max] > 0 {
            return Trial {
                runs: run,
                attempts,
                gold: spent,
            };
        }
    }
    Trial {
        runs: MAX_RUNS,
        attempts,
        gold: spent,
    }
}

/// Many players at one key level: the distribution of runs to the first max-level gem.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The key level.
    pub key_level: u32,
    /// Runs per player, sorted.
    pub runs: Vec<u32>,
    /// Mean upgrade attempts.
    pub attempts: f64,
    /// Mean gold spent on upgrades.
    pub gold: f64,
}

impl Cell {
    /// The `p`-quantile of runs (`0..=1`).
    #[must_use]
    pub fn quantile(&self, p: f64) -> u32 {
        let last = self.runs.len().saturating_sub(1);
        self.runs
            .get(((last as f64) * p).round() as usize)
            .copied()
            .unwrap_or(0)
    }

    /// The median.
    #[must_use]
    pub fn median(&self) -> u32 {
        self.quantile(0.5)
    }

    /// The mean.
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.runs.iter().map(|r| f64::from(*r)).sum::<f64>() / self.runs.len().max(1) as f64
    }
}

/// Simulates `players` at `key_level`.
#[must_use]
pub fn run_cell(
    rules: &LootRules,
    gems: &GemCatalog,
    key_level: u32,
    players: u32,
    seed: u64,
) -> Cell {
    let trials: Vec<Trial> = (0..players)
        .map(|n| {
            let mix = u64::from(key_level) * 0x85EB_CA6B + u64::from(n) * 0x9E37_79B9;
            trial(rules, gems, key_level, seed ^ mix)
        })
        .collect();
    let mut runs: Vec<u32> = trials.iter().map(|t| t.runs).collect();
    runs.sort_unstable();
    let count = trials.len().max(1) as f64;
    Cell {
        key_level,
        runs,
        attempts: trials.iter().map(|t| f64::from(t.attempts)).sum::<f64>() / count,
        gold: trials.iter().map(|t| t.gold as f64).sum::<f64>() / count,
    }
}

/// Every key level of [`KEY_LEVELS`].
#[must_use]
pub fn run(rules: &LootRules, gems: &GemCatalog, players: u32, seed: u64) -> Vec<Cell> {
    KEY_LEVELS
        .into_iter()
        .map(|key| run_cell(rules, gems, key, players, seed))
        .collect()
}

/// The expected number of level-1 gems that one max-level gem is worth, from the success
/// chances alone (an attempt consumes `consume`, a failure returns one).
#[must_use]
pub fn level_one_equivalents(gems: &GemCatalog) -> f64 {
    let consume = f64::from(gems.upgrade.consume);
    gems.upgrade
        .steps
        .iter()
        .map(|step| (consume - (1.0 - step.success_chance)) / step.success_chance)
        .product()
}

/// The expected gold one max-level gem costs in upgrades, from the chances alone.
#[must_use]
pub fn upgrade_gold(gems: &GemCatalog) -> f64 {
    let consume = f64::from(gems.upgrade.consume);
    let mut needed = 1.0;
    let mut total = 0.0;
    for step in gems.upgrade.steps.iter().rev() {
        // `needed` gems of the next level take `needed / chance` attempts.
        let attempts = needed / step.success_chance;
        total += attempts * step.gold as f64;
        needed = attempts * (consume - (1.0 - step.success_chance));
    }
    total
}

/// The report's gem economy section body (no heading), shared by the file and the main report.
pub fn write_body(out: &mut String, rules: &LootRules, gems: &GemCatalog, cells: &[Cell]) {
    let _ = writeln!(
        out,
        "One player farms a dungeon of {MOBS} ordinary kills, {ELITES} elite kills, {BOSSES} \
         boss and {CHESTS} chest per run and upgrades every gem of one kind ({kind}) through the \
         chain, spending {share:.0} % of the run's gold income ({income:.0} gold per run, from \
         `loot.json`) on it. Drops use `gems.json` through `engine_game::loot::roll_gem`; the \
         upgrade is `consume` {consume} of one level, then success at the step's chance, and a \
         failure keeps one. A max-level gem is level {max}.\n",
        kind = gems.kinds[KIND].name,
        share = GOLD_SHARE * 100.0,
        income = gold_per_run(rules),
        consume = gems.upgrade.consume,
        max = gems.max_level,
    );
    let _ = writeln!(
        out,
        "Chain, from the chances alone: one max-level gem is worth {:.1} level-1 gems and \
         {:.0} gold of upgrades.\n",
        level_one_equivalents(gems),
        upgrade_gold(gems)
    );
    let _ = writeln!(
        out,
        "| Key level | Median runs | 10th pct | 90th pct | Mean | Upgrade attempts | Upgrade gold |"
    );
    let _ = writeln!(out, "|---:|---:|---:|---:|---:|---:|---:|");
    for cell in cells {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.0} | {:.0} | {:.0} |",
            cell.key_level,
            cell.median(),
            cell.quantile(0.1),
            cell.quantile(0.9),
            cell.mean(),
            cell.attempts,
            cell.gold
        );
    }
    let _ = writeln!(
        out,
        "\nThe design target at key level {KEY_LEVEL} is a median of about 100 runs (the test \
         holds it between 60 and 150).\n"
    );
}

/// The standalone markdown report (`--gem-sim`).
#[must_use]
pub fn report(rules: &LootRules, gems: &GemCatalog, cells: &[Cell], seed: u64) -> String {
    let players = cells.first().map_or(0, |cell| cell.runs.len());
    let mut out = String::new();
    let _ = writeln!(out, "# Gem economy\n");
    let _ = writeln!(
        out,
        "Generated by `engine-sim --gem-sim` (seed {seed:#x}, {players} players per key level). \
         Regenerate after editing `assets/catalog/gems.json`.\n"
    );
    write_body(&mut out, rules, gems, cells);
    out
}

/// The main report's section: the headline key levels only, from a modest sample.
pub fn write_section(out: &mut String, seed: u64) {
    let _ = writeln!(out, "## 9. Gem economy\n");
    let (Some(rules), Some(gems)) = (
        engine_realm::loot::builtin_rules(),
        engine_realm::gems::builtin(),
    ) else {
        let _ = writeln!(out, "Not run: the shipped gem rules did not build.\n");
        return;
    };
    let cells = run(rules, gems, 400, seed);
    write_body(out, rules, gems, &cells);
    let _ = writeln!(
        out,
        "`engine-sim --gem-sim` writes the full report (`docs/balance/gem-economy.md`). The \
         `p2_gems` gear tier in section 7 is `p2_set` with a level-{} {} gem in every socket.\n",
        gems.max_level, gems.kinds[KIND].name
    );
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn setup() -> (&'static LootRules, &'static GemCatalog) {
        (
            engine_realm::loot::builtin_rules().expect("the shipped loot rules build"),
            engine_realm::gems::builtin().expect("the shipped gem catalog parses"),
        )
    }

    #[test]
    fn a_max_level_gem_takes_about_a_hundred_runs() {
        let (rules, gems) = setup();
        let cell = run_cell(rules, gems, KEY_LEVEL, 600, 0xBEEF);
        let median = cell.median();
        assert!(
            (60..=150).contains(&median),
            "median {median} runs to a max-level gem at key {KEY_LEVEL}"
        );
        // Nobody is stuck, and the spread is sane.
        assert!(cell.quantile(1.0) < MAX_RUNS);
        assert!(cell.quantile(0.1) < median && median < cell.quantile(0.9));
    }

    #[test]
    fn the_whole_key_range_stays_in_a_playable_band() {
        let (rules, gems) = setup();
        let low = run_cell(rules, gems, 1, 300, 3).median();
        let high = run_cell(rules, gems, 20, 300, 3).median();
        assert!(
            high <= low,
            "a higher key must not slow gems down: {high} vs {low}"
        );
        assert!(
            (40..=250).contains(&low) && (40..=250).contains(&high),
            "{low} {high}"
        );
    }

    #[test]
    fn the_same_seed_gives_the_same_report() {
        let (rules, gems) = setup();
        let a = report(rules, gems, &run(rules, gems, 50, 9), 9);
        let b = report(rules, gems, &run(rules, gems, 50, 9), 9);
        assert_eq!(a, b);
        assert!(a.contains("Median runs"));
    }

    #[test]
    fn the_analytic_chain_matches_the_catalog() {
        let (_, gems) = setup();
        // Every level-up needs more than the consume count in level-below gems on average.
        let one = level_one_equivalents(gems);
        let consume = f64::from(gems.upgrade.consume);
        assert!(one > consume.powi(i32::from(gems.max_level) - 1) * 0.5);
        assert!(upgrade_gold(gems) > 0.0);
    }
}
