//! The Druid's eight talents, each observed in a real simulation.
//!
//! Every test plays the same scene twice - without the talent and with it - and asserts on the
//! difference, so what is measured is the talent and not the kit around it.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::druid::tuning::*;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::{Status, StatusKind};
use engine_game::talent::druid::DruidTalent;
use engine_game::talent::{TalentGrid, TalentKey};
use engine_game::world::{EntityDesc, EntityKind};

fn druid_with(talents: &[DruidTalent]) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.set_hero(Hero::Druid));
    simulation.select_nearest_target();
    simulation.set_talents(TalentGrid::from_talents(talents));
    simulation
}

fn target(simulation: &Simulation) -> EntityId {
    simulation.player().target.expect("a dummy is selected")
}

fn stacks(simulation: &Simulation, id: EntityId, kind: StatusKind) -> u8 {
    simulation
        .world()
        .statuses(id)
        .map_or(0, |statuses| statuses.stacks(kind))
}

fn has(simulation: &Simulation, id: EntityId, kind: StatusKind) -> bool {
    simulation
        .world()
        .statuses(id)
        .is_some_and(|statuses| statuses.has(kind))
}

fn cast_through(simulation: &mut Simulation, ability: FireAbility) {
    simulation.step_until_ready();
    simulation.cast(ability).expect("the cast was refused");
    for _ in 0..800 {
        if simulation.casting().is_none() {
            break;
        }
        simulation.step_once();
    }
}

fn healing_dummy(simulation: &Simulation) -> EntityId {
    simulation
        .world()
        .entities()
        .find(|id| simulation.world().kind(*id) == Some(EntityKind::HealingDummy))
        .expect("the main hall has a healing dummy")
}

fn wounded_ally(simulation: &mut Simulation) -> EntityId {
    let ally = healing_dummy(simulation);
    if let Some(health) = simulation.world_mut().health_mut(ally) {
        let current = health.current();
        health.damage(current * 0.5);
    }
    simulation.player_mut().target = Some(ally);
    ally
}

fn healing_done(simulation: &Simulation, ability: FireAbility) -> f32 {
    let id = AbilityId::from_fire(ability);
    simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Healing {
                amount, ability, ..
            } if *ability == id => Some(*amount),
            _ => None,
        })
        .sum()
}

fn first_damage(simulation: &Simulation, ability: FireAbility) -> f32 {
    let id = AbilityId::from_fire(ability);
    simulation
        .events()
        .iter()
        .find_map(|event| match event {
            CombatEvent::Damage(damage) if damage.ability == id => Some(damage.amount),
            _ => None,
        })
        .expect("the ability dealt damage")
}

fn poison(simulation: &mut Simulation, victim: EntityId) {
    let source = simulation.player().entity;
    let status = Status::new(StatusKind::Poisoned, source, AbilityId::NONE, 30.0, 10.0);
    if let Some(statuses) = simulation.world_mut().statuses_mut(victim) {
        statuses.apply(status);
    }
}

// ------------------------------------------------------------------------ swift wrath

#[test]
fn swift_wrath_makes_every_fourth_consecutive_wrath_instant_with_double_astral() {
    for talented in [false, true] {
        let mut simulation = druid_with(if talented {
            &[DruidTalent::SwiftWrath]
        } else {
            &[]
        });
        for _ in 0..3 {
            cast_through(&mut simulation, FireAbility::Wrath);
        }
        simulation.step_until_ready();
        let before = simulation.druid().astral.value();
        simulation.cast(FireAbility::Wrath).expect("fourth wrath");
        let gained = simulation.druid().astral.value() - before;
        if talented {
            assert!(simulation.casting().is_none(), "the fourth was not instant");
            assert!((gained - 30.0).abs() < 1e-3, "double astral: {gained}");
        } else {
            assert!(simulation.casting().is_some(), "an ordinary wrath is cast");
        }
    }
}

