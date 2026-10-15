//! The Warrior's eight talents, each observed in a real simulation.
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
use engine_game::combat::Faction;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{AbilityError, FireAbility};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::talent::warrior::{WarriorTalent, WarriorTalentEffects};
use engine_game::talent::{TalentGrid, TalentKey};
use engine_game::warrior::tuning::*;
use engine_game::world::{EntityDesc, EntityKind};

fn warrior_with(talents: &[WarriorTalent]) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.set_hero(Hero::Warrior));
    simulation.set_health_regen_per_second(0.0);
    simulation.select_nearest_target();
    simulation.set_talents(TalentGrid::from_talents(talents));
    simulation
}

fn target(simulation: &Simulation) -> EntityId {
    simulation.player().target.expect("a dummy is selected")
}

fn close_to_target(simulation: &mut Simulation, metres: f32) {
    let victim = target(simulation);
    let destination = simulation.world().position(victim).expect("position");
    let player = simulation.player().entity;
    let from = simulation.world().position(player).expect("position");
    let mut offset = from - destination;
    offset.y = 0.0;
    let length = offset.length().max(1e-3);
    let placed = destination + offset * (metres / length);
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = placed;
    }
}

fn valor(simulation: &Simulation) -> u8 {
    simulation.warrior().valor.stacks()
}

fn fervor_stacks(simulation: &Simulation) -> u8 {
    simulation
        .world()
        .statuses(simulation.player().entity)
        .map_or(0, |statuses| statuses.stacks(StatusKind::BattleFervor))
}

fn stacks(simulation: &Simulation, id: EntityId, kind: StatusKind) -> u8 {
    simulation
        .world()
        .statuses(id)
        .map_or(0, |statuses| statuses.stacks(kind))
}

/// Damage the player's `ability` dealt in the events the simulation currently holds.
fn damage_of(simulation: &Simulation, ability: FireAbility, periodic: bool) -> f32 {
    let id = AbilityId::from_fire(ability);
    simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.ability == id && damage.periodic == periodic => {
                Some(damage.amount + damage.overkill)
            }
            _ => None,
        })
        .sum()
}

/// Wounds `victim` to `fraction` of its health.
fn wound(simulation: &mut Simulation, victim: EntityId, fraction: f32) {
    if let Some(health) = simulation.world_mut().health_mut(victim) {
        let keep = health.max() * fraction;
        let lose = (health.current() - keep).max(0.0);
        health.damage(lose);
    }
}

/// Tops the bar up to one short of full and casts a War Stomp, which fills it.
fn fill_the_bar(simulation: &mut Simulation) {
    simulation.step_many(60 * 5);
    let capacity = simulation.warrior().valor.capacity();
    simulation.warrior_mut().valor.set(capacity - 1);
    simulation.cast(FireAbility::WarStomp).expect("in range");
}

