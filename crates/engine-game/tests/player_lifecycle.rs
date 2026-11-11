//! Dying, coming back, and the twenty seconds in between — through the real simulation.
//!
//! ```text
//!   F7 ──► kill_player ──► the ordinary damage path ──► death
//!                                                        │
//!                                    3 s                  ▼
//!                                          respawn at the level's spawn point
//!                                                        │
//!                                                        ▼
//!                                    "Cannot be respawned", 20 s
//!                                                        │
//!                          die again ──► no respawn until it expires
//! ```
//!
//! The unit tests in `sim::respawn` cover the clock on its own. These cover the thing the
//! clock is attached to: that a death goes through the shipping damage path, that the player
//! comes back *where the level says* rather than where they fell, that the protection is an
//! ordinary status the rest of the game can read, and that the edge cases the specification
//! names — repeated kills, a death inside the protection, a level with no spawn point at all —
//! do what they are supposed to instead of corrupting the player.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_environment::spawn::SpawnPoint;
use engine_game::sim::Simulation;
use engine_game::sim::respawn::{RESPAWN_DELAY, RESPAWN_PROTECTION};
use engine_game::status::StatusKind;

/// Ticks the simulation for `seconds` of simulated time.
fn run(simulation: &mut Simulation, seconds: f32) {
    let ticks = (seconds * 60.0).round() as u32;
    for _ in 0..ticks {
        simulation.step_once();
    }
}

/// A level with one spawn point, well away from the origin so "came back to the spawn" and
/// "never moved" cannot be the same answer.
fn level() -> EnvironmentScene {
    let mut scene = EnvironmentScene::empty("lifecycle");
    scene.push_spawn(SpawnPoint::new(Vec3::new(18.0, 0.0, -14.0)).with_yaw(0.75));
    scene
}

fn simulation_of(scene: &EnvironmentScene) -> Simulation {
    let mut simulation = engine_game::arena::environment_arena(scene, 0);
    simulation.step_once();
    simulation
}

/// Where the player is standing now.
fn player_position(simulation: &Simulation) -> Vec3 {
    simulation
        .world()
        .position(simulation.player().entity)
        .expect("the player is in the world")
}

/// Whether the player is carrying the respawn protection, and how long is left on it.
fn protection(simulation: &Simulation) -> Option<f32> {
    simulation
        .world()
        .statuses(simulation.player().entity)
        .and_then(|statuses| statuses.get(StatusKind::RespawnProtection))
        .map(|status| status.remaining)
}

#[test]
fn the_kill_key_kills_the_player_through_the_ordinary_death_path() {
    // Not a teleport and not a health write: it is `slay`, which is the same path a monster
    // takes, so everything a real death produces is produced.
    let mut simulation = simulation_of(&level());
    assert!(simulation.player_alive());

    simulation.kill_player();
    assert!(!simulation.player_alive(), "F7 did not kill the player");

    let died = simulation
        .events()
        .iter()
        .any(|event| matches!(event, engine_game::event::CombatEvent::Death { .. }));
    assert!(died, "the death produced no death event");
}

#[test]
fn a_killed_player_comes_back_at_the_levels_spawn_point_facing_the_right_way() {
    let mut simulation = simulation_of(&level());

    // Walked away from the spawn, so coming back is a move rather than standing still.
    let elsewhere = Vec3::new(-30.0, 0.0, 25.0);
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = elsewhere;
    }

    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);

    assert!(simulation.player_alive(), "the player never came back");
    let position = player_position(&simulation);
    assert!(
        (position.x - 18.0).abs() < 1.0 && (position.z + 14.0).abs() < 1.0,
        "the player came back at {position:?} rather than at the spawn point"
    );
    let facing = simulation
        .world()
        .body(simulation.player().entity)
        .expect("the player has a body")
        .facing_yaw;
    assert!((facing - 0.75).abs() < 1e-3, "faced {facing}");

    // Whole: a respawn that returned a player with no health would be a respawn into a
    // second death.
    let health = simulation
        .world()
        .health(simulation.player().entity)
        .expect("the player has health");
    assert_eq!(health.current(), health.max());
}