#[test]
fn another_spell_breaks_the_swift_wrath_run() {
    let mut simulation = druid_with(&[DruidTalent::SwiftWrath]);
    cast_through(&mut simulation, FireAbility::Wrath);
    cast_through(&mut simulation, FireAbility::Wrath);
    cast_through(&mut simulation, FireAbility::Moonfire);
    cast_through(&mut simulation, FireAbility::Wrath);
    cast_through(&mut simulation, FireAbility::Wrath);
    cast_through(&mut simulation, FireAbility::Wrath);
    simulation.step_until_ready();
    simulation.cast(FireAbility::Wrath).expect("cast");
    assert!(simulation.casting().is_none(), "the fourth since the break");
}

// ------------------------------------------------------------------------ verdant sunfire

#[test]
fn verdant_sunfire_reaches_further_and_burns_softer() {
    let mut results = Vec::new();
    for talented in [false, true] {
        let mut simulation = druid_with(if talented {
            &[DruidTalent::VerdantSunfire]
        } else {
            &[]
        });
        let primary = target(&simulation);
        let centre = simulation.world().position(primary).expect("position");
        let far = simulation.world_mut().spawn(EntityDesc::dummy(
            centre + Vec3::new(10.0, 0.0, 0.0),
            5_000_000.0,
        ));
        cast_through(&mut simulation, FireAbility::Sunfire);
        let per_tick = simulation
            .world()
            .statuses(primary)
            .and_then(|statuses| statuses.get(StatusKind::Sunfire))
            .map(|status| status.damage_per_tick)
            .expect("the primary burns");
        results.push((has(&simulation, far, StatusKind::Sunfire), per_tick));
    }
    assert!(!results[0].0, "ten metres is outside eight");
    assert!(results[1].0, "ten metres is inside twelve");
    assert!(
        (results[1].1 / results[0].1 - 0.8).abs() < 1e-3,
        "{results:?}"
    );
}

// ------------------------------------------------------------------------ deep exposure

#[test]
fn deep_exposure_stacks_higher_and_lapses_sooner() {
    let mut plain = druid_with(&[]);
    let mut deep = druid_with(&[DruidTalent::DeepExposure]);
    for simulation in [&mut plain, &mut deep] {
        for _ in 0..24 {
            cast_through(simulation, FireAbility::Wrath);
        }
    }
    let (a, b) = (target(&plain), target(&deep));
    assert_eq!(
        stacks(&plain, a, StatusKind::NaturesExposure),
        NATURES_EXPOSURE_MAX_STACKS
    );
    assert_eq!(
        stacks(&deep, b, StatusKind::NaturesExposure),
        NATURES_EXPOSURE_MAX_STACKS + 4
    );
    let left = |simulation: &Simulation, id| {
        simulation.world().statuses(id).map_or(0.0, |statuses| {
            statuses.remaining(StatusKind::NaturesExposure)
        })
    };
    assert!(left(&plain, a) > NATURES_EXPOSURE_DURATION - 2.0);
    assert!(left(&deep, b) <= NATURES_EXPOSURE_DURATION * 0.7 + 1e-3);
}

// ------------------------------------------------------------------------ living seed

fn regrowth_then_damage(talents: &[DruidTalent], wait_seconds: u32) -> (f32, f32) {
    let mut simulation = druid_with(talents);
    let ally = wounded_ally(&mut simulation);
    cast_through(&mut simulation, FireAbility::Regrowth);
    let direct = healing_done(&simulation, FireAbility::Regrowth);
    simulation.step_many(wait_seconds * 60);
    poison(&mut simulation, ally);
    simulation.step_many(120);
    (direct, healing_done(&simulation, FireAbility::Regrowth))
}

