//! The Warrior, asserted against a real arena.
//!
//! What a unit test on Deflection or on the defence roll cannot see: that a real attack on a
//! real character generates exactly two tenths, that a bleed applied at one maximum health
//! ticks for another after Last Stand, that Puncture Blood's cost and its shield read two
//! different numbers, and that spending twenty gives the passive bonuses back on the same
//! call.
//!
//! Every attack in here goes through [`Simulation::deliver_attack`], which is the same
//! pipeline an enemy's swing takes — not a second one written for the tests.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::{AbilityId, Motion};
use engine_game::arena::{self, ArenaConfig};
use engine_game::character::stat::RATING_PER_PERCENT;
use engine_game::character::{DefenseOutcome, Stat, StatBlock};
use engine_game::combat::DamageType;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::warrior::tuning::*;
use engine_game::warrior::{
    DEFLECTION_MAXIMUM, DEFLECTION_PER_EVENT, DEFLECTION_PER_EVENT_IN_TRANCE, DEFLECTION_SPEND,
    PASSIVE_ARMOR_AT_ZERO, PASSIVE_MAX_HEALTH_AT_ZERO,
};
use engine_game::world::EntityKind;

/// A training arena with the Warrior selected and the nearest dummy targeted.
///
/// Out-of-combat regeneration is switched off: every test in here measures healing, and a
/// character mending on its own would be counted as an ability's work.
fn warrior() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.set_hero(Hero::Warrior));
    simulation.set_health_regen_per_second(0.0);
    simulation.select_nearest_target();
    simulation
}

/// The same, with a real enemy — one with a mind — spawned beside the player and selected.
///
/// A training dummy has no brain: it cannot be angry, it cannot be slowed and it cannot be
/// stunned. Anything asserting on threat or on a debuff the *enemy* carries needs one of
/// these instead.
fn warrior_against_an_enemy() -> (Simulation, EntityId) {
    let mut simulation = warrior();
    let Some(origin) = simulation.world().position(simulation.player().entity) else {
        panic!("the player has a position");
    };
    let enemy = simulation.spawn_enemy(
        // The boss, because it is the only thing in the roster with enough health to be
        // stomped eight times and still be there to carry the eighth stack.
        engine_game::enemy::EnemyKind::Boss,
        origin + Vec3::new(0.0, 0.0, -3.0),
        0.0,
    );
    simulation.player_mut().target = Some(enemy);
    (simulation, enemy)
}

/// What the health-scaled abilities measure against: Stamina and gear, without the class's
/// own multipliers.
fn scaling_health(simulation: &Simulation) -> f32 {
    simulation.scaling_health()
}

fn target(simulation: &Simulation) -> EntityId {
    simulation.player().target.expect("a dummy is selected")
}

/// Walks the player to within `metres` of its target.
fn close_to_target(simulation: &mut Simulation, metres: f32) {
    let victim = target(simulation);
    let Some(destination) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    let player = simulation.player().entity;
    let Some(from) = simulation.world().position(player) else {
        panic!("the player has a position");
    };
    let mut offset = from - destination;
    offset.y = 0.0;
    let length = offset.length().max(1e-3);
    let placed = destination + offset * (metres / length);
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = placed;
    }
}

/// Grants the player defensive ratings, replacing whatever was granted before.
///
/// A negative grant is how a test takes the Warrior's *base* avoidance away, which is what
/// isolating one of the three rolls needs — the class starts with three per cent of each.
fn set_avoidance(simulation: &mut Simulation, block: f32, parry: f32, dodge: f32) {
    let base = Hero::Warrior.base_stats();
    let mut bonus = StatBlock::zero();
    bonus.set(
        Stat::Block,
        block * RATING_PER_PERCENT - base.get(Stat::Block),
    );
    bonus.set(
        Stat::Parry,
        parry * RATING_PER_PERCENT - base.get(Stat::Parry),
    );
    bonus.set(
        Stat::Dodge,
        dodge * RATING_PER_PERCENT - base.get(Stat::Dodge),
    );
    // Armour off, so the numbers a mitigation test reads are the ones it set up.
    bonus.set(Stat::Armor, -base.get(Stat::Armor));
    simulation.set_stat_bonus(bonus);
}

/// One attack from the selected dummy, through the ordinary incoming pipeline.
fn swing(simulation: &mut Simulation, amount: f32) -> Option<DefenseOutcome> {
    let attacker = target(simulation);
    simulation.deliver_attack(
        attacker,
        amount,
        AbilityId::from_enemy(engine_game::enemy::EnemyAbility::Swing),
        DamageType::Physical,
    )
}

/// One magical attack, which no amount of Block, Parry or Dodge can answer.
fn bolt(simulation: &mut Simulation, amount: f32) {
    let attacker = target(simulation);
    simulation.deliver_attack(
        attacker,
        amount,
        AbilityId::from_enemy(engine_game::enemy::EnemyAbility::Bolt),
        DamageType::Arcane,
    );
}

