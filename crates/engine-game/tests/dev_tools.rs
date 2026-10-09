//! The developer "teleport to the boss and kill it" control: it has to complete the run the
//! way a real kill does, from the far side of the sealed gate, with nothing skipped.
//!
//! Compiled only with the `dev-tools` feature, as the control itself is.

#![cfg(feature = "dev-tools")]
#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::dungeon::{DungeonScaling, GRAVEYARD_CRYPT_FILE, build_level_file};
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::world::EntityDesc;

fn crypt() -> Simulation {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(GRAVEYARD_CRYPT_FILE);
    build_level_file(path, DungeonScaling::default())
        .expect("the crypt builds")
        .0
}

fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

fn count(events: &[CombatEvent], pick: impl Fn(&CombatEvent) -> bool) -> usize {
    events.iter().filter(|event| pick(event)).count()
}

#[test]
fn the_dev_kill_completes_the_run_and_drops_one_roll_per_player() {
    let mut simulation = crypt();
    let first = simulation.player().entity;
    let second = simulation
        .world_mut()
        .spawn(EntityDesc::player(Vec3::new(0.0, 0.0, 0.0)));
    simulation.add_player(second, Hero::Warrior);
    simulation.step_once();

    assert!(simulation.gate_is_closed(), "the crypt starts sealed");
    let boss = simulation.dev_boss().expect("a boss to kill");

    assert!(simulation.dev_teleport_and_kill_boss());
    let events = run(&mut simulation, 5);

    assert!(!simulation.gate_is_closed(), "the gate was left standing");
    assert!(!simulation.world().is_alive(boss));
    assert_eq!(
        count(&events, |e| matches!(
            e,
            CombatEvent::DungeonCompleted { .. }
        )),
        1
    );
    assert_eq!(
        count(&events, |e| matches!(e, CombatEvent::ChestSpawned { .. })),
        1
    );
    assert!(simulation.dungeon_run().expect("a run").is_completed());
    let chest = simulation.loot_chest().expect("a chest");
    let at = chest.position();
    assert_eq!(chest.claims().len(), 2, "one roll per player");
    assert_eq!(simulation.chest_loot().count(), 2);
    assert!(simulation.dev_boss().is_none(), "nothing left to kill");
    assert!(
        !simulation.dev_teleport_and_kill_boss(),
        "a finished run has no boss to kill"
    );

    // The player stands beside where the boss fell, on the floor.
    let me = simulation.world().position(first).expect("a player");
    assert!(
        ((me.x - at.x).powi(2) + (me.z - at.z).powi(2)).sqrt() < 8.0,
        "the player is not beside the boss: {me:?} vs {at:?}"
    );
    assert!((me.y - at.y).abs() < 1.5);
}

#[test]
fn without_a_dungeon_there_is_no_boss() {
    let mut simulation =
        engine_game::arena::training_arena(&engine_game::arena::ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.dev_boss().is_none());
    assert!(!simulation.dev_teleport_and_kill_boss());
}