#[test]
fn living_seed_pays_a_share_of_the_heal_when_the_target_is_next_hurt() {
    let (direct, plain) = regrowth_then_damage(&[], 1);
    let (direct_seeded, seeded) = regrowth_then_damage(&[DruidTalent::LivingSeed], 1);
    assert!((direct - direct_seeded).abs() < 1e-3);
    let seed = seeded - plain;
    assert!(
        (seed - direct * 0.3).abs() < direct * 0.01,
        "seed paid {seed} of a {direct} heal"
    );
}

#[test]
fn a_living_seed_withers_if_nothing_hurts_the_target_in_time() {
    let (_, plain) = regrowth_then_damage(&[], 12);
    let (_, seeded) = regrowth_then_damage(&[DruidTalent::LivingSeed], 12);
    assert!((seeded - plain).abs() < 1e-3, "a withered seed paid");
}

#[test]
fn swiftmend_leaves_a_seed_too() {
    let run = |talents: &[DruidTalent]| {
        let mut simulation = druid_with(talents);
        let ally = wounded_ally(&mut simulation);
        cast_through(&mut simulation, FireAbility::Rejuvenation);
        cast_through(&mut simulation, FireAbility::Swiftmend);
        let direct = healing_done(&simulation, FireAbility::Swiftmend);
        poison(&mut simulation, ally);
        simulation.step_many(120);
        // A seed pays under Regrowth's name; nothing else in this scene is Regrowth.
        (direct, healing_done(&simulation, FireAbility::Regrowth))
    };
    let (direct, plain) = run(&[]);
    let (_, seeded) = run(&[DruidTalent::LivingSeed]);
    assert_eq!(plain, 0.0);
    assert!(
        (seeded - direct * 0.3).abs() < direct * 0.01,
        "{seeded} {direct}"
    );
}

// ------------------------------------------------------------------------ overgrowth

#[test]
fn overgrowth_shortens_swiftmend_and_refunds_astral_power() {
    let run = |talents: &[DruidTalent]| {
        let mut simulation = druid_with(talents);
        wounded_ally(&mut simulation);
        cast_through(&mut simulation, FireAbility::Rejuvenation);
        let astral = simulation.druid().astral.value();
        cast_through(&mut simulation, FireAbility::Swiftmend);
        (
            simulation
                .mage()
                .abilities
                .recharge_remaining(FireAbility::Swiftmend),
            simulation.druid().astral.value() - astral,
        )
    };
    let (cooldown, astral) = run(&[]);
    let (shorter, refunded) = run(&[DruidTalent::Overgrowth]);
    assert!(
        (cooldown - shorter - 3.0).abs() < 0.05,
        "{cooldown} {shorter}"
    );
    assert!(
        (refunded - astral - 10.0).abs() < 1e-3,
        "{astral} {refunded}"
    );
}

// ------------------------------------------------------------------------ astral attunement

#[test]
fn astral_attunement_is_cheaper_and_weaker() {
    let run = |talents: &[DruidTalent]| {
        let mut simulation = druid_with(talents);
        simulation.druid_mut().astral.gain(75.0);
        let armed = simulation.cast(FireAbility::AstralPower).is_ok();
        simulation.step_until_ready();
        if armed {
            cast_through(&mut simulation, FireAbility::Moonfire);
        }
        (
            armed,
            simulation.druid().astral.value(),
            first_damage(&simulation, FireAbility::Moonfire),
        )
    };
    let (armed_plain, _, _) = {
        let mut simulation = druid_with(&[]);
        simulation.druid_mut().astral.gain(75.0);
        (simulation.cast(FireAbility::AstralPower).is_ok(), 0.0, 0.0)
    };
    assert!(
        !armed_plain,
        "seventy-five is not a full bar without the talent"
    );

    let (armed, left, empowered) = run(&[DruidTalent::AstralAttunement]);
    assert!(armed);
    // 75 spent, then Moonfire's own generation went back in.
    assert!(left < 20.0, "left {left}");

    // The same empowered Moonfire on the plain kit, for the bonus.
    let mut simulation = druid_with(&[]);
    simulation.druid_mut().astral.gain(100.0);
    simulation.cast(FireAbility::AstralPower).expect("arm");
    simulation.step_until_ready();
    cast_through(&mut simulation, FireAbility::Moonfire);
    let plain = first_damage(&simulation, FireAbility::Moonfire);
    let expected = (1.0 + ASTRAL_MOONFIRE_DAMAGE * 0.85) / (1.0 + ASTRAL_MOONFIRE_DAMAGE);
    assert!(
        (empowered / plain - expected).abs() < 0.01,
        "{empowered} / {plain} vs {expected}"
    );
}