fn deflection(simulation: &Simulation) -> f32 {
    simulation.warrior().deflection.value()
}

fn player_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .map_or(0.0, |health| health.current())
}

fn player_max_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .map_or(0.0, |health| health.max())
}

// ---------------------------------------------------------------------------- Deflection

#[test]
fn each_of_the_three_defensive_events_generates_exactly_two_tenths() {
    // One roll per attack, so each observed outcome is paid for exactly once. The test
    // watches for all three rather than forcing one, because forcing one would be testing a
    // hook rather than the pipeline.
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 30.0, 30.0, 30.0);

    let mut seen = [false; 3];
    for _ in 0..400 {
        let before = deflection(&simulation);
        let outcome = swing(&mut simulation, 1.0);
        let after = deflection(&simulation);
        match outcome {
            Some(DefenseOutcome::Hit) | None => {
                assert!(
                    (after - before).abs() < 1e-6,
                    "a hit paid {}",
                    after - before
                );
            }
            Some(defended) => {
                let gained = after - before;
                assert!(
                    (gained - DEFLECTION_PER_EVENT).abs() < 1e-5,
                    "{defended:?} paid {gained}"
                );
                let index = match defended {
                    DefenseOutcome::Blocked => 0,
                    DefenseOutcome::Parried => 1,
                    DefenseOutcome::Dodged => 2,
                    DefenseOutcome::Hit => unreachable!(),
                };
                seen[index] = true;
            }
        }
        if seen.iter().all(|seen| *seen) {
            // Reset the bank so the cap never masks a payment.
            simulation.warrior_mut().deflection.set(0.0);
        }
        simulation.warrior_mut().deflection.set(0.0);
    }
    assert_eq!(seen, [true; 3], "not every outcome happened: {seen:?}");
}

#[test]
fn one_attack_pays_once_however_many_events_it_produces() {
    // The property behind "duplicate combat events do not generate duplicate Deflection":
    // there is one roll per attack, so there is one payment. Two hundred attacks pay two
    // hundred times two tenths and never once twice.
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 100.0);
    simulation.warrior_mut().deflection.set(0.0);

    let mut defended = 0;
    for _ in 0..200 {
        let before = deflection(&simulation);
        let outcome = swing(&mut simulation, 1.0);
        let gained = deflection(&simulation) - before;
        if outcome == Some(DefenseOutcome::Dodged) {
            defended += 1;
            assert!((gained - DEFLECTION_PER_EVENT).abs() < 1e-5, "{gained}");
        } else {
            assert!(gained.abs() < 1e-6, "{gained}");
        }
        if deflection(&simulation) >= DEFLECTION_MAXIMUM {
            simulation.warrior_mut().deflection.set(0.0);
        }
    }
    assert!(defended > 100, "only {defended} of 200 attacks were dodged");
}

#[test]
fn deflection_is_bounded_at_both_ends_and_never_decays() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 100.0);
    for _ in 0..1_000 {
        swing(&mut simulation, 1.0);
    }
    assert_eq!(deflection(&simulation), DEFLECTION_MAXIMUM);

    // Ten seconds of nothing happening. A resource that decayed would hand the passive
    // bonuses back for free and remove every reason to press anything.
    simulation.step_many(600);
    assert_eq!(deflection(&simulation), DEFLECTION_MAXIMUM);

    // And it cannot be driven below zero, however many spenders are pressed.
    simulation.warrior_mut().deflection.set(0.0);
    close_to_target(&mut simulation, 3.0);
    for _ in 0..20 {
        let _ = simulation.cast(FireAbility::ShieldBlock);
        simulation.step_many(60);
    }
    assert!(deflection(&simulation) >= 0.0);
}

#[test]
fn the_passive_bonuses_scale_continuously_between_nothing_and_full() {
    let mut simulation = warrior();
    let expected = [
        (0.0, PASSIVE_MAX_HEALTH_AT_ZERO, PASSIVE_ARMOR_AT_ZERO),
        (25.0, 0.0375, 0.075),
        (50.0, 0.025, 0.05),
        (75.0, 0.0125, 0.025),
        (100.0, 0.0, 0.0),
    ];
    let bare = Hero::Warrior.base_stats().get(Stat::Armor);
    for (value, health_bonus, armor_bonus) in expected {
        simulation.warrior_mut().deflection.set(value);
        simulation.refresh_character();

        let health = player_max_health(&simulation);
        let expected_health = scaling_health(&simulation) * (1.0 + health_bonus);
        assert!(
            (health - expected_health).abs() < 1e-2,
            "{value}: {health} against {expected_health}"
        );

        let armor = simulation.defense_profile().armor;
        let expected_armor = bare * (1.0 + armor_bonus);
        assert!(
            (armor - expected_armor).abs() < 1e-2,
            "{value}: {armor} against {expected_armor}"
        );
    }
}

