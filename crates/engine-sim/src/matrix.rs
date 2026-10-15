//! The balance matrix: every class, build, gear profile and scenario, run and collected.
//!
//! ```text
//! classes x builds_for(class) x GearProfile x SCENARIOS  ->  Vec<Cell>
//! ```
//!
//! Each cell is independent and deterministic - it builds its own [`Simulation`] from a seed
//! derived from the run seed alone - so the cells are spread across threads and collected in
//! table order. The output is identical whatever the thread count, which the determinism
//! test holds.
//!
//! [`Simulation`]: engine_game::sim::Simulation

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::build::{ClassBuild, builds_for};
use crate::class::Class;
use crate::gear::GearProfile;
use crate::scenario::{RunResult, ScenarioConfig, run_scenario};

/// Health of every target in a matrix scenario.
///
/// High enough that nothing dies inside the longest fight: a boss that is expected to survive,
/// and a pack that does too, so the DPS measured is the class's sustained output and not how
/// early it ran out of things to hit. The best top-geared run does not reach a tenth of it.
pub const TARGET_HEALTH: f32 = 5_000_000.0;

/// One fight setup of the matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScenarioSpec {
    /// A short stable id for the report.
    pub id: &'static str,
    /// What the report calls it.
    pub label: &'static str,
    /// How many enemies stand in the pack.
    pub targets: usize,
    /// How long the fight runs, in seconds.
    pub duration: f32,
}

/// **The scenarios**: a boss for a minute and for three, and a pack of three and of eight.
pub const SCENARIOS: [ScenarioSpec; 4] = [
    ScenarioSpec {
        id: "st60",
        label: "Single target, 60 s",
        targets: 1,
        duration: 60.0,
    },
    ScenarioSpec {
        id: "st180",
        label: "Single target, 180 s",
        targets: 1,
        duration: 180.0,
    },
    ScenarioSpec {
        id: "aoe3",
        label: "AoE, 3 targets, 60 s",
        targets: 3,
        duration: 60.0,
    },
    ScenarioSpec {
        id: "aoe8",
        label: "AoE, 8 targets, 60 s",
        targets: 8,
        duration: 60.0,
    },
];

/// Index of the single-target 60 s scenario in [`SCENARIOS`].
pub const ST60: usize = 0;
/// Index of the single-target 180 s scenario.
pub const ST180: usize = 1;
/// Index of the three-target scenario.
pub const AOE3: usize = 2;
/// Index of the eight-target scenario.
pub const AOE8: usize = 3;

impl ScenarioSpec {
    /// The fight configuration for `class`, which decides how far from the pack it starts.
    #[must_use]
    pub fn config(&self, class: Class) -> ScenarioConfig {
        ScenarioConfig {
            targets: self.targets,
            target_health: TARGET_HEALTH,
            duration: self.duration,
            distance: class.engage_distance(),
            ..ScenarioConfig::default()
        }
    }
}

/// One measured cell of the matrix.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The build.
    pub build: ClassBuild,
    /// The gear.
    pub profile: GearProfile,
    /// Index into [`SCENARIOS`].
    pub scenario: usize,
    /// The average of the iterations.
    pub result: RunResult,
}

/// Every cell, in table order.
#[derive(Debug, Clone, Default)]
pub struct Matrix {
    /// The cells: class, then build, then gear profile, then scenario.
    pub cells: Vec<Cell>,
}

impl Matrix {
    /// The cell for a build name, profile variant and scenario, if it was run.
    #[must_use]
    pub fn find(&self, class: Class, build: &str, variant: &str, scenario: usize) -> Option<&Cell> {
        self.cells.iter().find(|cell| {
            cell.build.class() == class
                && cell.build.name() == build
                && cell.profile.variant == variant
                && cell.scenario == scenario
        })
    }

    /// Builds of `class` that were run, in table order and without repeats.
    #[must_use]
    pub fn builds(&self, class: Class) -> Vec<ClassBuild> {
        let mut out: Vec<ClassBuild> = Vec::new();
        for cell in self.cells.iter().filter(|cell| cell.build.class() == class) {
            if !out.iter().any(|build| build.name() == cell.build.name()) {
                out.push(cell.build.clone());
            }
        }
        out
    }

    /// Profiles that were run, in table order and without repeats.
    #[must_use]
    pub fn profiles(&self) -> Vec<GearProfile> {
        let mut out: Vec<GearProfile> = Vec::new();
        for cell in &self.cells {
            if !out
                .iter()
                .any(|profile| profile.variant == cell.profile.variant)
            {
                out.push(cell.profile);
            }
        }
        out
    }
}

/// One unit of work: a build in some gear against a configured fight.
#[derive(Debug, Clone)]
pub struct Job {
    /// The build.
    pub build: ClassBuild,
    /// The gear.
    pub profile: GearProfile,
    /// The fight.
    pub config: ScenarioConfig,
}

/// Runs `jobs` across the machine's threads and returns each result in job order.
///
/// Every job is independent and seeded from `seed` alone, so the answer does not depend on
/// how the work was scheduled. `progress` is called with (done, total) as jobs finish, from
/// whichever thread finished them; it is for a status line and must not be needed for
/// correctness.
#[must_use]
pub fn execute(
    jobs: &[Job],
    iterations: u32,
    seed: u64,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Vec<RunResult> {
    let total = jobs.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<RunResult>>> = Mutex::new(vec![None; total]);
    let workers = std::thread::available_parallelism().map_or(1, usize::from);

    std::thread::scope(|scope| {
        for _ in 0..workers.min(total.max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(index) else { break };
                    let gear = job.profile.stat_block(job.build.class().hero());
                    let result = run_scenario(&job.config, &job.build, &gear, iterations, seed);
                    if let Ok(mut slots) = results.lock() {
                        slots[index] = Some(result);
                    }
                    progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                }
            });
        }
    });

    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

/// Runs the matrix for `classes` and `profiles`.
///
/// `progress` is called with (done, total) as cells finish; see [`execute`].
#[must_use]
pub fn run(
    classes: &[Class],
    profiles: &[GearProfile],
    iterations: u32,
    seed: u64,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Matrix {
    let mut jobs = Vec::new();
    let mut cells = Vec::new();
    for class in classes {
        for build in builds_for(*class) {
            for profile in profiles {
                for (scenario, spec) in SCENARIOS.iter().enumerate() {
                    jobs.push(Job {
                        build: build.clone(),
                        profile: *profile,
                        config: spec.config(*class),
                    });
                    cells.push((build.clone(), *profile, scenario));
                }
            }
        }
    }

    let results = execute(&jobs, iterations, seed, progress);
    let cells = cells
        .into_iter()
        .zip(results)
        .map(|((build, profile, scenario), result)| Cell {
            build,
            profile,
            scenario,
            result,
        })
        .collect();
    Matrix { cells }
}