/// Moves every dummy far away, so a test can place its own.
fn clear_the_arena(simulation: &mut Simulation) {
    let dummies: Vec<EntityId> = simulation
        .world()
        .entities()
        .filter(|id| simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .collect();
    for (index, dummy) in dummies.into_iter().enumerate() {
        if let Some(body) = simulation.world_mut().body_mut(dummy) {
            body.position = Vec3::new(500.0 + index as f32 * 20.0, 0.0, 500.0);
        }
    }
}

fn spawn_dummy(simulation: &mut Simulation, position: Vec3) -> EntityId {
    simulation.world_mut().spawn(EntityDesc {
        kind: EntityKind::Dummy,
        faction: Faction::Hostile,
        position,
        max_health: 5_000_000.0,
        ..EntityDesc::default()
    })
}

// ---------------------------------------------------------------------------- the table

#[test]
fn the_warrior_has_eight_talents_priced_by_tier() {
    let costs: Vec<u32> = WarriorTalent::ALL.iter().map(|t| t.spec().cost).collect();
    assert_eq!(costs, vec![3, 3, 4, 4, 4, 4, 5, 5]);
    let tiers: Vec<u8> = WarriorTalent::ALL.iter().map(|t| t.spec().tier).collect();
    assert_eq!(tiers, vec![1, 1, 2, 2, 3, 3, 4, 4]);
    for talent in WarriorTalent::ALL {
        assert!(!talent.spec().description.contains('{'), "{talent:?}");
    }
}

#[test]
fn the_talents_are_stored_on_the_warrior_and_derive_their_effects() {
    let simulation = warrior_with(&[WarriorTalent::Shockwave]);
    let grid = simulation.talent_grid();
    assert_eq!(grid.hero(), Hero::Warrior);
    assert!(grid.has(WarriorTalent::Shockwave));
    assert_eq!(
        *simulation.warrior_effects(),
        WarriorTalentEffects::from_grid(&grid)
    );
    let mut simulation = simulation;
    simulation.reset_talents();
    assert_eq!(
        *simulation.warrior_effects(),
        WarriorTalentEffects::default()
    );
}

// ---------------------------------------------------------------------------- tier 1

#[test]
fn unbroken_momentum_turns_four_valor_into_fervor() {
    let mut plain = warrior_with(&[]);
    close_to_target(&mut plain, 3.0);
    plain.warrior_mut().valor.set(3);
    plain.cast(FireAbility::WarStomp).expect("in range");
    assert_eq!((valor(&plain), fervor_stacks(&plain)), (4, 0));

    let mut quick = warrior_with(&[WarriorTalent::UnbrokenMomentum]);
    close_to_target(&mut quick, 3.0);
    assert_eq!(quick.warrior().valor.capacity(), 4);
    quick.warrior_mut().valor.set(3);
    quick.cast(FireAbility::WarStomp).expect("in range");
    assert_eq!((valor(&quick), fervor_stacks(&quick)), (0, 1));
}

#[test]
fn shockwave_widens_war_stomp_and_stacks_sundered_faster() {
    let reach = FireAbility::WarStomp.definition().range;
    for (talents, expect_reach) in [(vec![], false), (vec![WarriorTalent::Shockwave], true)] {
        let mut simulation = warrior_with(&talents);
        close_to_target(&mut simulation, reach * 1.2);
        let result = simulation.cast(FireAbility::WarStomp);
        if expect_reach {
            assert!(result.is_ok_and(|hits| hits >= 1), "{result:?}");
        } else {
            assert_eq!(result, Err(AbilityError::OutOfRange));
        }
    }

    let stomp = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        let victim = simulation.spawn_enemy(
            engine_game::enemy::EnemyKind::Boss,
            simulation.player_position() + Vec3::new(0.0, 0.0, -3.0),
            0.0,
        );
        simulation.player_mut().target = Some(victim);
        simulation.cast(FireAbility::WarStomp).expect("in range");
        stacks(&simulation, victim, StatusKind::Sundered)
    };
    assert_eq!(stomp(&[]), 1);
    assert_eq!(stomp(&[WarriorTalent::Shockwave]), 2);
}

#[test]
fn shockwave_softens_colossus_strike_by_a_tenth() {
    let hit = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        simulation
            .cast(FireAbility::ColossusStrike)
            .expect("self-cast");
        damage_of(&simulation, FireAbility::ColossusStrike, false)
    };
    let ratio = hit(&[WarriorTalent::Shockwave]) / hit(&[]);
    assert!((ratio - 0.9).abs() < 0.02, "{ratio}");
}

// ---------------------------------------------------------------------------- tier 2