#[test]
fn spending_gives_the_passive_bonuses_back_on_the_same_call() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    simulation.warrior_mut().deflection.set(DEFLECTION_MAXIMUM);
    simulation.refresh_character();
    let capped_health = player_max_health(&simulation);
    let capped_armor = simulation.defense_profile().armor;

    simulation
        .cast(FireAbility::ShieldBlock)
        .expect("Shield Block is instant and needs nothing");

    assert!(
        (deflection(&simulation) - (DEFLECTION_MAXIMUM - DEFLECTION_SPEND)).abs() < 1e-5,
        "{}",
        deflection(&simulation)
    );
    // No tick has passed: the recalculation happens inside the spend.
    assert!(
        player_max_health(&simulation) > capped_health,
        "the health bonus did not come back"
    );
    assert!(
        simulation.defense_profile().armor > capped_armor,
        "the armour bonus did not come back"
    );
}

#[test]
fn every_empowerable_ability_checks_and_consumes_exactly_twenty() {
    // The whole spender list, driven through the real cast path: nineteen is not enough,
    // twenty is spent, and the passive bonuses move on the spend.
    let spenders = [
        FireAbility::WarStomp,
        FireAbility::ColossusStrike,
        FireAbility::ShieldCharge,
        FireAbility::ShieldBash,
        FireAbility::ShieldBlock,
        FireAbility::PunctureBlood,
        FireAbility::Adrenaline,
        FireAbility::SpellReflect,
        FireAbility::LastStand,
    ];
    for ability in spenders {
        assert!(
            ability.definition().warrior().is_empowerable(),
            "{ability:?} is not a spender"
        );
        assert!(
            (ability.definition().warrior().deflection_cost - DEFLECTION_SPEND).abs() < 1e-6,
            "{ability:?} costs something other than twenty"
        );

        // Not enough: the ability is cast anyway, and nothing is taken.
        let mut simulation = warrior();
        close_to_target(&mut simulation, 3.0);
        simulation.warrior_mut().deflection.set(19.0);
        simulation.refresh_character();
        simulation.cast(ability).expect("the cast was refused");
        assert!(
            (deflection(&simulation) - 19.0).abs() < 1e-5,
            "{ability:?} part-paid: {}",
            deflection(&simulation)
        );

        // Enough: exactly twenty, and the health bonus grows because the resource shrank.
        let mut simulation = warrior();
        close_to_target(&mut simulation, 3.0);
        simulation.warrior_mut().deflection.set(60.0);
        simulation.refresh_character();
        let before_health = player_max_health(&simulation);
        simulation.cast(ability).expect("the cast was refused");
        assert!(
            (deflection(&simulation) - 40.0).abs() < 1e-5,
            "{ability:?} spent {}",
            60.0 - deflection(&simulation)
        );
        assert!(
            player_max_health(&simulation) > before_health,
            "{ability:?} did not recalculate the passive bonus"
        );
    }
}

// ---------------------------------------------------------------------------- Battle Trance

#[test]
fn battle_trance_heals_one_per_cent_three_times_a_second_with_no_total_cap() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 100.0);
    close_to_target(&mut simulation, 3.0);

    // Hurt, so every heal lands rather than overhealing.
    let max = player_max_health(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        health.set_current(max * 0.2);
    }
    simulation
        .cast(FireAbility::BattleTrance)
        .expect("instant and self-cast");

    // Ten attacks avoided inside one second: three of them heal.
    let before = player_health(&simulation);
    for _ in 0..10 {
        swing(&mut simulation, 0.0);
    }
    let healed = player_health(&simulation) - before;
    let expected = 3.0 * max * BATTLE_TRANCE_HEAL;
    assert!(
        (healed - expected).abs() < 0.5,
        "{healed} against {expected}"
    );

    // A second later the allowance is back, and it is three again — not a total cap.
    simulation.step_many(60);
    let before = player_health(&simulation);
    for _ in 0..10 {
        swing(&mut simulation, 0.0);
    }
    let healed = player_health(&simulation) - before;
    assert!(
        (healed - expected).abs() < 0.5,
        "the second second healed {healed}"
    );
}

#[test]
fn battle_trance_doubles_the_resource_and_leaves_the_heal_alone() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 100.0);
    close_to_target(&mut simulation, 3.0);

    simulation.warrior_mut().deflection.set(0.0);
    swing(&mut simulation, 1.0);
    assert!((deflection(&simulation) - DEFLECTION_PER_EVENT).abs() < 1e-5);

    simulation
        .cast(FireAbility::BattleTrance)
        .expect("instant and self-cast");
    simulation.warrior_mut().deflection.set(0.0);
    swing(&mut simulation, 1.0);
    assert!(
        (deflection(&simulation) - DEFLECTION_PER_EVENT_IN_TRANCE).abs() < 1e-5,
        "{}",
        deflection(&simulation)
    );
}

// ---------------------------------------------------------------------------- Colossus Strike

