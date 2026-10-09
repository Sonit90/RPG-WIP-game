//! Versatility in the fight: damage and healing done scale by `1 + v`, damage taken by
//! `1 - v / 2`, with `v = x / (x + 100)` read from the acting player's own sheet.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::character::{Stat, StatBlock};
use engine_game::combat::DamageType;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::world::EntityKind;

/// A rating whose fraction is exactly one half: `100 / (100 + 100)`.
const HALF: f32 = 100.0;

fn arena_sim() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation
}

fn with_versatility(simulation: &mut Simulation, rating: f32) {
    simulation.set_stat_bonus(StatBlock::of(Stat::Versatility, rating));
    simulation.refresh_character();
}

/// The amount of the last damage event.
fn last_damage(simulation: &Simulation) -> f32 {
    simulation
        .events()
        .iter()
        .rev()
        .find_map(|event| event.as_damage().map(|damage| damage.amount))
        .expect("a damage event")
}

fn dealt_by_player(rating: f32) -> f32 {
    let mut simulation = arena_sim();
    with_versatility(&mut simulation, rating);
    let player = simulation.player().entity;
    let dummy = arena::dummies(simulation.world()).next().expect("a dummy");
    simulation.apply_damage(player, dummy, 100.0, AbilityId::NONE, DamageType::Fire);
    last_damage(&simulation)
}

fn taken_by_player(rating: f32) -> f32 {
    let mut simulation = arena_sim();
    with_versatility(&mut simulation, rating);
    let player = simulation.player().entity;
    simulation.apply_damage(
        EntityId::INVALID,
        player,
        100.0,
        AbilityId::NONE,
        DamageType::Fire,
    );
    last_damage(&simulation)
}

#[test]
fn damage_done_is_raised_by_versatility() {
    let base = dealt_by_player(0.0);
    let boosted = dealt_by_player(HALF);
    assert!((boosted / base - 1.5).abs() < 1e-3, "{base} -> {boosted}");
}

#[test]
fn damage_taken_is_reduced_by_half_of_versatility() {
    let base = taken_by_player(0.0);
    let reduced = taken_by_player(HALF);
    assert!((reduced / base - 0.75).abs() < 1e-3, "{base} -> {reduced}");
}

#[test]
fn damage_done_to_a_dummy_is_not_reduced_by_the_players_versatility() {
    // The taken rule belongs to the one being hit: a dummy has no sheet.
    let base = dealt_by_player(0.0);
    let boosted = dealt_by_player(HALF);
    assert!(boosted > base);
}

fn nourish_amount(rating: f32) -> f32 {
    let mut simulation = arena_sim();
    assert!(simulation.set_hero(Hero::Druid));
    with_versatility(&mut simulation, rating);
    let ally = simulation
        .world()
        .entities()
        .find(|id| simulation.world().kind(*id) == Some(EntityKind::HealingDummy))
        .expect("a healing dummy");
    if let Some(health) = simulation.world_mut().health_mut(ally) {
        let current = health.current();
        health.damage(current * 0.5);
    }
    simulation.player_mut().target = Some(ally);
    simulation.druid_mut().mana.restore();
    simulation.step_until_ready();
    simulation
        .cast(FireAbility::Nourish)
        .expect("nourish is cast");
    for _ in 0..600 {
        if simulation.casting().is_none() {
            break;
        }
        simulation.step_once();
    }
    simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Healing { amount, .. } => Some(*amount),
            _ => None,
        })
        .sum()
}

#[test]
fn healing_done_is_raised_by_versatility() {
    let base = nourish_amount(0.0);
    let boosted = nourish_amount(HALF);
    assert!(base > 0.0);
    assert!((boosted / base - 1.5).abs() < 1e-3, "{base} -> {boosted}");
}

#[test]
fn each_player_uses_their_own_versatility() {
    use engine_game::world::EntityDesc;
    let mut simulation = arena_sim();
    let first = simulation.player().entity;
    let config = ArenaConfig::default();
    let second = simulation
        .world_mut()
        .spawn(EntityDesc::player(config.player_spawn));
    simulation.add_player(second, Hero::Warrior);
    // Only the second player has any.
    simulation.with_player(second, |sim| with_versatility(sim, HALF));

    let dummy = arena::dummies(simulation.world()).next().expect("a dummy");
    simulation.apply_damage(first, dummy, 100.0, AbilityId::NONE, DamageType::Fire);
    let by_first = last_damage(&simulation);
    simulation.apply_damage(second, dummy, 100.0, AbilityId::NONE, DamageType::Fire);
    let by_second = last_damage(&simulation);
    assert!(
        (by_second / by_first - 1.5).abs() < 1e-3,
        "{by_first} {by_second}"
    );

    simulation.apply_damage(
        EntityId::INVALID,
        first,
        100.0,
        AbilityId::NONE,
        DamageType::Fire,
    );
    let taken_first = last_damage(&simulation);
    simulation.apply_damage(
        EntityId::INVALID,
        second,
        100.0,
        AbilityId::NONE,
        DamageType::Fire,
    );
    let taken_second = last_damage(&simulation);
    assert!(
        (taken_second / taken_first - 0.75).abs() < 1e-3,
        "{taken_first} {taken_second}"
    );
}
