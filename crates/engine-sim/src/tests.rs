//! Cross-class tests: determinism, sanity of every class at every gear tier, and the
//! arithmetic the report depends on.

use engine_game::ability::ABILITY_SLOTS;

use crate::Options;
use crate::balance::matrix_report;
use crate::build::{ClassBuild, builds_for};
use crate::class::Class;
use crate::gear::{GearProfile, GearTier, PROFILES};
use crate::matrix::{self, SCENARIOS, ST60};
use crate::metrics::AURA_COUNT;
use crate::scenario::{RunResult, run_scenario};

/// Fights are short and few so the suite stays quick; every run is seeded, so it never flakes.
const ITERATIONS: u32 = 3;
const SEED: u64 = 0x5EED_0001;

/// The default (first) build of a class.
fn default_build(class: Class) -> ClassBuild {
    builds_for(class).remove(0)
}

/// One fight of the single-target 60 s scenario.
fn fight(class: Class, build: &ClassBuild, profile: &GearProfile, seed: u64) -> RunResult {
    let config = SCENARIOS[ST60].config(class);
    let gear = profile.stat_block(class.hero());
    run_scenario(&config, build, &gear, ITERATIONS, seed)
}

#[test]
fn the_same_seed_replays_exactly_and_a_different_one_does_not() {
    for class in Class::ALL {
        let build = default_build(class);
        let profile = GearProfile::headline(GearTier::Top);
        let first = fight(class, &build, &profile, SEED);
        let second = fight(class, &build, &profile, SEED);
        assert_eq!(first.damage.to_bits(), second.damage.to_bits(), "{class:?}");
        assert_eq!(first.by_ability, second.by_ability, "{class:?}");
        assert_eq!(first.metrics, second.metrics, "{class:?}");
    }
    // Crits and Ignite rolls are random, so another seed is another fight.
    let build = default_build(Class::Mage);
    let profile = GearProfile::headline(GearTier::Top);
    let a = fight(Class::Mage, &build, &profile, SEED);
    let b = fight(Class::Mage, &build, &profile, SEED + 1);
    assert_ne!(a.damage.to_bits(), b.damage.to_bits());
}

#[test]
fn the_matrix_and_its_report_do_not_depend_on_thread_scheduling() {
    let options = Options {
        iterations: 1,
        seed: SEED,
        ..Options::default()
    };
    let profiles = GearProfile::select(&[GearTier::None, GearTier::Top]);
    let run = || matrix::run(&Class::ALL, &profiles, 1, SEED, &|_, _| {});
    let first = matrix_report(&run(), &options);
    let second = matrix_report(&run(), &options);
    assert_eq!(first, second, "two runs of the same seed disagree");
}

#[test]
fn every_class_deals_damage_in_every_gear_tier() {
    for class in Class::ALL {
        let build = default_build(class);
        for profile in PROFILES {
            let result = fight(class, &build, &profile, SEED);
            assert!(
                result.damage > 0.0 && result.dps > 0.0,
                "{class:?} dealt nothing in {}",
                profile.label()
            );
        }
    }
}

#[test]
fn better_gear_means_more_damage_for_every_class() {
    for class in Class::ALL {
        let build = default_build(class);
        let none = fight(class, &build, &GearProfile::headline(GearTier::None), SEED).dps;
        let top = fight(class, &build, &GearProfile::headline(GearTier::Top), SEED).dps;
        for medium in PROFILES.iter().filter(|p| p.tier == GearTier::Medium) {
            let dps = fight(class, &build, medium, SEED).dps;
            assert!(
                none < dps && dps < top,
                "{class:?} {}: none {none:.1}, medium {dps:.1}, top {top:.1}",
                medium.label()
            );
        }
    }
}

#[test]
fn the_ability_breakdown_sums_to_the_total_damage() {
    for class in Class::ALL {
        for build in builds_for(class) {
            let result = fight(class, &build, &GearProfile::headline(GearTier::Top), SEED);
            let sum: f64 = result.by_ability.iter().sum();
            assert!(
                (sum - result.damage).abs() <= result.damage * 1e-9 + 1e-6,
                "{class:?} {}: breakdown {sum} vs total {}",
                build.name(),
                result.damage
            );
        }
    }
}