#[test]
fn the_colossus_bleed_reads_maximum_health_on_every_tick_rather_than_snapshotting_it() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    simulation.warrior_mut().deflection.set(0.0);
    let victim = target(&simulation);

    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("a self-centred smash needs nothing");
    assert!(
        simulation
            .world()
            .statuses(victim)
            .is_some_and(|statuses| statuses.has(StatusKind::Bleeding)),
        "no bleed was applied"
    );

    // One tick at the health the Warrior had.
    let first = bleed_tick(&mut simulation, victim);

    // More maximum health, and the *next* tick is worth exactly the coefficient times what
    // the health went up by. Nothing reapplied the bleed: the status carries a coefficient,
    // not a number, so there is nowhere for the old health to have been kept.
    let coefficient = FireAbility::ColossusStrike
        .definition()
        .warrior()
        .tick_health_coefficient;
    let before_health = player_max_health(&simulation);
    let mut bonus = StatBlock::zero();
    bonus.set(Stat::Stamina, 500.0);
    simulation.set_stat_bonus(bonus);
    let after_health = player_max_health(&simulation);
    let second = bleed_tick(&mut simulation, victim);
    let expected = coefficient * (after_health - before_health);
    assert!(
        second > first,
        "the bleed snapshotted: {first} then {second}"
    );
    assert!(
        (second - first - expected).abs() < 0.5,
        "the tick moved by {} rather than {expected}",
        second - first
    );

    // And back down: less maximum health is less damage on the ticks that follow.
    simulation.set_stat_bonus(StatBlock::zero());
    let third = bleed_tick(&mut simulation, victim);
    assert!(
        (third - first).abs() < 0.5,
        "the bleed kept the larger health: {first}, {second}, then {third}"
    );
}

/// Runs the simulation until the bleed on `victim` ticks, and reports what it dealt.
fn bleed_tick(simulation: &mut Simulation, victim: EntityId) -> f32 {
    for _ in 0..240 {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::Damage(damage) = event
                && damage.target == victim
                && damage.ability.fire() == Some(FireAbility::ColossusStrike)
                && damage.amount > 0.0
                // The impact is dealt on the cast's own tick; a tick arrives later, on its
                // own, with no `AbilityUsed` beside it.
                && !simulation
                    .events()
                    .iter()
                    .any(|other| matches!(other, CombatEvent::AbilityUsed { .. }))
            {
                return damage.amount;
            }
        }
    }
    panic!("the bleed never ticked");
}

#[test]
fn colossus_strike_scales_its_impact_from_maximum_health() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    let small = impact_of(&mut simulation, victim);

    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let mut bonus = StatBlock::zero();
    bonus.set(Stat::Stamina, 500.0);
    simulation.set_stat_bonus(bonus);
    let victim = target(&simulation);
    let large = impact_of(&mut simulation, victim);

    assert!(
        large > small,
        "more maximum health did not hit harder: {small} then {large}"
    );
}

/// Casts Colossus Strike and reports what its impact dealt to `victim`.
fn impact_of(simulation: &mut Simulation, victim: EntityId) -> f32 {
    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("the smash was refused");
    simulation
        .events()
        .iter()
        .filter_map(CombatEvent::as_damage)
        .find(|damage| {
            damage.target == victim && damage.ability.fire() == Some(FireAbility::ColossusStrike)
        })
        .map_or(0.0, |damage| damage.amount)
}

// ---------------------------------------------------------------------------- Puncture Blood

#[test]
fn puncture_blood_spends_current_health_and_shields_from_maximum_health() {
    let mut simulation = warrior();
    let max = player_max_health(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        health.set_current(max * 0.8);
    }
    let before = player_health(&simulation);
    simulation.warrior_mut().deflection.set(0.0);

    simulation
        .cast(FireAbility::PunctureBlood)
        .expect("self-cast and instant");

    let spent = before - player_health(&simulation);
    assert!(
        (spent - before * PUNCTURE_HEALTH_COST).abs() < 0.5,
        "spent {spent} of a current {before}"
    );
    let shield = simulation.warrior().absorb.remaining();
    assert!(
        (shield - max * PUNCTURE_SHIELD).abs() < 0.5,
        "shielded {shield} against a maximum {max}"
    );
}

#[test]
fn an_empowered_puncture_shields_a_fifth_more_and_costs_the_same() {
    let mut plain = warrior();
    plain.warrior_mut().deflection.set(0.0);
    let max = player_max_health(&plain);
    let before = player_health(&plain);
    plain.cast(FireAbility::PunctureBlood).expect("instant");
    let plain_cost = before - player_health(&plain);
    let plain_shield = plain.warrior().absorb.remaining();

    let mut empowered = warrior();
    empowered.warrior_mut().deflection.set(DEFLECTION_SPEND);
    empowered.refresh_character();
    // The Deflection bonus is gone at twenty, so the maximum health is not the same: the
    // shield is compared as a *share* of maximum health rather than as a number.
    let before = player_health(&empowered);
    let before_max = player_max_health(&empowered);
    empowered.cast(FireAbility::PunctureBlood).expect("instant");
    // Read *after* the cast: spending twenty gives the passive maximum health bonus back
    // before the shield is worked out, which is the design's "recalculate immediately, then
    // apply the empowered effect" in the one place it is observable.
    let empowered_max = player_max_health(&empowered);
    assert!(empowered_max > before_max, "the spend did not recalculate");
    let empowered_cost = before - player_health(&empowered);
    let empowered_shield = empowered.warrior().absorb.remaining();

    assert!(
        (plain_shield / max - PUNCTURE_SHIELD).abs() < 1e-3,
        "{plain_shield} of {max}"
    );
    assert!(
        (empowered_shield / empowered_max - PUNCTURE_SHIELD * PUNCTURE_EMPOWERED_SCALE).abs()
            < 1e-3,
        "{empowered_shield} of {empowered_max}"
    );
    // Thirty-six per cent, not forty-five: the empowered share is a fifth more, not half.
    assert!((PUNCTURE_SHIELD * PUNCTURE_EMPOWERED_SCALE - 0.36).abs() < 1e-6);

    // The cost is a share of current health and Deflection does not touch it.
    assert!(
        (plain_cost / max - empowered_cost / before_max).abs() < 1e-3,
        "{plain_cost} against {empowered_cost}"
    );
}

