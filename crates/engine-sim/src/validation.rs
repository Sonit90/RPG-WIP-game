//! Talent validation: is any talent mathematically mandatory, or worth nothing?
//!
//! ```text
//! per class, per gear tier
//!   Default / Build A / Build B       single target (a boss that dies) and AoE 8
//!   best build = the best of A and B  by single-target DPS
//!   every talent, alone on Default    "solo"      what it adds to nothing
//!   every talent of the best build,
//!   removed from it                   "removal"   what the build loses without it
//! ```
//!
//! # Why the single-target fight ends
//!
//! The matrix's targets hold five million health so nothing dies and DPS is sustained output.
//! That hides every talent that pays on a *dying* target (the Warrior's Executioner is the
//! example: it does nothing above 35 % health). So the single-target validation fight uses a
//! boss sized to die after about [`KILL_SECONDS`] of the class's own `Default` build at that
//! gear, and DPS is the damage that landed on it (overkill excluded) over the time it took to
//! die. The AoE 8 fight is the matrix's own and does not end.
//!
//! # The verdicts
//!
//! * **mandatory-ish** - removing the talent from the best build costs more than
//!   [`MANDATORY`] of its DPS in some fight and tier: the build is not a choice without it.
//! * **no damage value - check defensive/utility value** - it never adds [`NO_VALUE`] of DPS,
//!   alone or in the best build, in any fight or tier. Not a verdict that it is useless: a
//!   defensive talent is meant to say this. The survival proxy beside it says whether it does
//!   anything for survival; when it does not either, nothing the simulator measures sees the
//!   talent (a heal, a utility effect) and it has to be judged by hand.
//!
//! The survival proxy is the average share of a physical hit the player is not taking
//! (avoidance, block, armour, flat reductions and damage-taken effects such as Battle Fervor).
//! The dummies never attack, so it is read from the player's own defences, not from damage
//! taken.

use std::fmt::Write as _;

use engine_game::talent::hero_talents;

use crate::Options;
use crate::build::{ClassBuild, builds_for};
use crate::class::Class;
use crate::gear::{GearProfile, GearTier};
use crate::matrix::{self, AOE8, Job, Matrix, SCENARIOS, ST60};
use crate::scenario::{RunResult, ScenarioConfig};

/// A talent whose removal costs more than this share of a build's DPS is mandatory-ish.
pub const MANDATORY: f64 = 0.15;

/// A talent that adds less than this share of DPS has no damage value.
pub const NO_VALUE: f64 = 0.01;

/// A talent that moves the survival proxy by less than this (a fraction, so 0.005 is half a
/// percentage point) has no survival value.
pub const SURVIVAL_VALUE: f64 = 0.005;

/// How long the `Default` build takes to kill the validation boss, in seconds.
pub const KILL_SECONDS: f32 = 60.0;

/// The longest the validation boss fight may run, in seconds.
const KILL_LIMIT: f32 = 180.0;

/// What one fight measured.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measure {
    /// Damage that landed on health, per second of the fight.
    pub dps: f64,
    /// The survival proxy, `0.0..=1.0`.
    pub mitigation: f64,
}

impl Measure {
    fn of(result: &RunResult) -> Self {
        let seconds = result.seconds.max(1e-9);
        Self {
            dps: (result.damage - result.overkill).max(0.0) / seconds,
            mitigation: result.metrics.average_mitigation(),
        }
    }
}

/// A build's two measurements at one tier.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pair {
    /// The single-target boss that dies.
    pub single: Measure,
    /// Eight targets that do not.
    pub area: Measure,
}

/// A change in DPS, as a fraction of the thing it is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Delta {
    /// Single target.
    pub single: f64,
    /// Eight targets.
    pub area: f64,
}

/// One build's measurements across the tiers.
#[derive(Debug, Clone)]
pub struct BuildRow {
    /// Its name in the report.
    pub name: String,
    /// Its talents, for the table.
    pub talents: String,
    /// One pair per tier.
    pub tiers: Vec<Pair>,
}