#[test]
fn executioner_hits_a_nearly_dead_target_harder_and_slows_the_cooldown() {
    let hit = |talents: &[WarriorTalent], health: f32| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        let victim = target(&simulation);
        wound(&mut simulation, victim, health);
        simulation
            .cast(FireAbility::ColossusStrike)
            .expect("self-cast");
        let impact = damage_of(&simulation, FireAbility::ColossusStrike, false);
        // The first tick of the bleed, two seconds on.
        simulation.step_many(60 * 2 + 5);
        let tick = damage_of(&simulation, FireAbility::ColossusStrike, true);
        let recharge = simulation
            .mage()
            .abilities
            .recharge_remaining(FireAbility::ColossusStrike);
        (impact, tick, recharge)
    };
    let (plain_impact, plain_tick, plain_cooldown) = hit(&[], 0.30);
    let (exec_impact, exec_tick, exec_cooldown) = hit(&[WarriorTalent::Executioner], 0.30);
    assert!((exec_impact / plain_impact - 1.6).abs() < 0.02);
    assert!(plain_tick > 0.0 && (exec_tick / plain_tick - 1.6).abs() < 0.02);
    let cooldown = FireAbility::ColossusStrike.definition().cooldown;
    // Six seconds in (two of them past the impact) the plain strike has this much to go.
    assert!(
        (exec_cooldown - plain_cooldown - 2.0).abs() < 0.05,
        "{plain_cooldown} then {exec_cooldown} of {cooldown}"
    );

    // Above the threshold nothing changes but the cooldown.
    let (healthy_plain, ..) = hit(&[], 0.90);
    let (healthy_exec, ..) = hit(&[WarriorTalent::Executioner], 0.90);
    assert!((healthy_exec / healthy_plain - 1.0).abs() < 0.01);
}

#[test]
fn bloodied_valor_pays_a_stack_when_the_bleed_runs_out() {
    let after_bleed = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        simulation
            .cast(FireAbility::ColossusStrike)
            .expect("self-cast");
        assert_eq!(valor(&simulation), 1);
        // Seven seconds in: the bleed has ticked three times and is not over.
        simulation.step_many(60 * 7);
        let midway = valor(&simulation);
        simulation.step_many(60 * 2);
        (midway, valor(&simulation))
    };
    assert_eq!(after_bleed(&[]), (1, 1));
    assert_eq!(after_bleed(&[WarriorTalent::BloodiedValor]), (1, 2));
}

#[test]
fn a_bleed_that_is_cut_short_pays_no_bloodied_valor() {
    let mut simulation = warrior_with(&[WarriorTalent::BloodiedValor]);
    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    simulation
        .cast(FireAbility::ColossusStrike)
        .expect("self-cast");
    simulation.step_many(60 * 3);
    if let Some(statuses) = simulation.world_mut().statuses_mut(victim) {
        statuses.consume(StatusKind::Bleeding);
    }
    simulation.step_many(60 * 7);
    assert_eq!(valor(&simulation), 1);
}

// ---------------------------------------------------------------------------- tier 3

#[test]
fn fervent_guard_trades_damage_dealt_for_damage_taken() {
    let numbers = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        fill_the_bar(&mut simulation);
        assert_eq!(fervor_stacks(&simulation), 1);
        let stats = simulation
            .effect_stats(simulation.player().entity)
            .expect("the player has stats");
        let physical = stats
            .damage_taken
            .iter()
            .find(|(kind, _)| *kind == engine_game::combat::DamageType::Physical)
            .map(|(_, multiplier)| *multiplier)
            .expect("physical");
        (stats.damage_dealt, physical)
    };
    let (plain_dealt, plain_taken) = numbers(&[]);
    let (guard_dealt, guard_taken) = numbers(&[WarriorTalent::FerventGuard]);
    assert!((plain_dealt - 1.10).abs() < 1e-4 && (plain_taken - 0.97).abs() < 1e-4);
    assert!((guard_dealt - 1.06).abs() < 1e-4 && (guard_taken - 0.94).abs() < 1e-4);
}

#[test]
fn fervent_guard_changes_the_damage_the_pipeline_actually_deals() {
    let hit = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        fill_the_bar(&mut simulation);
        simulation.step_until_ready();
        simulation.step_many(60 * 2);
        simulation.cast(FireAbility::Recuperate).expect("in reach");
        damage_of(&simulation, FireAbility::Recuperate, false)
    };
    let ratio = hit(&[WarriorTalent::FerventGuard]) / hit(&[]);
    assert!((ratio - 1.06 / 1.10).abs() < 0.01, "{ratio}");
}

