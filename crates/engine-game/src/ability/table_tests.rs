//! The ability table and the cooldown runtime, asserted against the design.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use super::table::ABILITY_SLOTS;
use super::*;
use crate::firemage::stats::Stats;
use crate::firemage::talent::TalentEffects;
use crate::firemage::talent::{TalentGrid, TalentId};
use crate::hero::Hero;

fn effects() -> TalentEffects {
    TalentEffects::default()
}

#[test]
fn the_mages_bar_holds_the_ten_abilities_in_the_designed_order() {
    let names: Vec<&str> = Hero::FireMage
        .abilities()
        .iter()
        .map(|ability| ability.definition().name)
        .collect();
    assert_eq!(
        names,
        [
            "FIREBOLT",
            "FLAME LANCE",
            "FLAME CONE",
            "METEOR",
            "PYROBLAST",
            "FIRESTORM",
            "DETONATE",
            "FIRE MASTERY",
            "FLAME SHIELD",
            "BLINK",
        ]
    );
}

#[test]
fn the_removed_abilities_are_gone_from_the_class() {
    // Inferno, Flame Wave and the old Fire Detonation were replaced outright; a stray
    // row would be an ability the player could still find.
    for ability in FireAbility::ALL {
        let name = ability.definition().name;
        assert_ne!(name, "INFERNO");
        assert_ne!(name, "FLAME WAVE");
        assert_ne!(name, "FIRE DETONATION");
        assert_ne!(name, "COMBUSTION");
    }
}

#[test]
fn the_ultimate_is_not_on_the_bar_and_is_disabled() {
    assert!(FireAbility::Cataclysm.is_ultimate());
    assert_eq!(FireAbility::Cataclysm.bar_slot(), None);
    assert_eq!(Hero::FireMage.abilities().len(), ABILITY_SLOTS);
    #[allow(
        clippy::assertions_on_constants,
        reason = "the constant is the subject"
    )]
    {
        assert!(!ULTIMATE_ENABLED, "the ultimate has no design yet");
    }
}

#[test]
fn the_table_matches_the_design_document() {
    // The values after the balance pass recorded in docs/firemage-design.md; the two
    // Ignite scalings opened at 10% and were raised to 15% because the maintenance
    // build's payoff spells were not worth holding a burn for.
    let firebolt = FireAbility::Firebolt.definition();
    assert_eq!(
        (firebolt.cast_time, firebolt.cooldown, firebolt.coefficient),
        (1.0, 0.0, 1.00)
    );
    assert_eq!(
        (firebolt.fire().heat, firebolt.fire().ignite_chance),
        (2.0, 0.50)
    );

    let lance = FireAbility::FlameLance.definition();
    assert_eq!(
        (lance.cast_time, lance.cooldown, lance.charges),
        (1.5, 8.0, 2)
    );
    assert_eq!(
        (
            lance.coefficient,
            lance.fire().ignite_stacks,
            lance.fire().heat
        ),
        (2.60, 2, 4.0)
    );

    let cone = FireAbility::FlameCone.definition();
    assert_eq!(
        (cone.cast_time, cone.cooldown, cone.range),
        (0.0, 10.0, 12.0)
    );
    assert_eq!(
        (
            cone.coefficient,
            cone.fire().ignite_stacks,
            cone.fire().heat
        ),
        (1.40, 2, 3.0)
    );

    let meteor = FireAbility::Meteor.definition();
    assert_eq!(
        (meteor.cooldown, meteor.radius, meteor.cast_time),
        (20.0, 5.0, 0.0)
    );
    assert_eq!(meteor.fire().ignite_stacks, 0, "meteor applies no ignite");
    assert_eq!(
        (meteor.coefficient, meteor.fire().ignite_bonus_per_stack),
        (5.00, 0.15)
    );

    let pyroblast = FireAbility::Pyroblast.definition();
    assert_eq!((pyroblast.cast_time, pyroblast.cooldown), (3.0, 30.0));
    assert_eq!(
        (
            pyroblast.coefficient,
            pyroblast.fire().ignite_bonus_per_stack
        ),
        (6.50, 0.15)
    );

    let firestorm = FireAbility::Firestorm.definition();
    assert_eq!((firestorm.duration, firestorm.tick_interval), (6.0, 0.5));
    // The linear zone: twenty metres ahead, ten across.
    assert_eq!((firestorm.range, firestorm.width), (20.0, 10.0));
    assert_eq!(firestorm.target, TargetMode::Line);
    assert_eq!(
        (firestorm.tick_coefficient, firestorm.fire().ignite_chance),
        (0.70, 0.15)
    );
    assert_eq!(
        (firestorm.duration / firestorm.tick_interval) as i32,
        12,
        "twelve ticks, which the simulator assumes"
    );

    let detonate = FireAbility::Detonate.definition();
    assert_eq!((detonate.cooldown, detonate.radius), (0.0, 10.0));
    assert_eq!(detonate.cast_time, 0.0, "detonate is instant");
    assert!(
        !detonate.cooldown_hasted,
        "haste both fills the bank and spends it"
    );
    for ability in FireAbility::ALL {
        if ability != FireAbility::Detonate {
            assert!(
                ability.definition().cooldown_hasted,
                "{ability:?} stopped scaling with haste"
            );
        }
    }

    let mastery = FireAbility::FireMastery.definition();
    assert_eq!((mastery.cooldown, mastery.duration), (60.0, 20.0));
}

