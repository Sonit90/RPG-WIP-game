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
    let gear = profile.worn(class.hero());
    run_scenario(&config, build, &gear, ITERATIONS, seed)
}

#[test]
fn the_same_seed_replays_exactly_and_a_different_one_does_not() {
    for class in Class::ALL {
        let build = default_build(class);
        let profile = GearProfile::headline(GearTier::High);
        let first = fight(class, &build, &profile, SEED);
        let second = fight(class, &build, &profile, SEED);
        assert_eq!(first.damage.to_bits(), second.damage.to_bits(), "{class:?}");
        assert_eq!(first.by_ability, second.by_ability, "{class:?}");
        assert_eq!(first.metrics, second.metrics, "{class:?}");
    }
    // Crits and Ignite rolls are random, so another seed is another fight.
    let build = default_build(Class::Mage);
    let profile = GearProfile::headline(GearTier::High);
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
    let profiles = GearProfile::select(&[GearTier::None, GearTier::High]);
    let run = || matrix::run(&Class::ALL, &profiles, 1, SEED, &|_, _| {});
    let first = matrix_report(&run(), &options, None, None, &[]);
    let second = matrix_report(&run(), &options, None, None, &[]);
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
    // The "catastrophic scaling" guard: every tier out-damages the one below it, for every
    // class, through real catalog items. Few iterations and one seed, so it is quick and
    // never flakes; the report's gear-scaling section is the same check at full accuracy.
    for class in Class::ALL {
        let build = default_build(class);
        let dps: Vec<(GearTier, f64)> = PROFILES
            .iter()
            .map(|profile| (profile.tier, fight(class, &build, profile, SEED).dps))
            .collect();
        for pair in dps.windows(2) {
            assert!(
                pair[0].1 < pair[1].1,
                "{class:?}: {:?} deals {:.1} DPS but {:?} deals {:.1}",
                pair[1].0,
                pair[1].1,
                pair[0].0,
                pair[0].1
            );
        }
    }
}

#[test]
fn the_scaling_section_reads_the_matrix_and_flags_a_tier_that_does_not_beat_the_one_below() {
    use crate::scaling::{self, Metric};

    let options = Options {
        iterations: 1,
        seed: SEED,
        classes: vec![Class::Warrior],
        ..Options::default()
    };
    let profiles = GearProfile::select(&options.tiers);
    let matrix = matrix::run(&options.classes, &profiles, 1, SEED, &|_, _| {});
    let mut measured = scaling::measure(&matrix, &options, &|_, _| {});
    assert_eq!(measured.classes.len(), 1);
    assert_eq!(measured.classes[0].tiers.len(), GearTier::ALL.len());
    assert!(measured.violations_of(&[Metric::Dps]).is_empty());

    // Flatten the top tier: it is no longer better, and the check says exactly that.
    measured.classes[0].tiers[4].dps = measured.classes[0].tiers[3].dps;
    let found = measured.violations_of(&[Metric::Dps]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].upper, GearTier::P2Max);
    let mut text = String::new();
    scaling::write_section(&mut text, &measured);
    assert!(text.contains("1 violation"), "{text}");
}

