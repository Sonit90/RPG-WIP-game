//! Collision as the game uses it: through commands, ticks and the simulation.
//!
//! The collision crate's own suites prove the geometry and the controller. This one proves
//! the *wiring*: that the player the simulation moves is the one the collision world stops,
//! that a training dummy is not an obstacle, and that the collision demo scene can be
//! walked around headlessly — with no window, no device and no renderer in the process.
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::demo;
use engine_game::sim::Simulation;
use engine_game::world::EntityDesc;

/// Pushes a movement intent and runs `ticks` fixed steps.
fn walk(simulation: &mut Simulation, direction: Vec3, ticks: u32) {
    for _ in 0..ticks {
        simulation.push_command(PlayerCommand::Move {
            forward: -direction.z,
            right: direction.x,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_once();
    }
}

/// Where the player's feet are.
fn feet(simulation: &Simulation) -> Vec3 {
    simulation
        .player_body()
        .map_or(Vec3::ZERO, |body| body.position)
}

#[test]
fn the_training_arena_has_a_floor_and_four_walls_made_of_collision_geometry() {
    let simulation = arena::training_arena(&ArenaConfig::default());
    let world = simulation.collision_world();

    assert_eq!(world.len(), 5, "a floor and four walls");
    assert!(
        !world.is_dirty(),
        "the arena handed over a stale broad phase"
    );
    let labels: Vec<&str> = world.statics().map(|id| world.label(id)).collect();
    assert!(labels.iter().any(|label| label.contains("FLOOR")));
    assert_eq!(
        labels.iter().filter(|label| label.contains("WALL")).count(),
        4
    );
}

#[test]
fn the_player_is_stopped_by_the_arena_wall_rather_than_clamped_at_it() {
    let config = ArenaConfig::default();
    let mut simulation = arena::training_arena(&config);
    walk(&mut simulation, Vec3::X, 900);

    let position = feet(&simulation);
    assert!(
        position.x < config.arena_half_extent,
        "it reached the clamp instead of the wall: {position:?}"
    );
    assert!(
        position.x > config.arena_half_extent - 2.0,
        "it stopped far short of the wall: {position:?}"
    );

    // And the wall does not hold it: walking back has to work immediately.
    walk(&mut simulation, -Vec3::X, 120);
    assert!(feet(&simulation).x < position.x - 1.0);
}

#[test]
fn running_diagonally_at_an_arena_wall_slides_along_it() {
    let config = ArenaConfig::default();
    let mut simulation = arena::training_arena(&config);
    walk(&mut simulation, Vec3::new(1.0, 0.0, -1.0), 900);

    let position = feet(&simulation);
    assert!(position.x < config.arena_half_extent, "{position:?}");
    assert!(
        position.z < -20.0,
        "it stopped dead instead of sliding: {position:?}"
    );
}

#[test]
fn a_training_dummy_is_a_target_and_not_an_obstacle() {
    // The decision recorded in `collision.md`: dummies do not block movement. A dummy the
    // player can get wedged against is a dummy that ruins the fight it exists for.
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let dummy = ArenaConfig::default().single_dummy;

    walk(&mut simulation, -Vec3::Z, 180);
    let position = feet(&simulation);
    assert!(
        position.z < dummy.z - 1.0,
        "the player was blocked by a dummy at {dummy:?}: {position:?}"
    );
}

#[test]
fn the_player_still_falls_and_lands_inside_the_arena() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = Vec3::new(0.0, 20.0, 0.0);
        body.grounded = false;
    }

    let mut lowest = f32::MAX;
    for _ in 0..240 {
        simulation.step_once();
        lowest = lowest.min(feet(&simulation).y);
    }

    assert!(lowest > -0.05, "it fell through the floor to {lowest}");
    assert!(
        simulation
            .player_body()
            .expect("the player exists")
            .grounded
    );
}

#[test]
fn combat_still_works_with_collision_in_the_way() {
    // The regression that matters: collision must not have broken the thing the game is
    // about. One full rotation against the nearest dummy, from a standing start.
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    let target = simulation.player().target.expect("a dummy is targeted");
    let before = simulation
        .world()
        .health(target)
        .map(engine_game::combat::Health::current)
        .expect("the dummy has health");

    for slot in 0..3 {
        if let Some(ability) = engine_game::ability::AbilityId::from_slot(slot) {
            simulation.push_command(PlayerCommand::UseAbility(ability.slot()));
        }
        simulation.step_many(120);
    }

    let after = simulation
        .world()
        .health(target)
        .map(engine_game::combat::Health::current)
        .expect("the dummy still exists");
    assert!(after < before, "no damage landed: {before} -> {after}");
    assert!(simulation.meter().total_damage() > 0.0, "the meter saw it");
}

#[test]
fn the_collision_demo_scene_runs_headlessly_with_no_combat_in_it() {
    let (mut simulation, scene) = demo::collision_demo();
    assert_eq!(simulation.world().len(), 1);
    assert!(simulation.collision_world().len() > 20, "the course exists");

    walk(&mut simulation, Vec3::X, 300);
    assert!(feet(&simulation).x > 1.0, "the character did not move");
    assert_eq!(
        simulation.events().len(),
        0,
        "a combat event in a combat-free scene"
    );
    assert_eq!(scene.stations.len(), 7);
}

#[test]
fn every_station_in_the_demo_scene_can_be_stood_at_and_walked_away_from() {
    let (mut simulation, scene) = demo::collision_demo();

    for station in &scene.stations {
        simulation.push_command(PlayerCommand::Teleport {
            position: station.spawn,
        });
        simulation.step_once();

        let arrived = feet(&simulation);
        assert!(
            (arrived - station.spawn).length() < 1.0,
            "{}: teleported to {arrived:?} rather than {:?}",
            station.name,
            station.spawn
        );

        walk(&mut simulation, Vec3::X, 120);
        let after = feet(&simulation);
        assert!(
            after.y > demo::VOID_Y,
            "{}: the character left the world at {after:?}",
            station.name
        );
    }
}