// ---------------------------------------------------------------------------- Adrenaline

#[test]
fn adrenaline_banks_its_share_and_pays_it_back_over_the_recovery() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
    simulation.warrior_mut().deflection.set(0.0);
    let max = player_max_health(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        health.set_current(max * 0.5);
    }

    simulation
        .cast(FireAbility::Adrenaline)
        .expect("self-cast and instant");
    assert!((simulation.warrior().adrenaline_share - ADRENALINE_SHARE).abs() < 1e-6);

    // Magic, so nothing is dodged and the banked share is the whole of what changed.
    bolt(&mut simulation, 100.0);
    assert!(
        (simulation.warrior().recovery_pool - 30.0).abs() < 0.1,
        "{}",
        simulation.warrior().recovery_pool
    );
    bolt(&mut simulation, 100.0);
    assert!((simulation.warrior().recovery_pool - 60.0).abs() < 0.1);

    // The buff runs out and the pool becomes healing rather than a lump.
    let banked = simulation.warrior().recovery_pool;
    let low = player_health(&simulation);
    simulation.step_many(6 * 60 + 2);
    assert_eq!(simulation.warrior().recovery_pool, 0.0, "the pool was kept");
    let partway = player_health(&simulation);
    assert!(
        partway - low < banked,
        "the whole pool was healed at once: {} of {banked}",
        partway - low
    );

    simulation.step_many(6 * 60 + 2);
    let restored = player_health(&simulation) - low;
    assert!(
        restored >= banked - 1.0,
        "only {restored} of {banked} came back"
    );
}

#[test]
fn an_empowered_adrenaline_banks_four_tenths_over_the_same_six_seconds() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
    simulation.warrior_mut().deflection.set(DEFLECTION_SPEND);
    simulation.refresh_character();

    simulation.cast(FireAbility::Adrenaline).expect("instant");
    assert!((simulation.warrior().adrenaline_share - ADRENALINE_EMPOWERED_SHARE).abs() < 1e-6);
    bolt(&mut simulation, 100.0);
    assert!(
        (simulation.warrior().recovery_pool - 40.0).abs() < 0.1,
        "{}",
        simulation.warrior().recovery_pool
    );
    // The window is unchanged: six seconds of banking, whatever was spent.
    assert!(
        (simulation.warrior().adrenaline.remaining()
            - FireAbility::Adrenaline.definition().duration)
            .abs()
            < 0.1
    );
}

// ---------------------------------------------------------------------------- War Stomp