#[test]
fn the_ability_breakdown_sums_to_the_total_damage() {
    for class in Class::ALL {
        for build in builds_for(class) {
            let result = fight(class, &build, &GearProfile::headline(GearTier::High), SEED);
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
            let gear = GearProfile::headline(GearTier::High).worn(class.hero());
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

    let top = GearProfile::headline(GearTier::High);
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
    let top = GearProfile::headline(GearTier::High);
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
fn the_build_table_has_default_and_two_builds_for_every_class() {
    for class in Class::ALL {
        let builds = builds_for(class);
        assert_eq!(builds[0].name(), "Default", "{class:?}");
        assert!(builds.iter().all(|build| build.class() == class));
    }
    for class in Class::ALL {
        let names: Vec<String> = builds_for(class)
            .iter()
            .map(|build| build.name().to_owned())
            .collect();
        assert_eq!(names, ["Default", "Build A", "Build B"], "{class:?}");
    }
}

#[test]
fn the_report_has_every_section_and_every_class() {
    let options = Options {
        iterations: 1,
        seed: SEED,
        ..Options::default()
    };
    let profiles = GearProfile::select(&[GearTier::High]);
    let matrix = matrix::run(&Class::ALL, &profiles, 1, SEED, &|_, _| {});
    let text = matrix_report(&matrix, &options, None, None, &[]);
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
    assert_eq!(options.tiers, [GearTier::High]);
    assert_eq!((options.iterations, options.seed), (5, 9));

    let all = parse(&["--class", "all", "--gear", "all"]);
    assert_eq!(all.classes, Class::ALL);
    assert_eq!(all.tiers, GearTier::ALL);

    assert!(crate::parse_options(["--class".to_owned(), "bard".to_owned()].into_iter()).is_err());
    assert!(crate::parse_options(["--iterations".to_owned()].into_iter()).is_err());
}

/// Wraps a rotation and notes when the Ultimate bar becomes full.
struct EnergyProbe {
    inner: Box<dyn crate::ai::Rotation>,
    was_full: bool,
    full_at: Vec<f64>,
}

impl crate::ai::Rotation for EnergyProbe {
    fn act(
        &mut self,
        simulation: &mut engine_game::sim::Simulation,
        pack: &[engine_game::entity::EntityId],
    ) {
        let full = simulation.mage().energy.is_full();
        if full && !self.was_full {
            self.full_at.push(simulation.time());
        }
        self.was_full = full;
        self.inner.act(simulation, pack);
    }

    fn tracked_cooldowns(&self) -> &'static [engine_game::firemage::FireAbility] {
        self.inner.tracked_cooldowns()
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn priority(&self) -> &'static [&'static str] {
        self.inner.priority()
    }
}

#[test]
fn the_bar_refills_in_about_a_minute_and_a_half() {
    // The Ultimate is balanced on a sixty-to-ninety-second cadence: the bar must refill in
    // roughly that, whichever way the rotation plays, so the ninety-second cooldown is the
    // slower of the two gates and the energy is a real second one.
    let profile = GearProfile::headline(GearTier::High);
    for scenario in [ST60, matrix::AOE8] {
        let config = SCENARIOS[scenario].config(Class::Mage).with_duration(400.0);
        let build = default_build(Class::Mage);
        let gear = profile.worn(Class::Mage.hero());
        let mut encounter = crate::scenario::Encounter::new(&config, &build, &gear, SEED);
        let mut probe = EnergyProbe {
            inner: build.rotation(),
            was_full: false,
            full_at: Vec::new(),
        };
        let _ = encounter.run(&mut probe);
        // `full_at[0]` is the opener; every later entry is a refill after a cast.
        assert!(
            probe.full_at.len() >= 3,
            "scenario {scenario}: the bar filled {} times in 400 s",
            probe.full_at.len()
        );
        for pair in probe.full_at.windows(2).skip(1) {
            let gap = pair[1] - pair[0];
            assert!(
                (60.0..=100.0).contains(&gap),
                "scenario {scenario}: the bar took {gap:.0} s between full"
            );
        }
    }
}

#[test]
fn the_mage_uses_the_beam_and_it_shows_in_the_damage_table() {
    let build = default_build(Class::Mage);
    let profile = GearProfile::headline(GearTier::High);
    let result = fight(Class::Mage, &build, &profile, SEED);
    let beam =
        engine_game::ability::AbilityId::from_fire(engine_game::firemage::FireAbility::FireBeam)
            .slot();
    let dealt = result.by_ability[beam];
    assert!(dealt > 0.0, "the opener never fired the beam");
}

#[test]
fn the_warrior_builds_put_their_talents_in_the_game() {
    use engine_game::talent::warrior::WarriorTalentEffects;
    let profile = GearProfile::headline(GearTier::High);
    let gear = profile.worn(Class::Warrior.hero());
    let none = WarriorTalentEffects::default();
    for build in builds_for(Class::Warrior) {
        let config = SCENARIOS[ST60].config(Class::Warrior);
        let encounter = crate::scenario::Encounter::new(&config, &build, &gear, SEED);
        let effects = *encounter.simulation().warrior_effects();
        if build.name() == "Default" {
            assert_eq!(effects, none);
        } else {
            assert_ne!(effects, none, "{} bought nothing", build.name());
        }
    }
}

#[test]
fn the_warrior_reports_valor_and_fervor() {
    let build = default_build(Class::Warrior);
    let result = fight(
        Class::Warrior,
        &build,
        &GearProfile::headline(GearTier::High),
        SEED,
    );
    let resources = &result.metrics.resources;
    assert!(resources.valor_generated > 10.0, "{resources:?}");
    assert!(resources.valor_fills >= 3.0, "{resources:?}");
    assert!(resources.fervor_stacks_gained >= 3.0, "{resources:?}");
    let fervor = engine_game::status::StatusKind::BattleFervor.index();
    assert!(result.metrics.buff_uptime(fervor) > 0.5);
    assert!(result.metrics.average_mitigation() > 0.0);
}

#[test]
fn the_warriors_area_damage_stays_level_with_the_mages() {
    // The balance target: Mage Default / (Warrior + Druid) near one in an eight-target fight.
    // The bleed cap on Colossus Strike is what holds the Warrior there; a small band, because
    // three iterations are noisy.
    let profile = GearProfile::headline(GearTier::High);
    let dps = |class: Class| {
        let build = default_build(class);
        let config = SCENARIOS[matrix::AOE8].config(class);
        let gear = profile.worn(class.hero());
        run_scenario(&config, &build, &gear, ITERATIONS, SEED).dps
    };
    let ratio = dps(Class::Mage) / (dps(Class::Warrior) + dps(Class::Druid));
    assert!((0.9..=1.1).contains(&ratio), "Mage / (W + D) is {ratio:.2}");
}

#[test]
fn the_validation_names_every_talent_and_flags_what_it_finds() {
    let options = Options {
        iterations: 1,
        seed: SEED,
        classes: vec![Class::Warrior],
        tiers: vec![GearTier::High],
        ..Options::default()
    };
    let profiles = GearProfile::select(&options.tiers);
    let matrix = matrix::run(&options.classes, &profiles, 1, SEED, &|_, _| {});
    let validation = crate::validation::run(&matrix, &options, &|_, _| {});
    assert_eq!(validation.classes.len(), 1);
    let warrior = &validation.classes[0];
    assert_eq!(warrior.talents.len(), 8);
    assert_eq!(warrior.builds.len(), 3);
    assert_eq!(warrior.best.len(), 1);
    // Something is in the best build, so something has a removal figure.
    assert!(warrior.talents.iter().any(|t| t.removal[0].is_some()));

    let text = matrix_report(&matrix, &options, Some(&validation), None, &[]);
    assert!(text.contains("## 6. Talent validation"));
    for talent in engine_game::talent::hero_talents(Class::Warrior.hero())
        .visible()
        .iter()
    {
        assert!(text.contains(&talent.name), "{} is missing", talent.name);
    }
}