#[test]
fn only_the_ultimate_costs_a_resource() {
    for ability in Hero::FireMage.abilities() {
        assert_eq!(ability.definition().fire().energy_cost, 0.0);
    }
    assert_eq!(
        FireAbility::Cataclysm.definition().fire().energy_cost,
        100.0
    );
}

#[test]
fn a_charge_is_spent_and_comes_back_after_the_cooldown() {
    let effects = effects();
    let mut book = AbilityBook::new(&effects);
    assert!(book.spend(FireAbility::Meteor, &effects));
    assert!(!book.is_ready(FireAbility::Meteor));

    book.tick(19.0, 0.0, &effects);
    assert!(!book.is_ready(FireAbility::Meteor), "back early");
    book.tick(1.5, 0.0, &effects);
    assert!(book.is_ready(FireAbility::Meteor));
}

#[test]
fn two_charges_are_spent_one_at_a_time_and_return_one_at_a_time() {
    let effects = effects();
    let mut book = AbilityBook::new(&effects);
    assert_eq!(book.charges(FireAbility::FlameLance), 2);

    book.spend(FireAbility::FlameLance, &effects);
    assert!(book.is_ready(FireAbility::FlameLance));
    book.spend(FireAbility::FlameLance, &effects);
    assert_eq!(book.charges(FireAbility::FlameLance), 0);

    book.tick(8.0, 0.0, &effects);
    assert_eq!(book.charges(FireAbility::FlameLance), 1);
    book.tick(8.0, 0.0, &effects);
    assert_eq!(book.charges(FireAbility::FlameLance), 2);
}

#[test]
fn volatile_ignition_hands_pyroblast_a_second_charge() {
    let mut grid = TalentGrid::with_points(100);
    grid.purchase(TalentId::VolatileIgnition)
        .expect("affordable");
    let effects = grid.effects();
    assert_eq!(
        FireAbility::Pyroblast.definition().charges_with(&effects),
        2
    );
    assert_eq!(
        FireAbility::Pyroblast
            .definition()
            .charges_with(&TalentEffects::default()),
        1
    );
}

#[test]
fn haste_shortens_cooldowns_and_casts() {
    let effects = effects();
    let mut fast = AbilityBook::new(&effects);
    fast.spend(FireAbility::Meteor, &effects);
    fast.tick(10.0, 1.0, &effects);
    assert!(fast.is_ready(FireAbility::Meteor));

    let stats = Stats {
        haste: 1.0,
        ..Stats::base()
    };
    assert!((FireAbility::Pyroblast.definition().cast_time_with(&stats) - 1.5).abs() < 1e-6);
}

#[test]
fn a_cooldown_reduction_reports_what_it_wasted() {
    let effects = effects();
    let mut book = AbilityBook::new(&effects);

    // Nothing running: the whole reduction is wasted, and none of it is banked.
    assert!((book.reduce_cooldown(FireAbility::Meteor, 4.0, &effects) - 4.0).abs() < 1e-6);
    book.spend(FireAbility::Meteor, &effects);
    assert!(
        book.recharge_remaining(FireAbility::Meteor) > 19.0,
        "banked"
    );

    assert_eq!(
        book.reduce_cooldown(FireAbility::Meteor, 3.0, &effects),
        0.0
    );
    let wasted = book.reduce_cooldown(FireAbility::Meteor, 100.0, &effects);
    assert!(wasted > 0.0, "an overshoot reported nothing");
    assert!(book.is_ready(FireAbility::Meteor));
}

#[test]
fn a_zero_cooldown_ability_never_becomes_unready() {
    let effects = effects();
    let mut book = AbilityBook::new(&effects);
    for _ in 0..10 {
        assert!(book.spend(FireAbility::Firebolt, &effects));
        book.tick(0.016, 0.0, &effects);
        assert!(book.is_ready(FireAbility::Firebolt));
    }
}

#[test]
fn every_ability_round_trips_through_its_written_name() {
    let mut seen: Vec<&str> = Vec::new();
    for ability in FireAbility::ALL {
        let name = ability.config_name();
        assert_eq!(
            FireAbility::from_config_name(name),
            Some(ability),
            "{ability:?} did not survive its own name"
        );
        assert!(!seen.contains(&name), "{name} names two abilities");
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{name} is not a written name"
        );
        seen.push(name);
    }
    assert_eq!(FireAbility::from_config_name("frostbolt"), None);
    assert_eq!(FireAbility::from_config_name(""), None);
    // The display name is not the written one, and must not be usable as it.
    assert_eq!(FireAbility::from_config_name("FIREBOLT"), None);
}

#[test]
fn everything_on_the_bar_can_be_assigned_and_nothing_else_can() {
    for ability in Hero::FireMage.abilities().iter().copied() {
        assert!(
            ability.is_assignable(),
            "{ability:?} cannot be put in a slot"
        );
    }
    assert!(
        !FireAbility::Ignite.is_assignable(),
        "a damage meter row is not a button"
    );
    assert!(
        !FireAbility::Cataclysm.is_assignable(),
        "the Ultimate has a place of its own"
    );
}