#[test]
fn relentless_charge_holds_two_charges_and_hits_softer() {
    let mut plain = warrior_with(&[]);
    close_to_target(&mut plain, 12.0);
    plain.cast(FireAbility::ShieldCharge).expect("first charge");
    plain.step_many(60 * 2);
    assert_eq!(
        plain.cast(FireAbility::ShieldCharge),
        Err(AbilityError::OnCooldown)
    );

    let mut twin = warrior_with(&[WarriorTalent::RelentlessCharge]);
    assert_eq!(twin.mage().abilities.charges(FireAbility::ShieldCharge), 2);
    close_to_target(&mut twin, 12.0);
    twin.cast(FireAbility::ShieldCharge).expect("first charge");
    twin.step_many(60 * 2);
    close_to_target(&mut twin, 12.0);
    twin.cast(FireAbility::ShieldCharge)
        .expect("a second charge is waiting");

    let impact = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 12.0);
        simulation.cast(FireAbility::ShieldCharge).expect("charge");
        let mut total = 0.0;
        for _ in 0..180 {
            simulation.step_once();
            total += damage_of(&simulation, FireAbility::ShieldCharge, false);
            if simulation.warrior().charge.is_none() && total > 0.0 {
                break;
            }
        }
        total
    };
    let ratio = impact(&[WarriorTalent::RelentlessCharge]) / impact(&[]);
    assert!((ratio - 0.75).abs() < 0.02, "{ratio}");
}

// ---------------------------------------------------------------------------- tier 4

#[test]
fn bulwark_rhythm_cuts_shield_block_for_every_fervor_stack_gained() {
    let remaining = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        simulation
            .cast(FireAbility::ShieldBlock)
            .expect("self-cast");
        fill_the_bar(&mut simulation);
        assert_eq!(fervor_stacks(&simulation), 1);
        simulation
            .mage()
            .abilities
            .recharge_remaining(FireAbility::ShieldBlock)
    };
    let cut = remaining(&[]) - remaining(&[WarriorTalent::BulwarkRhythm]);
    assert!((cut - 5.0).abs() < 0.05, "{cut}");
}

#[test]
fn unyielding_grants_two_fervor_stacks_and_lengthens_last_stand() {
    let stand = |talents: &[WarriorTalent]| {
        let mut simulation = warrior_with(talents);
        close_to_target(&mut simulation, 3.0);
        simulation.cast(FireAbility::LastStand).expect("self-cast");
        (
            fervor_stacks(&simulation),
            simulation
                .mage()
                .abilities
                .recharge_remaining(FireAbility::LastStand),
        )
    };
    let (plain_stacks, plain_cooldown) = stand(&[]);
    let (stacks, cooldown) = stand(&[WarriorTalent::Unyielding]);
    assert_eq!(plain_stacks, 0);
    assert_eq!(stacks, 2);
    assert!((cooldown - plain_cooldown - 30.0).abs() < 0.05);
}

// ---------------------------------------------------------------------------- the bleed cap

#[test]
fn colossus_strikes_bleed_lands_on_three_targets_and_its_hit_on_all() {
    let mut simulation = warrior_with(&[]);
    clear_the_arena(&mut simulation);
    let origin = simulation.player_position();
    // Five dummies in reach, the selected one the farthest.
    let near: Vec<EntityId> = [2.0, 3.0, 4.0, 5.0, 6.0]
        .into_iter()
        .map(|distance| spawn_dummy(&mut simulation, origin + Vec3::new(distance, 0.0, 0.0)))
        .collect();
    let primary = near[4];
    simulation.player_mut().target = Some(primary);
    let hits = simulation
        .cast(FireAbility::ColossusStrike)
        .expect("self-cast");
    assert_eq!(
        hits as usize, COLOSSUS_BLEED_MAX_TARGETS,
        "bleeding targets"
    );

    let bleeding: Vec<bool> = near
        .iter()
        .map(|id| stacks(&simulation, *id, StatusKind::Bleeding) > 0)
        .collect();
    assert_eq!(bleeding, vec![true, true, false, false, true]);

    // The smash itself reached all five.
    let struck = simulation
        .events()
        .iter()
        .filter(|event| {
            matches!(event, CombatEvent::Damage(damage)
                if damage.ability == AbilityId::from_fire(FireAbility::ColossusStrike)
                    && near.contains(&damage.target))
        })
        .count();
    assert_eq!(struck, 5);
}