#[test]
fn the_demo_scenes_wall_stops_the_character_and_its_steps_do_not() {
    let (mut simulation, scene) = demo::collision_demo();

    let wall = scene.station(1).expect("station 1 is the wall");
    simulation.push_command(PlayerCommand::Teleport {
        position: wall.spawn,
    });
    simulation.step_once();
    walk(&mut simulation, Vec3::X, 300);
    assert!(
        feet(&simulation).x < wall.spawn.x + 6.0,
        "the wall did not stop it: {:?}",
        feet(&simulation)
    );

    let steps = scene.station(3).expect("station 3 is the steps");
    simulation.push_command(PlayerCommand::Teleport {
        position: steps.spawn,
    });
    simulation.step_once();
    let mut highest = 0.0_f32;
    for _ in 0..600 {
        walk(&mut simulation, Vec3::X, 1);
        highest = highest.max(feet(&simulation).y);
    }
    assert!(
        highest > 0.4,
        "the character never climbed the steps: reached {highest}"
    );
    assert!(
        highest < 0.8,
        "it climbed the 0.90 m obstacle that is meant to stop it: {highest}"
    );
}

#[test]
fn the_demo_scenes_slopes_are_walkable_up_to_the_limit_and_not_past_it() {
    let (mut simulation, scene) = demo::collision_demo();
    let slopes = scene.station(4).expect("station 4 is the slopes");

    // The 15-degree ramp is at z = -9, the 70-degree one at z = +9.
    for (z, climbs) in [(-9.0_f32, true), (9.0, false)] {
        simulation.push_command(PlayerCommand::Teleport {
            position: Vec3::new(slopes.spawn.x, slopes.spawn.y, z),
        });
        simulation.step_once();
        // Stopped part way up rather than run to the end: the ramps are ten metres long,
        // and a character that walks off the top lands back on the platform looking exactly
        // like one that never climbed.
        for _ in 0..240 {
            walk(&mut simulation, Vec3::X, 1);
            if feet(&simulation).x > 84.0 {
                break;
            }
        }

        let height = feet(&simulation).y;
        if climbs {
            assert!(height > 0.5, "the gentle ramp was not climbed: {height}");
        } else {
            assert!(height < 0.5, "the 70-degree ramp was climbed: {height}");
        }
    }
}

#[test]
fn walking_off_the_demo_scenes_ledge_falls_and_lands() {
    let (mut simulation, scene) = demo::collision_demo();
    let drop = scene.station(5).expect("station 5 is the drop");
    simulation.push_command(PlayerCommand::Teleport {
        position: drop.spawn,
    });
    simulation.step_once();
    let start = feet(&simulation).y;
    assert!(start > 3.0, "it did not start on the ledge: {start}");

    let mut airborne = false;
    for _ in 0..240 {
        walk(&mut simulation, Vec3::X, 1);
        airborne |= !simulation
            .player_body()
            .expect("the player exists")
            .grounded;
    }

    assert!(airborne, "it never left the ledge");
    let landed = feet(&simulation);
    assert!(landed.y < start - 3.0, "it did not fall: {landed:?}");
    assert!(
        simulation
            .player_body()
            .expect("the player exists")
            .grounded,
        "it never landed"
    );
}

#[test]
fn the_demo_scenes_thin_slab_catches_a_terminal_velocity_fall() {
    let (mut simulation, scene) = demo::collision_demo();
    let fast = scene.station(6).expect("station 6 is the high-speed drop");
    simulation.push_command(PlayerCommand::Teleport {
        position: fast.spawn,
    });
    simulation.step_once();

    for _ in 0..600 {
        simulation.step_once();
        if simulation
            .player_body()
            .expect("the player exists")
            .grounded
        {
            break;
        }
    }

    let landed = feet(&simulation);
    assert!(
        landed.y > 7.9,
        "it tunnelled through the 5 cm slab to {landed:?}"
    );
}

#[test]
fn a_scene_with_extra_static_geometry_still_moves_the_player_deterministically() {
    // Two identical runs of the same commands in the same world: the collision world is
    // part of the simulation, so determinism has to survive it.
    let run = || {
        let (mut simulation, _) = demo::collision_demo();
        for tick in 0..600 {
            let right = ((tick % 13) as f32 - 6.0) / 6.0;
            let forward = ((tick % 7) as f32 - 3.0) / 3.0;
            simulation.push_command(PlayerCommand::Move {
                forward,
                right,
                yaw: 0.4,
                face_camera: false,
            });
            if tick % 47 == 0 {
                simulation.push_command(PlayerCommand::Jump);
            }
            simulation.step_once();
        }
        simulation.player_body().copied()
    };
    assert_eq!(run(), run());
    assert!(run().is_some());
}

#[test]
fn a_body_with_no_collision_world_still_moves() {
    // The dummies' path: `movement::step` with no world at all. It has to keep working,
    // because a dummy is integrated with it 1,000 times a tick in the benchmark.
    let mut simulation = Simulation::new(engine_game::sim::SimConfig::default());
    let player = simulation
        .world_mut()
        .spawn(EntityDesc::player(Vec3::new(0.0, 5.0, 0.0)));
    simulation.set_player_entity(player);
    simulation.step_many(300);

    let position = feet(&simulation);
    assert!(
        position.y < 0.1,
        "with no floor to collide with it should have fallen: {position:?}"
    );
}
