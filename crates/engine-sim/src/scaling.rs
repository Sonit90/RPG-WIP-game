//! Gear scaling: does each rung of the gear ladder make a character strictly stronger?
//!
//! ```text
//! matrix (Default build, single target, 60 s)  -> DPS, survivability
//! one kill fight per class and tier            -> encounter duration
//!     -> Scaling --violations()--> the tiers that are not better than the one below
//! ```
//!
//! This is the guard P2 relies on. Sharpening, gems, sets and books all raise a character's
//! stats through the sheet; if any of them (or a retune) makes a higher tier *not* stronger
//! than a lower one, or makes the top of the ladder run away, the table here says so before a
//! player does. The three metrics:
//!
//! * **DPS** - the matrix's single-target 60 s figure for the class's `Default` build.
//! * **Survivability** - effective health: the player's maximum health (Stamina through the
//!   game's curve) divided by the share of a physical hit that still lands, which is the
//!   matrix's own survival proxy. Higher is better.
//! * **Encounter duration** - seconds to kill a boss sized to die after [`KILL_SECONDS`] of the
//!   class's *ungeared* `Default` build, the same fixed boss at every tier. Lower is better.
//!
//! A tier must beat the one below it on all three. DPS is the headline (and the test); the
//! other two are reported the same way, because a tier that kills slower or dies faster is as
//! wrong as one that deals less.

use std::fmt::Write as _;

use crate::Options;
use crate::build::builds_for;
use crate::class::Class;
use crate::gear::{GearProfile, GearTier};
use crate::matrix::{self, Job, Matrix, ST60};
use crate::scenario::ScenarioConfig;

/// Seconds the ungeared character takes to kill the scaling boss.
pub const KILL_SECONDS: f32 = 60.0;

/// The longest a kill fight may run, in seconds. A boss still alive is reported as this.
pub const KILL_LIMIT: f32 = 600.0;

/// One tier's measurements for one class.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierMeasure {
    /// Which tier.
    pub tier: GearTier,
    /// Single-target damage per second.
    pub dps: f64,
    /// Effective health: maximum health over the share of a hit that lands.
    pub survivability: f64,
    /// Seconds to kill the scaling boss.
    pub duration: f64,
}

/// One class's ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassScaling {
    /// Which class.
    pub class: Class,
    /// The boss's health, the same at every tier.
    pub boss_health: f32,
    /// The tiers measured, lowest first.
    pub tiers: Vec<TierMeasure>,
}

/// Which measure a [`Violation`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Damage per second. Higher is better.
    Dps,
    /// Effective health. Higher is better.
    Survivability,
    /// Seconds to kill the boss. Lower is better.
    Duration,
}

impl Metric {
    /// The name the report uses.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Dps => "DPS",
            Self::Survivability => "survivability",
            Self::Duration => "encounter duration",
        }
    }
}

/// A tier that is not better than the one below it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Violation {
    /// Which class.
    pub class: Class,
    /// Which measure.
    pub metric: Metric,
    /// The lower tier.
    pub lower: GearTier,
    /// The tier above it, which should have been better.
    pub upper: GearTier,
    /// The lower tier's value.
    pub before: f64,
    /// The upper tier's value.
    pub after: f64,
}

/// Every class's ladder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scaling {
    /// The classes measured.
    pub classes: Vec<ClassScaling>,
}

impl TierMeasure {
    fn value(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Dps => self.dps,
            Metric::Survivability => self.survivability,
            Metric::Duration => self.duration,
        }
    }
}

impl Scaling {
    /// Every place a tier is not strictly better than the one below it, in report order.
    ///
    /// A tier is compared with the previous one *measured*, so a run of a few tiers is still
    /// checked. `metrics` picks which measures count.
    #[must_use]
    pub fn violations_of(&self, metrics: &[Metric]) -> Vec<Violation> {
        let mut out = Vec::new();
        for class in &self.classes {
            for metric in metrics {
                for pair in class.tiers.windows(2) {
                    let (before, after) = (pair[0].value(*metric), pair[1].value(*metric));
                    let better = match metric {
                        Metric::Duration => after < before,
                        _ => after > before,
                    };
                    if !better {
                        out.push(Violation {
                            class: class.class,
                            metric: *metric,
                            lower: pair[0].tier,
                            upper: pair[1].tier,
                            before,
                            after,
                        });
                    }
                }
            }
        }
        out
    }

    /// Every violation of all three metrics.
    #[must_use]
    pub fn violations(&self) -> Vec<Violation> {
        self.violations_of(&[Metric::Dps, Metric::Survivability, Metric::Duration])
    }
}

/// One tier of a class as read from the matrix: profile, DPS, survivability.
type Cell = (GearProfile, f64, f64);