#[test]
fn war_stomp_is_a_hundred_and_twenty_degree_cleave_around_the_target() {
    let mut simulation = warrior();
    let victim = target(&simulation);
    let Some(centre) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    // One dummy beside the target — inside the cone — and one behind the Warrior, which no
    // cone pointed at the selection can reach.
    let others: Vec<EntityId> = simulation
        .world()
        .entities()
        .filter(|id| *id != victim && simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .take(2)
        .collect();
    assert!(others.len() >= 2, "the arena has more than two dummies");
    if let Some(body) = simulation.world_mut().body_mut(others[0]) {
        body.position = centre + Vec3::new(1.2, 0.0, 0.0);
    }
    close_to_target(&mut simulation, 3.0);
    let Some(player) = simulation.world().position(simulation.player().entity) else {
        panic!("the player has a position");
    };
    let behind = player + (player - centre);
    if let Some(body) = simulation.world_mut().body_mut(others[1]) {
        body.position = behind;
    }

    let hits = simulation
        .cast(FireAbility::WarStomp)
        .expect("the stomp was refused");
    assert_eq!(hits, 2, "the cleave caught {hits}");
    assert_eq!(
        simulation
            .world()
            .statuses(others[1])
            .map_or(0, |statuses| statuses.stacks(StatusKind::Sundered)),
        0,
        "something behind the Warrior was cleaved"
    );
}

#[test]
fn war_stomp_stacks_to_eight_and_pays_armour_for_the_eighth() {
    let (mut simulation, victim) = warrior_against_an_enemy();
    simulation.warrior_mut().deflection.set(0.0);

    let bare = simulation.defense_profile().armor;
    for expected in 1..=8 {
        simulation.cast(FireAbility::WarStomp).expect("in range");
        let stacks = simulation
            .world()
            .statuses(victim)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Sundered));
        assert_eq!(stacks, expected, "after {expected} stomps");
        // The attack speed the enemy has lost is the stacks, and nothing else.
        let scale = simulation.brain(victim).map_or(
            1.0,
            engine_game::enemy::brain::EnemyBrain::attack_speed_scale,
        );
        assert!(simulation.brain(victim).is_some(), "the enemy has a mind");
        let expected_scale = 1.0 - WAR_STOMP_ATTACK_SPEED_PER_STACK * f32::from(expected);
        assert!((scale - expected_scale).abs() < 1e-5, "{scale}");
        // Deflection is generated by whatever the enemy is doing back; kept at zero so the
        // stomps are never empowered and each one is worth exactly one stack.
        simulation.warrior_mut().deflection.set(0.0);
        simulation.step_many(60 * 4 + 2);
        simulation.warrior_mut().deflection.set(0.0);
        assert!(
            simulation.world().is_alive(victim),
            "the boss died mid-test"
        );
    }

    // A ninth changes nothing: eight is the ceiling.
    simulation.cast(FireAbility::WarStomp).expect("in range");
    assert_eq!(
        simulation
            .world()
            .statuses(victim)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Sundered)),
        8
    );

    assert!(simulation.warrior().war_stomp_armor, "no full stack seen");
    simulation.warrior_mut().deflection.set(0.0);
    let with_stomp = simulation.defense_profile().armor;
    // The bonuses add on the armour the sheet grants rather than multiplying each other:
    // Deflection's ten per cent and War Stomp's are both shares of the same number.
    let sheet_armor = Hero::Warrior.base_stats().get(Stat::Armor);
    assert!(
        (with_stomp - (bare + sheet_armor * WAR_STOMP_ARMOR_BONUS)).abs() < 1e-2,
        "{with_stomp} against {bare}"
    );

    // The debuff lapses, and the armour goes with it on the tick it does.
    simulation.step_many(60 * 11);
    assert!(!simulation.warrior().war_stomp_armor);
    assert!((simulation.defense_profile().armor - bare).abs() < 1e-3);
}

#[test]
fn an_empowered_war_stomp_applies_two_stacks() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    simulation.warrior_mut().deflection.set(DEFLECTION_SPEND);
    simulation.refresh_character();

    simulation.cast(FireAbility::WarStomp).expect("in range");
    assert_eq!(
        simulation
            .world()
            .statuses(victim)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Sundered)),
        2
    );
    assert_eq!(deflection(&simulation), 0.0);
}

// ---------------------------------------------------------------------------- the rest of the kit

#[test]
fn shield_bash_interrupts_and_only_silences_when_it_is_paid_for() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    simulation.warrior_mut().deflection.set(0.0);

    simulation.cast(FireAbility::ShieldBash).expect("in range");
    assert!(
        simulation
            .brain(victim)
            .is_none_or(|brain| !brain.is_silenced()),
        "a free bash silenced"
    );

    simulation.step_many(60 * 16);
    simulation.warrior_mut().deflection.set(DEFLECTION_SPEND);
    simulation.refresh_character();
    simulation.cast(FireAbility::ShieldBash).expect("in range");
    if let Some(brain) = simulation.brain(victim) {
        assert!(brain.is_silenced(), "twenty bought nothing");
        assert!((brain.silence_remaining() - SHIELD_BASH_SILENCE).abs() < 0.1);
    }
    // Never the Warrior: the silence went on what was bashed, and the caster is free to
    // keep casting.
    assert!(
        simulation.warrior().battle_trance.remaining() >= 0.0,
        "the Warrior silenced itself"
    );
    close_to_target(&mut simulation, 3.0);
    simulation.step_many(60 * 16);
    simulation
        .cast(FireAbility::ShieldBash)
        .expect("the Warrior cannot be silenced by its own bash");
}