#[test]
fn every_uptime_is_a_fraction_and_every_average_stack_count_is_sane() {
    for class in Class::ALL {
        for scenario in &SCENARIOS {
            let build = default_build(class);
            let config = scenario.config(class);
            let gear = GearProfile::headline(GearTier::Top).stat_block(class.hero());
            let metrics = run_scenario(&config, &build, &gear, 1, SEED).metrics;
            for aura in 0..AURA_COUNT {
                for uptime in [metrics.buff_uptime(aura), metrics.debuff_uptime(aura)] {
                    assert!(
                        (0.0..=1.0).contains(&uptime),
                        "{class:?} {} aura {aura}: {uptime}",
                        scenario.id
                    );
                }
                assert!(metrics.buffs[aura].average_stacks() >= 0.0);
                assert!(metrics.debuffs[aura].average_stacks() >= 0.0);
            }
        }
    }
}

#[test]
fn each_class_maintains_the_debuff_its_rotation_is_built_around() {
    use crate::metrics::{AURA_IGNITE, aura_name};
    use engine_game::status::StatusKind;

    let top = GearProfile::headline(GearTier::Top);
    let mage = fight(Class::Mage, &default_build(Class::Mage), &top, SEED).metrics;
    assert!(mage.debuff_uptime(AURA_IGNITE) > 0.5, "Ignite barely up");

    let druid = fight(Class::Druid, &default_build(Class::Druid), &top, SEED).metrics;
    for kind in [StatusKind::Moonfire, StatusKind::Sunfire] {
        assert!(
            druid.debuff_uptime(kind.index()) > 0.8,
            "{} uptime {}",
            aura_name(kind.index()),
            druid.debuff_uptime(kind.index())
        );
    }

    let warrior = fight(Class::Warrior, &default_build(Class::Warrior), &top, SEED).metrics;
    assert!(warrior.debuff_uptime(StatusKind::Sundered.index()) > 0.5);
}

#[test]
fn a_rotation_never_casts_more_than_its_cooldowns_allow() {
    let top = GearProfile::headline(GearTier::Top);
    for class in Class::ALL {
        let build = default_build(class);
        let metrics = fight(class, &build, &top, SEED).metrics;
        for ability in build.rotation().tracked_cooldowns() {
            let slot = engine_game::ability::AbilityId::from_fire(*ability).slot();
            assert!(
                metrics.casts[slot] <= metrics.max_casts[slot] + 1.0,
                "{class:?} {ability:?}: {} casts of {} allowed",
                metrics.casts[slot],
                metrics.max_casts[slot]
            );
        }
        assert_eq!(metrics.casts.len(), ABILITY_SLOTS);
    }
}

#[test]
fn the_build_table_has_default_for_every_class_and_two_more_for_the_mage() {
    for class in Class::ALL {
        let builds = builds_for(class);
        assert_eq!(builds[0].name(), "Default", "{class:?}");
        assert!(builds.iter().all(|build| build.class() == class));
    }
    let mage: Vec<String> = builds_for(Class::Mage)
        .iter()
        .map(|build| build.name().to_owned())
        .collect();
    assert_eq!(mage, ["Default", "Build A", "Build B"]);
}

#[test]
fn the_report_has_every_section_and_every_class() {
    let options = Options {
        iterations: 1,
        seed: SEED,
        ..Options::default()
    };
    let profiles = GearProfile::select(&[GearTier::Top]);
    let matrix = matrix::run(&Class::ALL, &profiles, 1, SEED, &|_, _| {});
    let text = matrix_report(&matrix, &options);
    for needle in [
        "## 1. Summary matrix",
        "## 2. Balance check",
        "## 3. Class detail",
        "## 4. Rotations",
        "## 5. Gear profiles",
        "Warrior / Default",
        "Druid / Default",
        "Mage / Build B",
        "Mage Default / (W+D)",
    ] {
        assert!(text.contains(needle), "report lacks {needle:?}");
    }
}

#[test]
fn the_command_line_selects_classes_and_gear() {
    let parse = |args: &[&str]| {
        crate::parse_options(args.iter().map(|arg| (*arg).to_owned()))
            .expect("valid arguments")
            .expect("not --help")
    };
    let options = parse(&[
        "--class",
        "druid",
        "--gear",
        "top",
        "--iterations",
        "5",
        "--seed",
        "9",
    ]);
    assert_eq!(options.classes, [Class::Druid]);
    assert_eq!(options.tiers, [GearTier::Top]);
    assert_eq!((options.iterations, options.seed), (5, 9));

    let all = parse(&["--class", "all", "--gear", "all"]);
    assert_eq!(all.classes, Class::ALL);
    assert_eq!(all.tiers, GearTier::ALL);

    assert!(crate::parse_options(["--class".to_owned(), "bard".to_owned()].into_iter()).is_err());
    assert!(crate::parse_options(["--iterations".to_owned()].into_iter()).is_err());
}
