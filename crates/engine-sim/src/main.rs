//! The FireMage combat simulator.
//!
//! ```text
//! engine-sim                 the designed builds, four target counts, two durations
//! engine-sim --search        every maximal talent combination, ranked
//! engine-sim --iterations 500 --budget 20 --out docs/firemage-sim-report.md
//! ```
//!
//! It drives the game's own [`engine_game::sim::Simulation`]: the same ability table, the
//! same Ignite, the same Heat, the same damage function. Nothing here re-implements a
//! formula, which is what makes a number it prints a number the game will produce.

mod ai;
mod build;
mod report;
mod scenario;

use std::fmt::Write as _;
use std::path::PathBuf;

use build::{
    Build, GEARED_PRIMARY, GEARED_SECONDARY, crit_gear, designed_crit_build, designed_haste_build,
    geared_primary_mid, haste_gear, search_space,
};
use report::{RankRow, ScenarioReport, comparison, ranking};
use scenario::{ScenarioConfig, run_scenario};

/// The target counts every scenario is run at.
const TARGET_COUNTS: [usize; 4] = [1, 3, 8, 15];

/// The fight lengths every scenario is run at.
const DURATIONS: [f32; 2] = [60.0, 90.0];

/// Health a target holds at each count.
///
/// A lone target is a boss and lives; a pack of fifteen is trash and does not. Without this
/// the fifteen-target scenario would measure a fight that never ends, which is the one
/// scenario a real player never has.
fn health_for(targets: usize) -> f32 {
    match targets {
        1 => 90_000.0,
        3 => 45_000.0,
        8 => 22_000.0,
        _ => 12_000.0,
    }
}

/// How the run was configured on the command line.
struct Options {
    /// Run the whole talent combination search rather than the two designed builds.
    search: bool,
    /// Fights averaged per scenario.
    iterations: u32,
    /// Fights averaged per scenario during the search sweep.
    sweep_iterations: u32,
    /// How many of the sweep's best builds are re-run at full accuracy.
    finalists: usize,
    /// Talent points a build may spend.
    budget: u32,
    /// Where to write the report, if anywhere.
    out: Option<PathBuf>,
    /// The seed every fight is derived from.
    seed: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            search: false,
            iterations: 60,
            sweep_iterations: 8,
            finalists: 12,
            budget: build::default_budget(),
            out: None,
            seed: 0xF12E_5EED,
        }
    }
}

