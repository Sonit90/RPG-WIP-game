//! The combat balance simulator, for all three classes.
//!
//! ```text
//! engine-sim                                  the full matrix, compact
//! engine-sim --class mage --gear high         one class, one gear tier
//! engine-sim --iterations 20 --seed 7 --out docs/balance/sim-report.md
//! engine-sim --search [--budget N]            the FireMage talent combination search
//! engine-sim --loot-sim [--samples N]         how often drops are pre-sharpened (loot.json)
//! engine-sim --gem-sim [--samples N]          runs to a max-level gem (gems.json)
//! ```
//!
//! It drives the game's own [`engine_game::sim::Simulation`]: the same ability table, the
//! same cooldowns, the same damage function, the same stat pipeline. Nothing here
//! re-implements a formula, which is what makes a number it prints a number the game will
//! produce. A fight is a seeded, headless run, so the same seed prints the same report.
//!
//! ```text
//! class.rs    the three classes          gear.rs     gear: real catalog loadouts
//! build.rs    builds, one table          ai/         one rotation per class
//! scenario.rs one fight                  metrics.rs  what a fight is measured by
//! matrix.rs   every combination          report.rs   markdown
//! study.rs    the FireMage talent search (--search)
//! loot_sim.rs the sharpening-drop distribution (--loot-sim)
//! gem_sim.rs  runs to a max-level gem (--gem-sim, section 8)
//! validation.rs  is any talent mandatory or worthless (the report's section 6)
//! scaling.rs  does each gear tier beat the one below it (section 7)
//! ```

mod ai;
mod balance;
mod build;
mod class;
mod gear;
mod gem_sim;
mod loot_sim;
mod matrix;
mod metrics;
mod report;
mod scaling;
mod scenario;
mod study;
#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests;
mod validation;

use std::path::PathBuf;
use std::time::Instant;

use class::Class;
use gear::{GearProfile, GearTier};

/// How the run was configured on the command line.
pub struct Options {
    /// Run the FireMage talent combination search instead of the balance matrix.
    pub search: bool,
    /// Run the loot-sharpening distribution instead of the combat matrix.
    pub loot_sim: bool,
    /// Run the gem economy simulation instead of the combat matrix.
    pub gem_sim: bool,
    /// Items rolled per source and key level by `--loot-sim`.
    pub samples: u32,
    /// Fights averaged per scenario.
    pub iterations: u32,
    /// Fights averaged per scenario during the search sweep.
    pub sweep_iterations: u32,
    /// How many of the sweep's best builds are re-run at full accuracy.
    pub finalists: usize,
    /// Talent points a build may spend.
    pub budget: u32,
    /// Where to write the report, if anywhere.
    pub out: Option<PathBuf>,
    /// The seed every fight is derived from.
    pub seed: u64,
    /// Which classes the matrix covers.
    pub classes: Vec<Class>,
    /// Which gear tiers the matrix covers.
    pub tiers: Vec<GearTier>,
    /// Whether the report ends with the talent validation (a second pass over the talents).
    pub validate: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            search: false,
            loot_sim: false,
            gem_sim: false,
            samples: 100_000,
            iterations: 10,
            sweep_iterations: 8,
            finalists: 12,
            budget: build::default_budget(),
            out: None,
            seed: 0xF12E_5EED,
            classes: Class::ALL.to_vec(),
            tiers: GearTier::ALL.to_vec(),
            validate: true,
        }
    }
}

