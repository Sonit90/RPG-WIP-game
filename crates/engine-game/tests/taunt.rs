//! Taunt, pressed the way a player presses it: the button beside the bar, on the selected
//! enemy, with no second click.
//!
//! The regression this guards: `F` used to open the Ultimate's placement circle, and the click
//! that followed cast Last Stand. The key now issues Taunt's own command
//! ([`Hero::command_for`]), and gameplay decides whether the selected target is valid, within
//! thirty metres and in line of sight.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::{CombatEvent, ControlKind};
use engine_game::firemage::{AbilityError, FireAbility};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;

/// A Warrior with an enemy `metres` straight *behind* them, selected.
///
/// Behind on purpose: a taunt is pointed at the selection, and the tank is not asked to turn
/// round first.
fn warrior_with_enemy_behind(metres: f32) -> (Simulation, EntityId) {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.set_hero(Hero::Warrior));
    let origin = simulation.player_position();
    simulation.face(origin + Vec3::new(0.0, 0.0, -10.0));
    let enemy = simulation.spawn_enemy(EnemyKind::Boss, origin + Vec3::new(0.0, 0.0, metres), 0.0);
    simulation.player_mut().target = Some(enemy);
    (simulation, enemy)
}

fn taunted(simulation: &Simulation, enemy: EntityId) -> bool {
    simulation
        .brain(enemy)
        .is_some_and(|brain| brain.threat().taunted().is_some())
}

/// Presses the button beside the bar that Taunt sits on, exactly as the key does.
fn press_taunt(simulation: &mut Simulation) {
    let caster = simulation.player_position();
    let command = Hero::Warrior
        .command_for(FireAbility::Taunt, None, caster)
        .expect("the Warrior has Taunt");
    assert_eq!(
        command,
        PlayerCommand::UseExtra(0),
        "Taunt is not its own button"
    );
    simulation.push_command(command);
    simulation.step_once();
}

#[test]
fn the_taunt_key_taunts_the_selected_enemy_with_no_second_click() {
    let (mut simulation, enemy) = warrior_with_enemy_behind(28.0);
    press_taunt(&mut simulation);
    assert!(
        simulation.events().iter().any(|event| matches!(
            *event,
            CombatEvent::ControlApplied { target, control: ControlKind::Taunt, .. } if target == enemy
        )) || taunted(&simulation, enemy),
        "the selected enemy was not taunted: {:?}",
        simulation.recent_error()
    );
    assert!(taunted(&simulation, enemy));
    assert_eq!(simulation.recent_error(), None);
    // And it was Taunt, not the Ultimate the old circle used to cast.
    assert!(
        simulation.warrior().last_stand_bonus <= 0.0,
        "Last Stand was cast"
    );
}

#[test]
fn taunt_reaches_thirty_metres_and_no_further() {
    assert!((FireAbility::Taunt.definition().range - 30.0).abs() < f32::EPSILON);

    let (mut simulation, enemy) = warrior_with_enemy_behind(29.5);
    assert!(simulation.cast(FireAbility::Taunt).is_ok());
    assert!(taunted(&simulation, enemy));

    // Measured edge to edge, like every range in the game, so "beyond thirty" is thirty plus
    // both bodies.
    let (mut simulation, enemy) = warrior_with_enemy_behind(40.0);
    assert_eq!(
        simulation.cast(FireAbility::Taunt),
        Err(AbilityError::OutOfRange)
    );
    assert!(!taunted(&simulation, enemy));
    assert!(
        simulation.mage().abilities.is_ready(FireAbility::Taunt),
        "a refused taunt spent its cooldown"
    );
}

#[test]
fn taunt_needs_line_of_sight() {
    let (mut simulation, enemy) = warrior_with_enemy_behind(20.0);
    let origin = simulation.player_position();
    let collision = simulation.collision_world_mut();
    collision.add_box(
        origin + Vec3::new(-6.0, -1.0, 9.5),
        origin + Vec3::new(6.0, 6.0, 10.5),
        "TAUNT TEST WALL",
    );
    collision.rebuild_broad_phase();
    assert_eq!(
        simulation.cast(FireAbility::Taunt),
        Err(AbilityError::NoLineOfSight)
    );
    assert!(!taunted(&simulation, enemy));
}

#[test]
fn taunt_without_a_valid_target_is_refused() {
    let (mut simulation, _) = warrior_with_enemy_behind(10.0);
    simulation.player_mut().target = None;
    assert_eq!(
        simulation.cast(FireAbility::Taunt),
        Err(AbilityError::NoTarget)
    );

    let (mut simulation, _) = warrior_with_enemy_behind(10.0);
    let me = simulation.player().entity;
    simulation.player_mut().target = Some(me);
    assert_eq!(
        simulation.cast(FireAbility::Taunt),
        Err(AbilityError::TargetFriendly)
    );
}