fn main() {
    let options = parse_options();
    let mut out = String::with_capacity(64 * 1024);

    let _ = writeln!(out, "# FireMage simulation report\n");
    let _ = writeln!(out, "```text");
    let _ = writeln!(
        out,
        "SEED {}   BUDGET {} PTS   ITERATIONS {}",
        options.seed, options.budget, options.iterations
    );
    let _ = writeln!(
        out,
        "GEAR  HASTE BUILD {:.0}% HASTE / {:.0}% CRIT     CRIT BUILD {:.0}% CRIT / {:.0}% HASTE",
        geared_primary_mid() * 100.0,
        GEARED_SECONDARY * 100.0,
        geared_primary_mid() * 100.0,
        GEARED_SECONDARY * 100.0
    );
    let _ = writeln!(out, "```\n");

    let haste = designed_haste_build();
    let crit = designed_crit_build();

    let _ = writeln!(out, "## Designed builds\n");
    let _ = writeln!(out, "```text");
    let mut haste_dps = [0.0; 4];
    let mut crit_dps = [0.0; 4];
    for (index, targets) in TARGET_COUNTS.into_iter().enumerate() {
        for duration in DURATIONS {
            let config = scenario_for(targets, duration);
            for (build, table) in [(&haste, &mut haste_dps), (&crit, &mut crit_dps)] {
                let result = run_scenario(&config, build, options.iterations, options.seed);
                if (duration - DURATIONS[0]).abs() < f32::EPSILON {
                    table[index] = result.dps;
                }
                let report = ScenarioReport {
                    build: build.clone(),
                    config: config.clone(),
                    iterations: options.iterations,
                    result,
                };
                let _ = writeln!(out, "{}", report.detail());
            }
        }
    }
    let _ = writeln!(out, "```\n");

    let _ = writeln!(out, "## Haste versus Crit, against the balance targets\n");
    let _ = writeln!(out, "Sixty-second fights.\n");
    let _ = writeln!(out, "```text");
    let _ = write!(out, "{}", comparison(&haste_dps, &crit_dps));
    let _ = writeln!(out, "```\n");

    // The gear band. A build that is only balanced at one point of it is not balanced: the
    // primary stat runs from 40 % to 50 % on a geared character, so both ends are measured.
    let _ = writeln!(
        out,
        "## Across the geared band
"
    );
    let _ = writeln!(
        out,
        "Sixty-second fights. Primary stat: haste for the Detonation build, critical strike for the maintenance one. The off-stat is {:.0}% either way.
",
        GEARED_SECONDARY * 100.0
    );
    let _ = writeln!(out, "```text");
    let _ = writeln!(
        out,
        "{:<9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "PRIMARY", "BUILD", "1 TGT", "3 TGT", "8 TGT", "15 TGT"
    );
    for primary in GEARED_PRIMARY {
        for (build, gear) in [(&haste, haste_gear(primary)), (&crit, crit_gear(primary))] {
            let dressed = build.wearing(gear);
            let dps = measure(&dressed, options.iterations, options.seed);
            let _ = writeln!(
                out,
                "{:<9} {:>9} {:>9.0} {:>9.0} {:>9.0} {:>9.0}",
                format!("{:.0}%", primary * 100.0),
                dressed.style.as_str(),
                dps[0],
                dps[1],
                dps[2],
                dps[3]
            );
        }
    }
    let _ = writeln!(
        out,
        "```
"
    );

    if options.search {
        let _ = writeln!(out, "## Talent combination search\n");
        let (rows, searched) = run_search(&options);
        let _ = writeln!(
            out,
            "{} maximal combinations x 4 pairings, swept at {} iterations, \
             the best {} re-run at {}.\n",
            searched, options.sweep_iterations, options.finalists, options.iterations
        );
        let _ = writeln!(out, "```text");
        let _ = write!(out, "{}", ranking(&rows, options.finalists));
        let _ = writeln!(out, "```\n");

        let _ = writeln!(out, "### Best build per target count\n");
        let _ = writeln!(out, "```text");
        for (index, targets) in TARGET_COUNTS.into_iter().enumerate() {
            let mut best = rows.clone();
            best.sort_by(|a, b| {
                b.dps[index]
                    .partial_cmp(&a.dps[index])
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(row) = best.first() {
                let _ = writeln!(
                    out,
                    "{:>2} TARGET  {:>9.0} DPS  {}  {}",
                    targets,
                    row.dps[index],
                    row.build.label(),
                    row.build.talent_names()
                );
            }
        }
        let _ = writeln!(out, "```\n");
    }

    print!("{out}");
    if let Some(path) = &options.out {
        match std::fs::write(path, &out) {
            Ok(()) => println!("\nwritten to {}", path.display()),
            Err(error) => eprintln!("could not write {}: {error}", path.display()),
        }
    }
}

/// The scenario for a target count and a fight length.
fn scenario_for(targets: usize, duration: f32) -> ScenarioConfig {
    ScenarioConfig {
        targets,
        target_health: health_for(targets),
        duration,
        ..ScenarioConfig::default()
    }
}

/// Sweeps every combination cheaply, then re-runs the best at full accuracy.
///
/// Returns the ranked finalists and how many talent combinations were searched.
fn run_search(options: &Options) -> (Vec<RankRow>, usize) {
    let builds = search_space(options.budget);
    let combinations = builds.len() / 4;
    eprintln!(
        "sweeping {} builds at {} iterations each...",
        builds.len(),
        options.sweep_iterations
    );

    let mut swept: Vec<RankRow> = builds
        .into_iter()
        .map(|build| RankRow {
            dps: measure(&build, options.sweep_iterations, options.seed),
            build,
        })
        .collect();
    swept.sort_by(|a, b| {
        b.average()
            .partial_cmp(&a.average())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // The sweep is noisy on purpose - it is there to throw away the obviously bad. The
    // finalists are measured properly before anything is concluded from them.
    let finalists = options.finalists.min(swept.len());
    eprintln!(
        "re-running {finalists} finalists at {} iterations...",
        options.iterations
    );
    let mut rows: Vec<RankRow> = swept
        .into_iter()
        .take(finalists)
        .map(|row| RankRow {
            dps: measure(&row.build, options.iterations, options.seed),
            build: row.build,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.average()
            .partial_cmp(&a.average())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (rows, combinations)
}

/// One build's DPS at each target count, over sixty seconds.
fn measure(build: &Build, iterations: u32, seed: u64) -> [f64; 4] {
    let mut dps = [0.0; 4];
    for (index, targets) in TARGET_COUNTS.into_iter().enumerate() {
        let config = scenario_for(targets, DURATIONS[0]);
        dps[index] = run_scenario(&config, build, iterations, seed).dps;
    }
    dps
}

/// Reads the command line. Deliberately tiny: this is a developer tool, not a product.
fn parse_options() -> Options {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--search" => options.search = true,
            "--iterations" => {
                options.iterations = args.next().and_then(|v| v.parse().ok()).unwrap_or(60);
            }
            "--sweep-iterations" => {
                options.sweep_iterations = args.next().and_then(|v| v.parse().ok()).unwrap_or(8);
            }
            "--finalists" => {
                options.finalists = args.next().and_then(|v| v.parse().ok()).unwrap_or(12);
            }
            "--budget" => {
                options.budget = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(build::default_budget);
            }
            "--seed" => {
                options.seed = args.next().and_then(|v| v.parse().ok()).unwrap_or(1);
            }
            "--out" => options.out = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!(
                    "engine-sim [--search] [--iterations N] [--sweep-iterations N] \
                     [--finalists N] [--budget N] [--seed N] [--out PATH]"
                );
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    options
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use ai::{DetonateRules, RotationAi, Style};
    use engine_game::firemage::{FireMastery, TalentId};
    use scenario::Encounter;

    fn quick(targets: usize, duration: f32) -> ScenarioConfig {
        ScenarioConfig {
            targets,
            target_health: health_for(targets),
            duration,
            ..ScenarioConfig::default()
        }
    }

    #[test]
    fn a_fight_produces_damage_and_a_full_set_of_counters() {
        let build = designed_haste_build();
        let result = run_scenario(&quick(1, 30.0), &build, 2, 7);

        assert!(result.damage > 0.0, "the rotation dealt nothing");
        assert!(result.dps > 0.0);
        let counters = &result.counters;
        assert!(counters.casts_started > 0);
        assert!(counters.casts_completed > 0);
        assert!(counters.ignite_applications > 0);
        assert!(
            counters.detonations > 0,
            "the detonation build never detonated"
        );
        assert!(counters.average_heat() > 0.0);
        assert!(counters.average_ignite_stacks() > 0.0);
    }

    #[test]
    fn the_maintenance_build_holds_its_burn_instead_of_spending_it() {
        // The two rotations must actually be different rotations.
        let haste = run_scenario(&quick(1, 45.0), &designed_haste_build(), 2, 11);
        let crit = run_scenario(&quick(1, 45.0), &designed_crit_build(), 2, 11);

        assert!(haste.counters.detonations > crit.counters.detonations);
        assert!(
            crit.counters.average_ignite_stacks() > haste.counters.average_ignite_stacks(),
            "the maintenance build did not hold a bigger burn"
        );
    }

    #[test]
    fn targets_really_die_and_the_fight_ends_with_them() {
        // Fifteen targets with the health of trash: the pack must not survive the fight.
        let config = ScenarioConfig {
            target_health: 4_000.0,
            duration: 90.0,
            ..quick(15, 90.0)
        };
        let result = run_scenario(&config, &designed_haste_build(), 2, 3);
        assert!(result.deaths > 0, "nothing died in ninety seconds");
        assert!(result.overkill > 0.0, "no overkill on a dying pack");
    }

    #[test]
    fn an_execute_heuristic_spends_the_burn_before_the_target_dies() {
        // A patient rotation still detonates when the burn would outlive its target, which
        // is the behaviour a real player has and a scripted rotation does not.
        let config = ScenarioConfig {
            target_health: 6_000.0,
            ..quick(1, 60.0)
        };
        let build = Build::new(
            "EXECUTE",
            &[TalentId::ExtendedFlame],
            FireMastery::Crit,
            Style::Crit,
        );
        let result = run_scenario(&config, &build, 3, 5);
        assert!(
            result.counters.detonations > 0,
            "the patient rotation never cashed a burn in on a dying target"
        );
    }

    #[test]
    fn movement_costs_the_area_abilities_some_of_their_damage() {
        let build = designed_haste_build();
        let still = ScenarioConfig {
            wander_chance: 0.0,
            ..quick(8, 60.0)
        };
        let moving = ScenarioConfig {
            wander_chance: 0.9,
            wander_distance: 12.0,
            return_chance: 0.1,
            ..quick(8, 60.0)
        };

        let planted = run_scenario(&still, &build, 3, 21);
        let scattered = run_scenario(&moving, &build, 3, 21);
        assert!(
            scattered.damage < planted.damage,
            "a scattering pack took as much as a stationary one: {} vs {}",
            scattered.damage,
            planted.damage
        );
    }

    #[test]
    fn a_run_is_reproducible_from_its_seed() {
        let build = designed_crit_build();
        let first = run_scenario(&quick(3, 30.0), &build, 2, 99);
        let second = run_scenario(&quick(3, 30.0), &build, 2, 99);
        assert!(
            (first.damage - second.damage).abs() < 1e-6,
            "the seed did not replay"
        );
    }

    #[test]
    fn the_detonate_rules_are_what_separate_the_two_rotations() {
        let build = designed_haste_build();
        let mut encounter = Encounter::new(&quick(1, 40.0), &build, 4);
        let mut patient = RotationAi::with_rules(Style::Haste, DetonateRules::patient());
        let held = encounter.run(&mut patient);

        let mut encounter = Encounter::new(&quick(1, 40.0), &build, 4);
        let mut eager = RotationAi::with_rules(Style::Haste, DetonateRules::eager());
        let spent = encounter.run(&mut eager);

        assert!(spent.counters.detonations > held.counters.detonations);
        assert!(encounter.simulation().time() > 0.0);
    }

    #[test]
    fn every_scenario_the_report_runs_is_configured_and_none_is_a_dummy() {
        for targets in TARGET_COUNTS {
            let config = scenario_for(targets, DURATIONS[0]);
            assert_eq!(config.targets, targets);
            assert!(config.target_health > 0.0, "an infinite-health target");
            assert!(config.duration > 0.0);
        }
        // A configurable duration, not two hardcoded ones.
        let long = quick(1, 300.0);
        assert!((long.duration - 300.0).abs() < f32::EPSILON);
        assert_eq!(long.with_targets(4).targets, 4);
        assert!((long.with_duration(30.0).duration - 30.0).abs() < f32::EPSILON);
        assert_eq!(long.stationary().wander_chance, 0.0);
    }
}