/// One talent's validation.
#[derive(Debug, Clone)]
pub struct TalentRow {
    /// Uppercase name.
    pub name: String,
    /// Talent points.
    pub cost: u32,
    /// Whether every non-`Default` build of the class buys it.
    pub in_every_build: bool,
    /// What it adds to `Default`, alone, per tier.
    pub solo: Vec<Delta>,
    /// What the best build loses without it, per tier, if the best build buys it.
    pub removal: Vec<Option<Delta>>,
    /// Survival proxy of `Default` plus this talent minus `Default`'s, per tier.
    pub survival: Vec<f64>,
}

/// A talent's verdict, from its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    /// Removing it costs more than [`MANDATORY`] somewhere.
    pub mandatory: bool,
    /// It never adds [`NO_VALUE`] of damage.
    pub no_damage_value: bool,
    /// It moves the survival proxy by [`SURVIVAL_VALUE`] somewhere.
    pub survival_value: bool,
}

impl TalentRow {
    /// What the numbers say.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        let removal = self.removal.iter().flatten();
        let mandatory = removal
            .clone()
            .any(|delta| delta.single > MANDATORY || delta.area > MANDATORY);
        let best = self
            .solo
            .iter()
            .chain(removal)
            .flat_map(|delta| [delta.single, delta.area])
            .fold(f64::MIN, f64::max);
        let survival = self.survival.iter().copied().fold(f64::MIN, f64::max);
        Verdict {
            mandatory,
            no_damage_value: best < NO_VALUE,
            survival_value: survival >= SURVIVAL_VALUE,
        }
    }
}

/// One class's validation.
#[derive(Debug, Clone)]
pub struct ClassValidation {
    /// Which class.
    pub class: Class,
    /// The gear tiers measured.
    pub tiers: Vec<GearTier>,
    /// The boss's health at each tier.
    pub health: Vec<f32>,
    /// `Default`, then the designed builds.
    pub builds: Vec<BuildRow>,
    /// The best non-`Default` build at each tier, by single-target DPS.
    pub best: Vec<String>,
    /// Every talent.
    pub talents: Vec<TalentRow>,
}

/// Every class's validation.
#[derive(Debug, Clone, Default)]
pub struct Validation {
    /// The classes measured.
    pub classes: Vec<ClassValidation>,
}

/// The single-target validation fight for `class` against a boss of `health`.
fn single_target(class: Class, health: f32) -> ScenarioConfig {
    ScenarioConfig {
        targets: 1,
        target_health: health,
        duration: KILL_LIMIT,
        distance: class.engage_distance(),
        ..ScenarioConfig::default()
    }
}