fn main() {
    // The talent numbers the game ships, from assets/catalog/talents.json: builds read the
    // same tree the client and the server do.
    if let Err(error) = engine_realm::talents::install_builtin() {
        eprintln!("{error}");
        std::process::exit(2);
    }
    let options = match parse_options(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => return,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    let started = Instant::now();
    let text = if options.loot_sim {
        let Some(rules) = engine_realm::loot::builtin_rules() else {
            eprintln!("the shipped loot rules did not build");
            std::process::exit(2);
        };
        let cells = loot_sim::run(rules, options.samples, options.seed);
        loot_sim::report(rules, &cells, options.seed)
    } else if options.gem_sim {
        let (Some(rules), Some(gems)) = (
            engine_realm::loot::builtin_rules(),
            engine_realm::gems::builtin(),
        ) else {
            eprintln!("the shipped gem rules did not build");
            std::process::exit(2);
        };
        let players = options.samples.min(5_000);
        let cells = gem_sim::run(rules, gems, players, options.seed);
        gem_sim::report(rules, gems, &cells, options.seed)
    } else if options.search {
        study::run(&options)
    } else {
        let profiles = GearProfile::select(&options.tiers);
        let matrix = matrix::run(
            &options.classes,
            &profiles,
            options.iterations,
            options.seed,
            &|done, total| {
                if done == total || done % 10 == 0 {
                    eprint!("\r{done}/{total} cells");
                }
            },
        );
        eprintln!();
        let validation = options.validate.then(|| {
            let validation = validation::run(&matrix, &options, &|done, total| {
                if done == total || done % 10 == 0 {
                    eprint!(
                        "
validation {done}/{total} cells"
                    );
                }
            });
            eprintln!();
            validation
        });
        let scaling = scaling::measure(&matrix, &options, &|done, total| {
            if done == total || done % 10 == 0 {
                eprint!("\rscaling {done}/{total} cells");
            }
        });
        eprintln!();
        balance::matrix_report(&matrix, &options, validation.as_ref(), Some(&scaling))
    };

    print!("{text}");
    eprintln!("finished in {:.1} s", started.elapsed().as_secs_f64());
    if let Some(path) = &options.out {
        match std::fs::write(path, &text) {
            Ok(()) => println!("\nwritten to {}", path.display()),
            Err(error) => eprintln!("could not write {}: {error}", path.display()),
        }
    }
}

/// Reads the command line. Deliberately tiny: this is a developer tool, not a product.
///
/// `Ok(None)` means help was printed and there is nothing to run.
fn parse_options(args: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut args = args;
    while let Some(argument) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match argument.as_str() {
            "--search" => options.search = true,
            "--loot-sim" => options.loot_sim = true,
            "--gem-sim" => options.gem_sim = true,
            "--samples" => options.samples = parse_number("--samples", &value("--samples")?)?,
            "--no-validation" => options.validate = false,
            "--iterations" => {
                options.iterations = parse_number("--iterations", &value("--iterations")?)?;
            }
            "--sweep-iterations" => {
                options.sweep_iterations =
                    parse_number("--sweep-iterations", &value("--sweep-iterations")?)?;
            }
            "--finalists" => {
                options.finalists = parse_number("--finalists", &value("--finalists")?)?;
            }
            "--budget" => options.budget = parse_number("--budget", &value("--budget")?)?,
            "--seed" => options.seed = parse_number("--seed", &value("--seed")?)?,
            "--out" => options.out = Some(PathBuf::from(value("--out")?)),
            "--class" => {
                let text = value("--class")?;
                options.classes = if text == "all" {
                    Class::ALL.to_vec()
                } else {
                    vec![Class::parse(&text).ok_or_else(|| {
                        format!("unknown class {text}; expected warrior, druid, mage or all")
                    })?]
                };
            }
            "--gear" => {
                let text = value("--gear")?;
                options.tiers = if text == "all" {
                    GearTier::ALL.to_vec()
                } else {
                    vec![GearTier::parse(&text).ok_or_else(|| {
                        format!("unknown gear {text}; expected none, low, medium, high, p2_max, p2_set, p2_gems (alias top) or all")
                    })?]
                };
            }
            "--help" | "-h" => {
                println!(
                    "engine-sim [--class warrior|druid|mage|all] [--gear none|low|medium|high|p2_max|p2_set|p2_gems|all]\n\
                     \x20          [--iterations N] [--seed N] [--out PATH] [--no-validation]\n\
                     engine-sim --search [--budget N] [--sweep-iterations N] [--finalists N]\n\
                     \x20          [--iterations N] [--seed N] [--out PATH]
\n                     engine-sim --loot-sim [--samples N] [--seed N] [--out PATH]"
                );
                return Ok(None);
            }
            other => return Err(format!("unknown argument {other}; try --help")),
        }
    }
    Ok(Some(options))
}

/// Parses a numeric option, naming it in the error.
fn parse_number<T: std::str::FromStr>(name: &str, text: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("{name} expects a number, got {text}"))
}