#[test]
fn the_protection_is_an_ordinary_status_the_rest_of_the_game_can_read() {
    let mut simulation = simulation_of(&level());
    assert_eq!(protection(&simulation), None, "protection before any death");

    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);

    let remaining = protection(&simulation).expect("the protection is on the player");
    assert!(
        remaining > RESPAWN_PROTECTION - 2.0 && remaining <= RESPAWN_PROTECTION,
        "{remaining} seconds of protection"
    );
    // The text the specification names, and the uppercase the interface draws.
    assert_eq!(
        StatusKind::RespawnProtection.display_name(),
        "Cannot be respawned"
    );
    assert_eq!(
        StatusKind::RespawnProtection.as_str(),
        "CANNOT BE RESPAWNED"
    );
}

#[test]
fn the_protection_expires_after_exactly_twenty_seconds_and_respawning_works_again() {
    let mut simulation = simulation_of(&level());
    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);
    assert!(protection(&simulation).is_some());

    // A second before the end it is still there; a second after it is gone.
    run(&mut simulation, RESPAWN_PROTECTION - 1.5);
    assert!(
        protection(&simulation).is_some(),
        "the protection ended early"
    );
    run(&mut simulation, 2.0);
    assert_eq!(
        protection(&simulation),
        None,
        "the protection outlived itself"
    );

    // And a death afterwards respawns normally.
    let before = simulation.respawn().count();
    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);
    assert!(simulation.player_alive());
    assert_eq!(simulation.respawn().count(), before + 1);
}

#[test]
fn a_death_during_the_protection_does_not_respawn_the_player() {
    // The requirement the twenty seconds exist for.
    let mut simulation = simulation_of(&level());
    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);
    assert_eq!(simulation.respawn().count(), 1);

    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 2.0);
    assert!(
        !simulation.player_alive(),
        "the player respawned inside their protection"
    );
    assert_eq!(simulation.respawn().count(), 1, "a second respawn happened");
    assert!(simulation.respawn().is_deferred());

    // And when it expires, the respawn that was owed happens.
    run(&mut simulation, RESPAWN_PROTECTION);
    assert!(simulation.player_alive(), "the owed respawn never came");
    assert_eq!(simulation.respawn().count(), 2);
}

#[test]
fn pressing_the_kill_key_over_and_over_does_not_corrupt_the_player() {
    // Repeated F7, including while already dead. It must produce one death and one respawn,
    // not a queue of them, and the player must end up alive and whole.
    let mut simulation = simulation_of(&level());
    for _ in 0..200 {
        simulation.kill_player();
        simulation.step_once();
    }
    // One respawn, not two hundred: every death after the first falls inside the protection
    // the first respawn started, and is held rather than granted.
    assert!(
        simulation.respawn().count() <= 1,
        "{} respawns from a held kill key",
        simulation.respawn().count()
    );

    // Let it all settle: the protection runs out, the owed respawn happens, and the player is
    // standing at the spawn point in one piece.
    run(&mut simulation, RESPAWN_DELAY + RESPAWN_PROTECTION + 2.0);
    assert!(simulation.player_alive(), "the player never recovered");
    let health = simulation
        .world()
        .health(simulation.player().entity)
        .expect("the player has health");
    assert_eq!(health.current(), health.max());
}

#[test]
fn a_level_with_no_spawn_point_still_respawns_the_player() {
    // The graceful case: no spawn point is not an error, and it must not be a crash or a
    // player who never comes back.
    let mut scene = EnvironmentScene::empty("no_spawn");
    scene.spawns.player = Vec3::new(4.0, 0.0, 9.0);
    let mut simulation = simulation_of(&scene);

    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);

    assert!(simulation.player_alive(), "the player never came back");
    let position = player_position(&simulation);
    assert!(
        (position.x - 4.0).abs() < 1.0 && (position.z - 9.0).abs() < 1.0,
        "the player came back at {position:?} rather than at the level's own start"
    );
}

#[test]
fn a_reset_takes_the_protection_and_the_owed_respawn_with_it() {
    // The level-reload case, through the mechanism this game already has for putting an
    // attempt back: twenty seconds of a previous attempt leaking into the next one would be a
    // player who cannot respawn in a fight they have only just started.
    let mut simulation = simulation_of(&level());
    simulation.kill_player();
    run(&mut simulation, RESPAWN_DELAY + 0.5);
    assert!(protection(&simulation).is_some());

    simulation.reset_encounter();
    simulation.step_once();

    assert!(simulation.player_alive());
    assert_eq!(
        protection(&simulation),
        None,
        "the protection survived a reset"
    );
    assert_eq!(simulation.respawn().count(), 0);
}