/// Runs the validation for the classes and tiers the matrix covers.
#[must_use]
pub fn run(
    matrix: &Matrix,
    options: &Options,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Validation {
    let mut validation = Validation::default();
    for class in &options.classes {
        if let Some(measured) = run_class(matrix, options, *class, progress) {
            validation.classes.push(measured);
        }
    }
    validation
}

fn run_class(
    matrix: &Matrix,
    options: &Options,
    class: Class,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Option<ClassValidation> {
    let tiers = options.tiers.clone();
    let profiles: Vec<GearProfile> = tiers.iter().map(|t| GearProfile::headline(*t)).collect();
    let builds = builds_for(class);
    let default = builds.first()?.clone();

    // The boss dies after about a minute of the Default build, at each tier.
    let health: Vec<f32> = profiles
        .iter()
        .map(|profile| {
            matrix
                .find(class, default.name(), profile.variant, ST60)
                .map_or(60_000.0, |cell| (cell.result.dps as f32) * KILL_SECONDS)
        })
        .collect();

    // Phase one: every designed build, single target. Their area numbers are the matrix's.
    let mut jobs = Vec::new();
    for build in &builds {
        for (profile, health) in profiles.iter().zip(&health) {
            jobs.push(Job {
                build: build.clone(),
                profile: *profile,
                config: single_target(class, *health),
            });
        }
    }
    let singles = matrix::execute(&jobs, options.iterations, options.seed, progress);
    let tier_count = tiers.len();
    let area_of = |build: &ClassBuild, profile: &GearProfile| {
        matrix
            .find(class, build.name(), profile.variant, AOE8)
            .map(|cell| Measure::of(&cell.result))
            .unwrap_or_default()
    };
    let rows: Vec<BuildRow> = builds
        .iter()
        .enumerate()
        .map(|(row, build)| BuildRow {
            name: build.name().to_owned(),
            talents: build.talent_names(),
            tiers: (0..tier_count)
                .map(|tier| Pair {
                    single: Measure::of(&singles[row * tier_count + tier]),
                    area: area_of(build, &profiles[tier]),
                })
                .collect(),
        })
        .collect();

    // The best designed build at each tier.
    let best: Vec<usize> = (0..tier_count)
        .map(|tier| {
            (1..builds.len())
                .max_by(|a, b| {
                    rows[*a].tiers[tier]
                        .single
                        .dps
                        .total_cmp(&rows[*b].tiers[tier].single.dps)
                })
                .unwrap_or(0)
        })
        .collect();

    // Phase two: every talent alone on Default, and every talent of the best build removed.
    let table = &hero_talents(class.hero()).talents;
    let mut jobs = Vec::new();
    // (talent, tier, kind) in job order, so the results can be read back.
    let mut keys: Vec<(usize, usize, bool, bool)> = Vec::new();
    for (talent, row) in table.iter().enumerate() {
        let solo = default.with_talent_indices(&row.name, &[talent]);
        for (tier, profile) in profiles.iter().enumerate() {
            for (area, config) in [
                (false, single_target(class, health[tier])),
                (true, SCENARIOS[AOE8].config(class)),
            ] {
                jobs.push(Job {
                    build: solo.clone(),
                    profile: *profile,
                    config,
                });
                keys.push((talent, tier, true, area));
            }
            let best_build = &builds[best[tier]];
            let owned = best_build.talent_indices();
            if owned.contains(&talent) {
                let without: Vec<usize> = owned.iter().copied().filter(|t| *t != talent).collect();
                let reduced = best_build.with_talent_indices("without", &without);
                for (area, config) in [
                    (false, single_target(class, health[tier])),
                    (true, SCENARIOS[AOE8].config(class)),
                ] {
                    jobs.push(Job {
                        build: reduced.clone(),
                        profile: *profile,
                        config,
                    });
                    keys.push((talent, tier, false, area));
                }
            }
        }
    }
    let results = matrix::execute(&jobs, options.iterations, options.seed, progress);

    let measured = |talent: usize, tier: usize, solo: bool| -> Option<Pair> {
        let find = |area: bool| {
            keys.iter()
                .position(|key| *key == (talent, tier, solo, area))
                .map(|index| Measure::of(&results[index]))
        };
        Some(Pair {
            single: find(false)?,
            area: find(true)?,
        })
    };
    let base = |tier: usize| rows[0].tiers[tier];
    let change = |value: f64, against: f64| {
        if against > 0.0 {
            value / against - 1.0
        } else {
            0.0
        }
    };

    let non_default: Vec<Vec<usize>> = builds[1..].iter().map(ClassBuild::talent_indices).collect();
    let talents = (0..table.len())
        .map(|talent| {
            let solo: Vec<Pair> = (0..tier_count)
                .map(|tier| measured(talent, tier, true).unwrap_or_default())
                .collect();
            TalentRow {
                name: table[talent].name.clone(),
                cost: table[talent].cost,
                in_every_build: !non_default.is_empty()
                    && non_default.iter().all(|owned| owned.contains(&talent)),
                solo: solo
                    .iter()
                    .enumerate()
                    .map(|(tier, pair)| Delta {
                        single: change(pair.single.dps, base(tier).single.dps),
                        area: change(pair.area.dps, base(tier).area.dps),
                    })
                    .collect(),
                removal: (0..tier_count)
                    .map(|tier| {
                        let without = measured(talent, tier, false)?;
                        let full = rows[best[tier]].tiers[tier];
                        Some(Delta {
                            single: change(full.single.dps, without.single.dps),
                            area: change(full.area.dps, without.area.dps),
                        })
                    })
                    .collect(),
                survival: solo
                    .iter()
                    .enumerate()
                    .map(|(tier, pair)| pair.single.mitigation - base(tier).single.mitigation)
                    .collect(),
            }
        })
        .collect();

    Some(ClassValidation {
        class,
        tiers,
        health,
        best: best.iter().map(|b| builds[*b].name().to_owned()).collect(),
        builds: rows,
        talents,
    })
}

// ------------------------------------------------------------------ the report

fn percent(value: f64) -> String {
    format!("{:+.1}%", value * 100.0)
}

fn tier_name(tier: GearTier) -> &'static str {
    tier.as_str()
}

/// The "Talent validation" section.
pub fn write_section(out: &mut String, validation: &Validation) {
    let _ = writeln!(out, "## 6. Talent validation\n");
    if validation.classes.is_empty() {
        let _ = writeln!(out, "Not run.\n");
        return;
    }
    let _ = writeln!(
        out,
        "Is any talent mandatory, or worth nothing? For every class and gear tier (the \
         headline profile of each: none, medium/balanced, top) the `Default`, `Build A` and \
         `Build B` builds are measured, then every talent **alone on `Default`** (*solo*) and \
         every talent of the **best build removed from it** (*removal*, \"best\" being the \
         higher single-target DPS of A and B at that tier).\n"
    );
    let _ = writeln!(
        out,
        "* **Single target** is a boss sized to die after about {KILL_SECONDS:.0} s of the \
         class's `Default` build at that gear, so talents that pay on a dying target \
         (Executioner) are seen; DPS is damage that landed over the time to kill. **AoE 8** is \
         the matrix's eight non-dying targets.\n\
         * **Solo** = `Default + talent` over `Default`. **Removal** = best build over best \
         build without the talent (so +20% means the build loses a sixth of its damage without \
         it). Costs are not considered: the numbers say what a talent does, not whether it is \
         worth its points.\n\
         * **mandatory-ish**: removal costs more than {:.0}% in some fight and tier. **no damage \
         value**: the talent never adds {:.0}% DPS, alone or in the best build, in any fight or \
         tier - not a verdict on a defensive or utility talent, so the **survival proxy** \
         (average share of a physical hit not taken: avoidance, block, armour, reductions, \
         damage-taken effects, read off the player's own defences because the dummies never \
         attack) is shown beside it. Healing and other utility effects are invisible to both \
         numbers.\n",
        MANDATORY * 100.0,
        NO_VALUE * 100.0
    );
    for class in &validation.classes {
        class_section(out, class);
    }
    findings(out, validation);
}

fn class_section(out: &mut String, class: &ClassValidation) {
    let _ = writeln!(out, "### {}\n", class.class.title());
    let _ = write!(out, "| Build |");
    for tier in &class.tiers {
        let _ = write!(out, " {} ST / AoE 8 |", tier_name(*tier));
    }
    let _ = writeln!(out, " Talents |");
    let _ = write!(out, "|---|");
    for _ in &class.tiers {
        let _ = write!(out, "---:|");
    }
    let _ = writeln!(out, "---|");
    for build in &class.builds {
        let _ = write!(out, "| {} |", build.name);
        for pair in &build.tiers {
            let _ = write!(out, " {:.0} / {:.0} |", pair.single.dps, pair.area.dps);
        }
        let _ = writeln!(out, " {} |", build.talents);
    }
    let _ = writeln!(
        out,
        "\nBest build per tier: {}. Boss health: {}.\n",
        class
            .tiers
            .iter()
            .zip(&class.best)
            .map(|(tier, best)| format!("{} = {best}", tier_name(*tier)))
            .collect::<Vec<_>>()
            .join(", "),
        class
            .tiers
            .iter()
            .zip(&class.health)
            .map(|(tier, health)| format!("{} {health:.0}", tier_name(*tier)))
            .collect::<Vec<_>>()
            .join(", "),
    );

    let _ = write!(out, "| Talent (cost) |");
    for tier in &class.tiers {
        let _ = write!(out, " Solo {} ST / AoE |", tier_name(*tier));
    }
    for tier in &class.tiers {
        let _ = write!(out, " Removal {} ST / AoE |", tier_name(*tier));
    }
    let _ = writeln!(out, " Survival (top) | Verdict |");
    let _ = write!(out, "|---|");
    for _ in 0..class.tiers.len() * 2 {
        let _ = write!(out, "---:|");
    }
    let _ = writeln!(out, "---:|---|");
    for talent in &class.talents {
        let _ = write!(out, "| {} ({}) |", talent.name, talent.cost);
        for delta in &talent.solo {
            let _ = write!(
                out,
                " {} / {} |",
                percent(delta.single),
                percent(delta.area)
            );
        }
        for delta in &talent.removal {
            match delta {
                Some(delta) => {
                    let _ = write!(
                        out,
                        " {} / {} |",
                        percent(delta.single),
                        percent(delta.area)
                    );
                }
                None => {
                    let _ = write!(out, " - |");
                }
            }
        }
        let survival = talent.survival.last().copied().unwrap_or(0.0);
        let _ = writeln!(
            out,
            " {:+.1} pp | {} |",
            survival * 100.0,
            verdict_text(talent)
        );
    }
    let _ = writeln!(out);
}

fn verdict_text(talent: &TalentRow) -> String {
    let verdict = talent.verdict();
    let mut parts = Vec::new();
    if verdict.mandatory {
        parts.push("mandatory-ish".to_owned());
    }
    if talent.in_every_build {
        parts.push("in every build".to_owned());
    }
    if verdict.no_damage_value {
        parts.push(if verdict.survival_value {
            "no damage value - check defensive/utility value (survival proxy moves)".to_owned()
        } else {
            "no damage value - check defensive/utility value (survival proxy flat too)".to_owned()
        });
    }
    if parts.is_empty() {
        "ok".to_owned()
    } else {
        parts.join("; ")
    }
}

fn findings(out: &mut String, validation: &Validation) {
    let _ = writeln!(out, "### Findings\n");
    let mut any = false;
    for class in &validation.classes {
        for talent in &class.talents {
            let verdict = talent.verdict();
            if verdict.mandatory {
                any = true;
                let _ = writeln!(
                    out,
                    "* {} / {}: **mandatory-ish** (removal costs more than {:.0}%).",
                    class.class.title(),
                    talent.name,
                    MANDATORY * 100.0
                );
            }
            if verdict.no_damage_value {
                any = true;
                let _ = writeln!(
                    out,
                    "* {} / {}: **no damage value** - check its defensive/utility value{}.",
                    class.class.title(),
                    talent.name,
                    if verdict.survival_value {
                        " (the survival proxy moves)"
                    } else {
                        " by hand (the survival proxy is flat too, so nothing the simulator \
                         measures sees it)"
                    }
                );
            }
            if talent.in_every_build {
                any = true;
                let worst = talent
                    .removal
                    .iter()
                    .flatten()
                    .flat_map(|delta| [delta.single, delta.area])
                    .fold(f64::MIN, f64::max);
                let _ = writeln!(
                    out,
                    "* {} / {}: bought by every designed build; removing it from the best \
                     build costs at most {} of its DPS{}.",
                    class.class.title(),
                    talent.name,
                    percent(worst.max(0.0)),
                    if verdict.mandatory {
                        " - mandatory-ish"
                    } else {
                        ", so it is a habit and not a requirement"
                    }
                );
            }
        }
    }
    if !any {
        let _ = writeln!(
            out,
            "No talent is mandatory-ish and none lacks damage value."
        );
    }
    let _ = writeln!(out);
}
