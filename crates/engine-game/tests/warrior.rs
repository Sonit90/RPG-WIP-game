//! The Warrior, asserted against a real arena.
//!
//! What a unit test on Valor or on the defence roll cannot see: that a real cast of War Stomp,
//! Colossus Strike or Shield Charge pays exactly one Valor, that the fifth becomes a stack of
//! Battle Fervor, that a bleed applied at one maximum health ticks for another after Last
//! Stand, and that Puncture Blood's cost and its shield read two different numbers.
//!
//! Every attack in here goes through [`Simulation::deliver_attack`], which is the same
//! pipeline an enemy's swing takes — not a second one written for the tests.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::{AbilityId, Motion, TargetMode};
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

fn valor(simulation: &Simulation) -> u8 {
    simulation.warrior().valor.stacks()
}

/// The stacks of Battle Fervor on the player, or zero.
fn fervor_stacks(simulation: &Simulation) -> u8 {
    simulation
        .world()
        .statuses(simulation.player().entity)
        .map_or(0, |statuses| statuses.stacks(StatusKind::BattleFervor))
}

/// Seconds left on Battle Fervor, or zero.
fn fervor_remaining(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .statuses(simulation.player().entity)
        .map_or(0.0, |statuses| statuses.remaining(StatusKind::BattleFervor))
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

// ---------------------------------------------------------------------------- Valor

#[test]
fn a_fresh_run_starts_with_no_valor_and_no_fervor() {
    let simulation = warrior();
    assert_eq!(valor(&simulation), 0);
    assert_eq!(fervor_stacks(&simulation), 0);
}

#[test]
fn war_stomp_and_colossus_strike_each_pay_exactly_one_valor_per_cast() {
    for ability in [FireAbility::WarStomp, FireAbility::ColossusStrike] {
        let mut simulation = warrior();
        close_to_target(&mut simulation, 3.0);
        let hits = simulation.cast(ability).expect("in range");
        assert!(hits >= 1, "{ability:?} hit nothing");
        assert_eq!(valor(&simulation), 1, "{ability:?}");
        let gained = simulation
            .events()
            .iter()
            .filter(|event| matches!(event, CombatEvent::ValorGained { .. }))
            .count();
        assert_eq!(gained, 1, "{ability:?} announced {gained} gains");
    }
}

#[test]
fn a_cast_that_hits_several_enemies_still_pays_one_valor() {
    let mut simulation = warrior();
    let victim = target(&simulation);
    let Some(centre) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    let others: Vec<EntityId> = simulation
        .world()
        .entities()
        .filter(|id| *id != victim && simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .take(2)
        .collect();
    assert!(others.len() >= 2, "the arena has more than two dummies");
    for (index, other) in others.iter().enumerate() {
        if let Some(body) = simulation.world_mut().body_mut(*other) {
            body.position = centre + Vec3::new(1.0 + index as f32, 0.0, 0.0);
        }
    }
    close_to_target(&mut simulation, 3.0);
    let hits = simulation
        .cast(FireAbility::ColossusStrike)
        .expect("self-cast");
    assert!(hits >= 3, "the smash caught {hits}");
    assert_eq!(valor(&simulation), 1);
}

#[test]
fn shield_charge_pays_one_valor_on_arrival_and_not_before() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 12.0);
    simulation
        .cast(FireAbility::ShieldCharge)
        .expect("the charge was refused");
    assert_eq!(valor(&simulation), 0, "paid before the impact");
    assert!(simulation.warrior().charge.is_some(), "not running");
    for _ in 0..600 {
        if simulation.warrior().charge.is_none() {
            break;
        }
        simulation.step_once();
    }
    assert!(simulation.warrior().charge.is_none(), "never arrived");
    assert_eq!(valor(&simulation), 1);
}

#[test]
fn a_smash_into_empty_air_pays_nothing() {
    let mut simulation = warrior();
    let far = simulation.player_position() + Vec3::new(200.0, 0.0, 200.0);
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = far;
    }
    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("a self-centred smash is never refused for range");
    assert_eq!(valor(&simulation), 0);
    assert!(
        !simulation
            .mage()
            .abilities
            .is_ready(FireAbility::ColossusStrike),
        "the cooldown is spent either way"
    );
}

#[test]
fn nothing_else_in_the_kit_pays_valor() {
    for ability in [
        FireAbility::Recuperate,
        FireAbility::ShieldBash,
        FireAbility::ShieldBlock,
        FireAbility::PunctureBlood,
        FireAbility::Adrenaline,
        FireAbility::BattleTrance,
        FireAbility::SpellReflect,
        FireAbility::Taunt,
        FireAbility::LastStand,
    ] {
        assert_eq!(
            ability.definition().warrior().valor,
            0.0,
            "{ability:?} has a Valor column"
        );
        let mut simulation = warrior();
        close_to_target(&mut simulation, 3.0);
        simulation.cast(ability).expect("the cast was refused");
        assert_eq!(valor(&simulation), 0, "{ability:?} paid Valor");
    }
    for ability in [
        FireAbility::WarStomp,
        FireAbility::ColossusStrike,
        FireAbility::ShieldCharge,
    ] {
        assert_eq!(ability.definition().warrior().valor, 1.0, "{ability:?}");
    }
}

