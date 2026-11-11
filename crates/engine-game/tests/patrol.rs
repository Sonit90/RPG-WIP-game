//! Patrols: an idle monster with waypoints walks its loop, and is still a pack that aggros.

#![allow(
    clippy::expect_used,
    reason = "integration tests may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::EnemyKind;
use engine_game::enemy::brain::AiState;
use engine_game::entity::EntityId;
use engine_game::sim::Simulation;
use engine_game::world::horizontal_distance;

/// A patrolling ghoul far from the player, and the loop it walks.
fn patrol_arena() -> (Simulation, EntityId, Vec<Vec3>) {
    let config = ArenaConfig::with_group(0);
    let home = config.player_spawn + Vec3::new(0.0, 0.0, -30.0);
    let encounter =
        Encounter::with_spawns("PATROL", vec![EnemySpawn::new(EnemyKind::CryptGhoul, home)]);
    let mut simulation = arena::encounter_arena(&config, encounter);
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    let ghoul = simulation
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    let loop_points = vec![
        home + Vec3::new(6.0, 0.0, 0.0),
        home + Vec3::new(6.0, 0.0, -6.0),
        home + Vec3::new(-6.0, 0.0, -6.0),
        home + Vec3::new(-6.0, 0.0, 0.0),
    ];
    simulation.set_enemy_patrol(ghoul, loop_points.clone());
    (simulation, ghoul, loop_points)
}

#[test]
fn an_idle_monster_walks_its_waypoints_in_order() {
    let (mut simulation, ghoul, points) = patrol_arena();
    let mut reached = vec![false; points.len()];
    for _ in 0..(60 * 40) {
        simulation.step_once();
        let position = simulation.world().position(ghoul).expect("a ghoul");
        for (done, point) in reached.iter_mut().zip(&points) {
            *done |= horizontal_distance(position, *point) < 1.5;
        }
    }
    assert!(reached.iter().all(|done| *done), "visited {reached:?}");
    let brain = simulation.brain(ghoul).expect("a brain");
    assert_eq!(
        brain.state(),
        AiState::Idle,
        "nobody was near enough to wake it"
    );
}

#[test]
fn a_patrolling_monster_aggros_a_player_who_comes_within_its_radius() {
    let (mut simulation, ghoul, _) = patrol_arena();
    for _ in 0..120 {
        simulation.step_once();
    }
    let player = simulation.player().entity;
    let at = simulation.world().position(ghoul).expect("a ghoul");
    let radius = simulation.brain(ghoul).expect("a brain").aggro_radius();
    // Within its radius from where it walks now, not from where it was placed.
    simulation
        .world_mut()
        .body_mut(player)
        .expect("a player body")
        .position = at + Vec3::new(0.0, 0.0, radius * 0.5);
    for _ in 0..30 {
        simulation.step_once();
    }
    let state = simulation.brain(ghoul).expect("a brain").state();
    assert_ne!(
        state,
        AiState::Idle,
        "the patroller never noticed the player"
    );
}

#[test]
fn after_a_reset_it_resumes_its_patrol() {
    let (mut simulation, ghoul, points) = patrol_arena();
    let player = simulation.player().entity;
    let at = simulation.world().position(ghoul).expect("a ghoul");
    simulation
        .world_mut()
        .body_mut(player)
        .expect("a player body")
        .position = at + Vec3::new(0.0, 0.0, 3.0);
    for _ in 0..60 {
        simulation.step_once();
    }
    assert_ne!(
        simulation.brain(ghoul).expect("a brain").state(),
        AiState::Idle
    );
    simulation.reset_encounter();
    // Reset puts the player back at the start, far from the loop.
    let brain = simulation.brain(ghoul).expect("a brain");
    assert_eq!(brain.state(), AiState::Idle);
    assert_eq!(brain.patrol().len(), points.len());
    let start = simulation.world().position(ghoul).expect("a ghoul");
    for _ in 0..(60 * 6) {
        simulation.step_once();
    }
    let later = simulation.world().position(ghoul).expect("a ghoul");
    assert!(
        horizontal_distance(start, later) > 2.0,
        "it stood still after the reset"
    );
}

#[test]
fn a_level_placement_with_waypoints_becomes_a_patrolling_monster() {
    use engine_environment::{EnvironmentScene, MonsterPlacement};
    let mut scene = EnvironmentScene::empty("patrol");
    let mut placement = MonsterPlacement::new("crypt_ghoul", Vec3::new(0.0, 0.0, -30.0));
    placement.patrol = vec![[0.0, -30.0], [6.0, -30.0]];
    scene.push_monster(placement);
    scene.push_monster(MonsterPlacement::new(
        "crypt_ghoul",
        Vec3::new(20.0, 0.0, -30.0),
    ));
    let mut simulation = Simulation::new(engine_game::sim::SimConfig::default());
    let report = engine_game::dungeon::populate_from_scene(
        &mut simulation,
        &scene,
        engine_game::dungeon::DungeonScaling::default(),
    );
    assert_eq!(report.monsters, 2);
    let patrolling = simulation
        .encounters()
        .iter()
        .flat_map(|encounter| encounter.members().to_vec())
        .filter(|entity| {
            !simulation
                .brain(*entity)
                .expect("a brain")
                .patrol()
                .is_empty()
        })
        .count();
    assert_eq!(patrolling, 1);
}