/// Measures the ladder for the classes and tiers of `matrix`, using its `Default` builds.
///
/// DPS and survivability are read from the matrix; the kill fights are the only extra runs.
#[must_use]
pub fn measure(
    matrix: &Matrix,
    options: &Options,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Scaling {
    let mut jobs = Vec::new();
    // (class, boss health, tier cells) in job order.
    let mut rows: Vec<(Class, f32, Vec<Cell>)> = Vec::new();
    for class in &options.classes {
        let Some(default) = builds_for(*class).into_iter().next() else {
            continue;
        };
        let mut tiers = Vec::new();
        for tier in GearTier::ALL {
            let profile = GearProfile::headline(tier);
            let Some(cell) = matrix.find(*class, default.name(), profile.variant, ST60) else {
                continue;
            };
            let result = &cell.result;
            let landing = (1.0 - result.metrics.average_mitigation()).max(1e-3);
            tiers.push((profile, result.dps, result.max_health / landing));
        }
        let Some(&(_, base_dps, _)) = tiers.first() else {
            continue;
        };
        let health = (base_dps as f32 * KILL_SECONDS).max(1.0);
        for (profile, _, _) in &tiers {
            jobs.push(Job {
                build: default.clone(),
                profile: *profile,
                config: ScenarioConfig {
                    targets: 1,
                    target_health: health,
                    duration: KILL_LIMIT,
                    distance: class.engage_distance(),
                    ..ScenarioConfig::default()
                },
            });
        }
        rows.push((*class, health, tiers));
    }

    let results = matrix::execute(&jobs, options.iterations, options.seed, progress);
    let mut results = results.into_iter();
    let classes = rows
        .into_iter()
        .map(|(class, boss_health, tiers)| ClassScaling {
            class,
            boss_health,
            tiers: tiers
                .into_iter()
                .map(|(profile, dps, survivability)| TierMeasure {
                    tier: profile.tier,
                    dps,
                    survivability,
                    duration: results.next().map_or(f64::from(KILL_LIMIT), |r| r.seconds),
                })
                .collect(),
        })
        .collect();
    Scaling { classes }
}

/// The "Gear scaling" section.
pub fn write_section(out: &mut String, scaling: &Scaling) {
    let _ = writeln!(out, "## 7. Gear scaling\n");
    if scaling.classes.is_empty() {
        let _ = writeln!(out, "Not run.\n");
        return;
    }
    let _ = writeln!(
        out,
        "Does each gear tier beat the one below it? Per class, the `Default` build wearing real \
         catalog items (see section 5): **DPS** is the single-target 60 s figure, \
         **survivability** is effective health (maximum health over the share of a physical hit \
         that lands), and **kill time** is how long a boss sized to die after \
         {KILL_SECONDS:.0} s of the ungeared build takes to die (capped at {KILL_LIMIT:.0} s). \
         The required order is `none < low < medium < high < p2_max < p2_set` in DPS and survivability, \
         and the reverse in kill time.\n"
    );
    for class in &scaling.classes {
        let _ = writeln!(
            out,
            "**{}** (boss: {:.0} health)\n",
            class.class.title(),
            class.boss_health
        );
        let _ = writeln!(
            out,
            "| Tier | DPS | vs none | Survivability | vs none | Kill time (s) | vs none |"
        );
        let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
        let base = class.tiers.first().copied();
        for tier in &class.tiers {
            let relative = |value: f64, against: Option<f64>| {
                against.filter(|b| *b > 0.0).map_or(String::new(), |b| {
                    format!("{:+.1}%", (value / b - 1.0) * 100.0)
                })
            };
            let _ = writeln!(
                out,
                "| {} | {:.0} | {} | {:.0} | {} | {:.1} | {} |",
                tier.tier.as_str(),
                tier.dps,
                relative(tier.dps, base.map(|b| b.dps)),
                tier.survivability,
                relative(tier.survivability, base.map(|b| b.survivability)),
                tier.duration,
                relative(tier.duration, base.map(|b| b.duration)),
            );
        }
        let _ = writeln!(out);
    }

    let violations = scaling.violations();
    if violations.is_empty() {
        let _ = writeln!(
            out,
            "**Monotonicity check: passed.** Every tier beats the one below it in DPS, \
             survivability and kill time, for every class.\n"
        );
        return;
    }
    let _ = writeln!(
        out,
        "**Monotonicity check: {} violation(s).** A tier that is not better than the one \
         below it:\n",
        violations.len()
    );
    for violation in violations {
        let _ = writeln!(
            out,
            "* **{}** - {}: `{}` ({:.1}) is not better than `{}` ({:.1})",
            violation.class.title(),
            violation.metric.name(),
            violation.upper.as_str(),
            violation.after,
            violation.lower.as_str(),
            violation.before,
        );
    }
    let _ = writeln!(out);
}