#[test]
fn defensive_events_pay_no_valor() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 30.0, 30.0, 30.0);
    let mut defended = 0;
    for _ in 0..200 {
        if swing(&mut simulation, 1.0).is_some_and(DefenseOutcome::is_defensive) {
            defended += 1;
        }
    }
    assert!(defended > 20, "only {defended} attacks were answered");
    assert_eq!(valor(&simulation), 0);
    assert_eq!(fervor_stacks(&simulation), 0);
}

#[test]
fn valor_never_decays() {
    let mut simulation = warrior();
    simulation.warrior_mut().valor.set(3);
    simulation.step_many(60 * 60);
    assert_eq!(valor(&simulation), 3);
}

// ---------------------------------------------------------------------------- Battle Fervor

#[test]
fn the_fifth_valor_empties_the_bar_and_grants_one_stack_of_fervor() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    for expected in 1..VALOR_MAX {
        simulation
            .cast(FireAbility::ColossusStrike)
            .expect("self-cast");
        assert_eq!(valor(&simulation), expected);
        assert_eq!(
            fervor_stacks(&simulation),
            0,
            "Fervor before the bar filled"
        );
        simulation.step_many(60 * 9);
        simulation.warrior_mut().valor.set(expected);
    }
    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("self-cast");
    assert_eq!(valor(&simulation), 0, "the bar did not empty");
    assert_eq!(fervor_stacks(&simulation), 1);
    assert!((fervor_remaining(&simulation) - FERVOR_DURATION).abs() < 0.1);
    assert!(simulation.events().iter().any(|event| matches!(
        event,
        CombatEvent::ValorGained {
            filled: true,
            value: 0,
            ..
        }
    )));
    assert!(simulation.events().iter().any(|event| matches!(
        event,
        CombatEvent::StatusApplied {
            status: StatusKind::BattleFervor,
            ..
        }
    )));
}

/// Fills the bar from four with one stomp, waiting out the cooldown first.
fn fill_the_bar(simulation: &mut Simulation) {
    simulation.step_many(60 * 5);
    simulation.warrior_mut().valor.set(VALOR_MAX - 1);
    simulation.cast(FireAbility::WarStomp).expect("in range");
}

#[test]
fn fervor_caps_at_three_stacks_and_every_gain_refreshes_the_duration() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);

    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), 1);

    // Ten seconds on, a second fill adds a stack and puts the clock back to thirty.
    simulation.step_many(60 * 10);
    assert!(fervor_remaining(&simulation) < FERVOR_DURATION - 9.0);
    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), 2);
    assert!(
        fervor_remaining(&simulation) > FERVOR_DURATION - 6.0,
        "{}",
        fervor_remaining(&simulation)
    );

    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), FERVOR_MAX_STACKS);

    // A fourth fill cannot raise the stacks, but it still refreshes the clock.
    simulation.step_many(60 * 10);
    let before = fervor_remaining(&simulation);
    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), FERVOR_MAX_STACKS);
    assert!(
        fervor_remaining(&simulation) > before + 4.0,
        "{before} then {}",
        fervor_remaining(&simulation)
    );
    assert_eq!(valor(&simulation), 0);
}

#[test]
fn fervor_expires_after_thirty_seconds() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), 1);

    simulation.step_many(60 * 29);
    assert_eq!(fervor_stacks(&simulation), 1, "it ended early");
    simulation.step_many(60 * 2);
    assert_eq!(fervor_stacks(&simulation), 0, "it outlived its duration");
}

#[test]
fn each_stack_of_fervor_is_ten_per_cent_dealt_and_three_per_cent_taken() {
    let mut simulation = warrior();
    let player = simulation.player().entity;
    let statuses = |simulation: &Simulation| {
        simulation
            .world()
            .statuses(player)
            .map(|set| {
                (
                    set.damage_dealt_multiplier(),
                    set.damage_taken_multiplier(DamageType::Physical),
                    set.damage_taken_multiplier(DamageType::Fire),
                    set.max_health_multiplier(),
                )
            })
            .expect("the player has statuses")
    };
    assert_eq!(statuses(&simulation), (1.0, 1.0, 1.0, 1.0));

    close_to_target(&mut simulation, 3.0);
    for stacks in 1..=FERVOR_MAX_STACKS {
        fill_the_bar(&mut simulation);
        assert_eq!(fervor_stacks(&simulation), stacks);
        let (dealt, physical, fire, health) = statuses(&simulation);
        let n = f32::from(stacks);
        assert!((dealt - (1.0 + 0.10 * n)).abs() < 1e-5, "{dealt}");
        assert!((physical - (1.0 - 0.03 * n)).abs() < 1e-5, "{physical}");
        assert!((fire - (1.0 - 0.03 * n)).abs() < 1e-5, "{fire}");
        assert!(
            (health - (1.0 + FERVOR_MAX_HEALTH_PER_STACK * n)).abs() < 1e-5,
            "{health}"
        );
    }
}