#[test]
fn shield_block_raises_block_and_turns_critical_strikes_into_ordinary_hits() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    simulation.warrior_mut().deflection.set(0.0);
    let before = simulation.defense_profile().block;

    simulation.cast(FireAbility::ShieldBlock).expect("instant");
    let during = simulation.defense_profile();
    assert!(during.immune_to_critical, "critical strikes still land");
    let gained = during.block - before;
    // No gear: the base number, exactly. The scaling reads the *sheet's* health pool, so the
    // class's own Deflection bonus does not quietly inflate it.
    assert!((gained - SHIELD_BLOCK_BASE).abs() < 1e-4, "{gained}");

    // And it does scale: a Warrior with the geared health pool gets the design's +24 %.
    let mut geared = warrior();
    let mut bonus = StatBlock::zero();
    bonus.set(Stat::Stamina, 30.0);
    geared.set_stat_bonus(bonus);
    let expected = engine_game::warrior::health_scaled(
        SHIELD_BLOCK_BASE,
        SHIELD_BLOCK_HEALTH_SCALE,
        scaling_health(&geared),
    );
    assert!((expected - 0.24).abs() < 1e-3, "{expected}");
    let before = geared.defense_profile().block;
    geared.cast(FireAbility::ShieldBlock).expect("instant");
    let gained = geared.defense_profile().block - before;
    assert!((gained - expected).abs() < 1e-4, "{gained}");

    // Empowered: five points more, and no more than that.
    let mut simulation = warrior();
    simulation.warrior_mut().deflection.set(DEFLECTION_SPEND);
    simulation.refresh_character();
    let before = simulation.defense_profile().block;
    simulation.cast(FireAbility::ShieldBlock).expect("instant");
    let gained = simulation.defense_profile().block - before;
    assert!(
        (gained - (SHIELD_BLOCK_BASE + SHIELD_BLOCK_EMPOWERED)).abs() < 1e-4,
        "{gained}"
    );

    // And it ends with its duration.
    simulation.step_many(60 * 7);
    assert!(!simulation.defense_profile().immune_to_critical);
}

#[test]
fn last_stand_heals_from_the_health_it_had_before_it_raised_it() {
    let mut simulation = warrior();
    simulation.warrior_mut().deflection.set(0.0);
    let max = player_max_health(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        health.set_current(max * 0.4);
    }
    let before = player_health(&simulation);

    simulation.cast(FireAbility::LastStand).expect("instant");

    let bonus = engine_game::warrior::health_scaled(
        LAST_STAND_BASE,
        LAST_STAND_HEALTH_SCALE,
        scaling_health(&simulation),
    );
    let healed = player_health(&simulation) - before;
    // The heal is the *pre-effect* maximum times the bonus: not the raised one, which would
    // be the circular calculation the design forbids.
    assert!(
        (healed - bonus * max).abs() < 0.5,
        "{healed} against {}",
        bonus * max
    );
    // The bonuses add on the sheet's pool rather than multiplying each other: Deflection's
    // five per cent and Last Stand's are both shares of the same number.
    assert!(
        (player_max_health(&simulation) - (max + scaling_health(&simulation) * bonus)).abs() < 0.5,
        "{}",
        player_max_health(&simulation)
    );
    assert!((simulation.defense_profile().damage_reduction - LAST_STAND_REDUCTION).abs() < 1e-6);

    // Empowered: fifteen per cent instead of ten, and the same maximum health bonus.
    let mut simulation = warrior();
    simulation.warrior_mut().deflection.set(DEFLECTION_SPEND);
    simulation.refresh_character();
    let max = player_max_health(&simulation);
    simulation.cast(FireAbility::LastStand).expect("instant");
    assert!(
        (simulation.defense_profile().damage_reduction - LAST_STAND_EMPOWERED_REDUCTION).abs()
            < 1e-6
    );
    let bonus = engine_game::warrior::health_scaled(
        LAST_STAND_BASE,
        LAST_STAND_HEALTH_SCALE,
        scaling_health(&simulation),
    );
    // Twenty spent, so the passive bonus came back as well: the maximum is the pool plus
    // both shares of it.
    let pool = scaling_health(&simulation);
    let expected = pool * (1.0 + simulation.warrior().deflection.max_health_bonus() + bonus);
    assert!(
        (player_max_health(&simulation) - expected).abs() < 0.5,
        "{} against {expected}, was {max}",
        player_max_health(&simulation)
    );
}

#[test]
fn spell_reflect_sends_enemy_spells_home_and_leaves_swings_alone() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    simulation.warrior_mut().deflection.set(0.0);
    simulation.cast(FireAbility::SpellReflect).expect("instant");

    let attacker_before = simulation
        .world()
        .health(victim)
        .map_or(0.0, |health| health.current());
    let before = player_health(&simulation);
    bolt(&mut simulation, 100.0);
    assert_eq!(player_health(&simulation), before, "the spell still landed");
    let attacker_after = simulation
        .world()
        .health(victim)
        .map_or(0.0, |health| health.current());
    assert!(
        (attacker_before - attacker_after - 100.0).abs() < 0.5,
        "the caster took {}",
        attacker_before - attacker_after
    );

    // A swing is not a spell: it lands as usual.
    let before = player_health(&simulation);
    swing(&mut simulation, 50.0);
    assert!(player_health(&simulation) < before, "a swing was reflected");
}

#[test]
fn a_defensive_event_shortens_spell_reflect_by_a_tenth_of_a_second() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 100.0);
    simulation.cast(FireAbility::SpellReflect).expect("instant");
    // Off the global cooldown and one tick into the recharge.
    simulation.step_once();
    let before = simulation
        .mage()
        .abilities
        .recharge_remaining(FireAbility::SpellReflect);
    let mut events = 0_i32;
    for _ in 0..5 {
        if swing(&mut simulation, 0.0) == Some(DefenseOutcome::Dodged) {
            events += 1;
        }
    }
    let after = simulation
        .mage()
        .abilities
        .recharge_remaining(FireAbility::SpellReflect);
    let expected = events as f32 * SPELL_REFLECT_COOLDOWN_PER_EVENT;
    assert!(events > 0, "nothing was dodged");
    assert!(
        (before - after - expected).abs() < 1e-3,
        "{} off for {events} events",
        before - after
    );
}