// ------------------------------------------------------------------------ barkskin instinct

#[test]
fn barkskin_instinct_shares_wild_rush_with_nearby_allies() {
    for talented in [false, true] {
        let mut simulation = druid_with(if talented {
            &[DruidTalent::BarkskinInstinct]
        } else {
            &[]
        });
        let ally = healing_dummy(&simulation);
        let me = simulation
            .world()
            .position(simulation.player().entity)
            .expect("me");
        let there = simulation.world().position(ally).expect("ally");
        assert!(
            Vec3::new(me.x - there.x, 0.0, me.z - there.z).length() < 8.0,
            "the healing dummy stands within eight metres of the player"
        );
        cast_through(&mut simulation, FireAbility::WildRush);
        assert_eq!(has(&simulation, ally, StatusKind::Barkskin), talented);
        if talented {
            let reduction = simulation
                .world()
                .statuses(ally)
                .map(|statuses| statuses.damage_reduction())
                .expect("statuses");
            assert!((reduction - 0.25).abs() < 1e-5);
            simulation.step_many(5 * 60);
            assert!(
                !has(&simulation, ally, StatusKind::Barkskin),
                "lasts four seconds"
            );
        }
    }
}

// ------------------------------------------------------------------------ tranquil mind

#[test]
fn tranquil_mind_recharges_sooner_costs_less_and_heals_less() {
    let run = |talents: &[DruidTalent]| {
        let mut simulation = druid_with(talents);
        let ally = wounded_ally(&mut simulation);
        let before = simulation.world().health(ally).map_or(0.0, |h| h.current());
        let mana = simulation.druid().mana.current();
        simulation.step_until_ready();
        simulation.cast(FireAbility::Tranquility).expect("cast");
        // The cost is paid when the channel ends, before the regeneration of the next tick.
        let mut spent = 0.0;
        for _ in 0..800 {
            if simulation.casting().is_none() {
                break;
            }
            simulation.step_once();
            spent += simulation
                .events()
                .iter()
                .filter_map(|event| match event {
                    CombatEvent::ManaSpent { amount, .. } => Some(*amount),
                    _ => None,
                })
                .sum::<f32>();
        }
        let _ = mana;
        let healed = simulation.world().health(ally).map_or(0.0, |h| h.current()) - before;
        (
            spent,
            simulation
                .mage()
                .abilities
                .recharge_remaining(FireAbility::Tranquility),
            healed,
        )
    };
    let (mana, cooldown, healing) = run(&[]);
    let (cheap_mana, shorter, softer) = run(&[DruidTalent::TranquilMind]);
    assert!(cheap_mana / mana > 0.0 && (cheap_mana / mana - 0.5).abs() < 0.02);
    assert!(
        (cooldown - shorter - 40.0).abs() < 0.1,
        "{cooldown} {shorter}"
    );
    assert!(
        (softer / healing - 0.85).abs() < 0.01,
        "{softer} / {healing}"
    );
}

// ------------------------------------------------------------------------ the framework

#[test]
fn every_druid_talent_has_a_row_a_cost_and_a_description_without_holes() {
    for talent in DruidTalent::ALL {
        let spec = talent.spec();
        assert!(!spec.description.contains('{'), "{}", spec.description);
        assert!(spec.cost >= 3);
    }
}