#[test]
fn fervor_really_changes_the_damage_the_warrior_deals_and_takes() {
    let mut plain = warrior();
    set_avoidance(&mut plain, 0.0, 0.0, 0.0);
    let before = player_health(&plain);
    bolt(&mut plain, 100.0);
    let plain_taken = before - player_health(&plain);

    let mut buffed = warrior();
    set_avoidance(&mut buffed, 0.0, 0.0, 0.0);
    close_to_target(&mut buffed, 3.0);
    fill_the_bar(&mut buffed);
    assert_eq!(fervor_stacks(&buffed), 1);
    let before = player_health(&buffed);
    bolt(&mut buffed, 100.0);
    let taken = before - player_health(&buffed);
    assert!(
        (taken / plain_taken - (1.0 - FERVOR_DAMAGE_TAKEN_PER_STACK)).abs() < 1e-3,
        "{taken} against {plain_taken}"
    );
}

#[test]
fn fervor_is_gone_when_the_warrior_dies() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    fill_the_bar(&mut simulation);
    assert_eq!(fervor_stacks(&simulation), 1);
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
    bolt(&mut simulation, 1.0e9);
    assert_eq!(fervor_stacks(&simulation), 0, "a corpse kept its buff");
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

// ---------------------------------------------------------------------------- Colossus Strike

#[test]
fn the_colossus_bleed_reads_maximum_health_on_every_tick_rather_than_snapshotting_it() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
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

// ---------------------------------------------------------------------------- Adrenaline

#[test]
fn adrenaline_banks_its_share_and_pays_it_back_over_the_recovery() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
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
        simulation.step_many(60 * 4 + 2);
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
    let with_stomp = simulation.defense_profile().armor;
    // War Stomp's bonus is a share of the armour the sheet grants.
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

// ---------------------------------------------------------------------------- the rest of the kit

#[test]
fn shield_bash_hits_and_interrupts_without_silencing() {
    let (mut simulation, victim) = warrior_against_an_enemy();
    let before = simulation
        .world()
        .health(victim)
        .map_or(0.0, |health| health.current());
    simulation.cast(FireAbility::ShieldBash).expect("in range");
    let after = simulation
        .world()
        .health(victim)
        .map_or(0.0, |health| health.current());
    assert!(after < before, "the bash did no damage");
    assert!(
        simulation
            .brain(victim)
            .is_none_or(|brain| !brain.is_silenced()),
        "a bash silenced"
    );
}

#[test]
fn shield_block_raises_block_and_turns_critical_strikes_into_ordinary_hits() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
    let before = simulation.defense_profile().block;

    simulation.cast(FireAbility::ShieldBlock).expect("instant");
    let during = simulation.defense_profile();
    assert!(during.immune_to_critical, "critical strikes still land");
    let gained = during.block - before;
    // No gear: the base number, exactly. The scaling reads the *sheet's* health pool, so the
    // class's own Last Stand does not quietly inflate it.
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

    // And it ends with its duration.
    let mut simulation = warrior();
    simulation.cast(FireAbility::ShieldBlock).expect("instant");
    simulation.step_many(60 * 7);
    assert!(!simulation.defense_profile().immune_to_critical);
}

#[test]
fn last_stand_heals_from_the_health_it_had_before_it_raised_it() {
    let mut simulation = warrior();
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
    // The bonus is a share of the sheet's pool, not of the raised maximum.
    assert!(
        (player_max_health(&simulation) - (max + scaling_health(&simulation) * bonus)).abs() < 0.5,
        "{}",
        player_max_health(&simulation)
    );
    assert!((simulation.defense_profile().damage_reduction - LAST_STAND_REDUCTION).abs() < 1e-6);
}

#[test]
fn spell_reflect_sends_enemy_spells_home_and_leaves_swings_alone() {
    let mut simulation = warrior();
    set_avoidance(&mut simulation, 0.0, 0.0, 0.0);
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
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
fn recuperate_stacks_to_five_and_pays_no_valor() {
    let mut simulation = warrior();
    close_to_target(&mut simulation, 3.0);
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
    assert_eq!(valor(&simulation), 0);
    simulation.cast(FireAbility::Recuperate).expect("in range");
    assert_eq!(
        simulation
            .world()
            .statuses(player)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Recuperating)),
        5,
        "the stack went past five"
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
fn every_warrior_row_has_a_motion_of_its_own_and_no_other_hero_does() {
    // The motion is data on the row, so the visual layer never matches on an ability's
    // identity: the Warrior is the melee hero and each of its abilities asks the body for a
    // motion; the other heroes' rows keep the ordinary cast, exactly as before the column.
    for ability in FireAbility::ALL {
        let motion = ability.definition().motion;
        if ability.hero() == Hero::Warrior {
            assert_ne!(motion, Motion::Cast, "{ability:?} has no motion of its own");
        } else if ability.hero() == Hero::Druid
            && ability.definition().target == TargetMode::Friendly
        {
            // The Druid's heals are a flavour of cast, not a motion of their own.
            assert_eq!(motion, Motion::Heal, "{ability:?} is a Druid heal");
        } else {
            assert_eq!(motion, Motion::Cast, "{ability:?} is not a Warrior row");
        }
    }
}