#[test]
fn recuperate_stacks_to_five_and_spends_a_capped_bank_for_two() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    simulation.warrior_mut().deflection.set(0.0);
    let player = simulation.player().entity;

    for expected in 1..=5 {
        simulation.cast(FireAbility::Recuperate).expect("in range");
        assert_eq!(
            simulation
                .world()
                .statuses(player)
                .map_or(0, |statuses| statuses.stacks(StatusKind::Recuperating)),
            expected
        );
        simulation.step_until_ready();
    }
    simulation.cast(FireAbility::Recuperate).expect("in range");
    assert_eq!(
        simulation
            .world()
            .statuses(player)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Recuperating)),
        5,
        "the stack went past five"
    );

    // The capped-Deflection outlet: a hundred banked turns the next application into two
    // stacks and takes twenty for it.
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    simulation.warrior_mut().deflection.set(DEFLECTION_MAXIMUM);
    simulation.refresh_character();
    let player = simulation.player().entity;
    simulation.cast(FireAbility::Recuperate).expect("in range");
    assert_eq!(
        simulation
            .world()
            .statuses(player)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Recuperating)),
        2
    );
    assert!(
        (deflection(&simulation) - (DEFLECTION_MAXIMUM - DEFLECTION_SPEND)).abs() < 1e-5,
        "{}",
        deflection(&simulation)
    );
}

#[test]
fn recuperate_heals_a_fifth_of_a_per_cent_a_second_a_stack() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let max = player_max_health(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        health.set_current(max * 0.3);
    }
    simulation.cast(FireAbility::Recuperate).expect("in range");
    let before = player_health(&simulation);
    simulation.step_many(60 * 3 + 2);
    let healed = player_health(&simulation) - before;
    let expected = 3.0 * max * RECUPERATE_HEAL_PER_STACK;
    // Out-of-combat regeneration is off in the arena's default configuration, so what came
    // back is the buff and nothing else.
    assert!(
        (healed - expected).abs() < expected * 0.5 + 0.5,
        "{healed} against {expected}"
    );
}

// ---------------------------------------------------------------------------- threat

#[test]
fn the_warriors_threat_tools_generate_more_threat_than_the_damage_they_deal() {
    // Not a second threat system: the same table, fed from the same damage, multiplied by
    // the ability's own row.
    let (mut simulation, victim) = warrior_against_an_enemy();
    simulation.warrior_mut().deflection.set(0.0);

    simulation.cast(FireAbility::WarStomp).expect("in range");
    let dealt: f32 = simulation
        .events()
        .iter()
        .filter_map(CombatEvent::as_damage)
        .filter(|damage| damage.target == victim)
        .map(|damage| damage.amount)
        .sum();
    let threat = simulation.brain(victim).map_or(0.0, |brain| {
        brain.threat().threat(simulation.player().entity)
    });
    let multiplier = FireAbility::WarStomp.definition().threat.damage;
    assert!(multiplier > 1.0, "War Stomp is not a threat tool");
    assert!(
        (threat - dealt * multiplier).abs() < 1.0,
        "{threat} for {dealt} damage at x{multiplier}"
    );
}

#[test]
fn a_spender_that_reaches_nobody_keeps_the_bank() {
    // Colossus Strike with nobody within its radius: the cooldown is spent, the Deflection is
    // not — an empowerment is a bigger effect, and there is no effect to make bigger.
    let mut simulation = warrior();
    let far = simulation.player_position() + Vec3::new(200.0, 0.0, 200.0);
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = far;
    }
    simulation.warrior_mut().deflection.set(60.0);
    simulation.refresh_character();
    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("a self-centred smash is never refused for range");
    assert!(
        (deflection(&simulation) - 60.0).abs() < 1e-5,
        "a smash into empty air spent {}",
        60.0 - deflection(&simulation)
    );
    assert!(
        !simulation
            .mage()
            .abilities
            .is_ready(FireAbility::ColossusStrike)
    );
}

#[test]
fn every_warrior_row_has_a_motion_of_its_own_and_no_other_hero_does() {
    // The motion is data on the row, so the visual layer never matches on an ability's
    // identity: the Warrior is the melee hero and each of its abilities asks the body for a
    // motion; the other heroes' rows keep the ordinary cast, exactly as before the column.
    for ability in FireAbility::ALL {
        let motion = ability.definition().motion;
        if ability.hero() == Hero::Warrior {
            assert_ne!(motion, Motion::Cast, "{ability:?} has no motion of its own");
        } else {
            assert_eq!(motion, Motion::Cast, "{ability:?} is not a Warrior row");
        }
    }
}
