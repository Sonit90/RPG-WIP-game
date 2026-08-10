//! Turning runs into something a designer can read.
//!
//! One block per scenario in the design document's order, then a ranking across the four
//! target counts, then the Haste versus Crit comparison against the balance targets.

use std::fmt::Write as _;

use engine_game::ability::AbilityId;
use engine_game::firemage::FireAbility;

use crate::build::Build;
use crate::scenario::{RunResult, ScenarioConfig};

/// One scenario's result, with everything needed to print it.
#[derive(Debug, Clone)]
pub struct ScenarioReport {
    /// Which build produced it.
    pub build: Build,
    /// How the fight was set up.
    pub config: ScenarioConfig,
    /// How many fights were averaged.
    pub iterations: u32,
    /// The average of those fights.
    pub result: RunResult,
}

impl ScenarioReport {
    /// The full block the design document's §33 asks for.
    #[must_use]
    pub fn detail(&self) -> String {
        let mut out = String::with_capacity(2_048);
        let result = &self.result;
        let counters = &result.counters;

        let _ = writeln!(
            out,
            "TARGETS {}   DURATION {:.0} S   HP {:.0}   ITERATIONS {}",
            self.config.targets, self.config.duration, self.config.target_health, self.iterations
        );
        let _ = writeln!(out, "BUILD    {}", self.build.name);
        let _ = writeln!(out, "         {}", self.build.label());
        let _ = writeln!(out, "TALENTS  {}", self.build.talent_names());
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "TOTAL DAMAGE     {:>12.0}   DPS {:>10.0}   PER TARGET {:>10.0}",
            result.damage,
            result.dps,
            result.damage / self.config.targets.max(1) as f64
        );
        let _ = writeln!(out);

        for ability in [
            FireAbility::Firebolt,
            FireAbility::FlameLance,
            FireAbility::FlameCone,
            FireAbility::Meteor,
            FireAbility::Pyroblast,
            FireAbility::Firestorm,
            FireAbility::Ignite,
            FireAbility::Detonate,
        ] {
            let damage = result.by_ability[AbilityId::from_fire(ability).slot()];
            let share = if result.damage > 0.0 {
                damage / result.damage * 100.0
            } else {
                0.0
            };
            let _ = writeln!(
                out,
                "  {:<14} {:>12.0}  {:>5.1}%",
                ability.definition().name,
                damage,
                share
            );
        }

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "CASTS    STARTED {}  COMPLETED {}  INTERRUPTED {}  WASTED {}",
            counters.casts_started,
            counters.casts_completed,
            counters.casts_interrupted,
            counters.casts_wasted
        );
        let _ = writeln!(
            out,
            "IGNITE   APPLICATIONS {}  AVERAGE STACKS {:.2}  DETONATIONS {}",
            counters.ignite_applications,
            counters.average_ignite_stacks(),
            counters.detonations
        );
        let _ = writeln!(
            out,
            "HEAT     CYCLES {}  AVERAGE {:.1}  30+ {:.1} S  70+ {:.1} S  INFERNO {:.1} S",
            counters.heat_cycles,
            counters.average_heat(),
            counters.seconds_warm,
            counters.seconds_hot,
            counters.seconds_inferno
        );
        let _ = writeln!(
            out,
            "CRIT     STRIKES {} OF {} HITS ({:.1}%)  PYROBLAST BOUNCES {}",
            counters.crits,
            counters.hits,
            if counters.hits > 0 {
                f64::from(counters.crits) / f64::from(counters.hits) * 100.0
            } else {
                0.0
            },
            counters.pyroblast_bounces
        );
        let _ = writeln!(
            out,
            "COOLDOWN REDUCTION {:.1} S  WASTED {:.1} S",
            counters.cooldown_reduction, counters.wasted_cooldown_reduction
        );
        let _ = writeln!(
            out,
            "TARGETS  DEATHS {}  OVERKILL {:.0}",
            result.deaths, result.overkill
        );
        out
    }
}

/// One build's DPS across the four target counts.
#[derive(Debug, Clone)]
pub struct RankRow {
    /// Which build.
    pub build: Build,
    /// DPS at 1, 3, 8 and 15 targets.
    pub dps: [f64; 4],
}

impl RankRow {
    /// The score a ranking sorts by: the mean across the four counts.
    #[must_use]
    pub fn average(&self) -> f64 {
        self.dps.iter().sum::<f64>() / self.dps.len() as f64
    }
}

/// A ranking table across the four target counts.
#[must_use]
pub fn ranking(rows: &[RankRow], limit: usize) -> String {
    let mut out = String::with_capacity(4_096);
    let _ = writeln!(
        out,
        "{:<4} {:<8} {:<6} {:>9} {:>9} {:>9} {:>9}  TALENTS",
        "RANK", "STYLE", "MASTERY", "1 TGT", "3 TGT", "8 TGT", "15 TGT"
    );
    for (index, row) in rows.iter().take(limit).enumerate() {
        let _ = writeln!(
            out,
            "{:<4} {:<8} {:<6} {:>9.0} {:>9.0} {:>9.0} {:>9.0}  {}",
            index + 1,
            row.build.style.as_str(),
            match row.build.mastery {
                engine_game::firemage::FireMastery::Haste => "HASTE",
                engine_game::firemage::FireMastery::Crit => "CRIT",
            },
            row.dps[0],
            row.dps[1],
            row.dps[2],
            row.dps[3],
            row.build.talent_names()
        );
    }
    out
}

/// How far apart two builds are, against the design's targets.
#[must_use]
pub fn comparison(haste: &[f64; 4], crit: &[f64; 4]) -> String {
    let mut out = String::with_capacity(1_024);
    let targets = [1, 3, 8, 15];
    let goals = ["HASTE 5-10% AHEAD", "WITHIN 10%", "WITHIN 5%", "WITHIN 5%"];
    let _ = writeln!(
        out,
        "{:<8} {:>12} {:>12} {:>10}  {:<20} RESULT",
        "TARGETS", "HASTE DPS", "CRIT DPS", "HASTE +%", "GOAL"
    );
    for index in 0..4 {
        let (a, b) = (haste[index], crit[index]);
        let delta = if b > 0.0 { (a / b - 1.0) * 100.0 } else { 0.0 };
        let met = match index {
            0 => (5.0..=10.0).contains(&delta),
            1 => delta.abs() <= 10.0,
            _ => delta.abs() <= 5.0,
        };
        let _ = writeln!(
            out,
            "{:<8} {:>12.0} {:>12.0} {:>9.1}%  {:<20} {}",
            targets[index],
            a,
            b,
            delta,
            goals[index],
            if met { "MET" } else { "MISSED" }
        );
    }
    out
}
