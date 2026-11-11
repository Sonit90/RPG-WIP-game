//! The dungeon's progress panel reads the run, and says what the design says it says.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_game::dungeon::{DungeonScaling, GRAVEYARD_CRYPT_FILE, build_level_file};
use engine_game::sim::Simulation;
use engine_presentation::game_ui::GameUi;
use engine_presentation::nameplate::NameplateConfig;
use engine_presentation::snapshot::UiSnapshot;

fn crypt() -> Simulation {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(GRAVEYARD_CRYPT_FILE);
    let (mut simulation, _) =
        build_level_file(path, DungeonScaling::default()).expect("the crypt builds");
    simulation.step_once();
    simulation
}

fn drawn_text(simulation: &Simulation) -> String {
    let snapshot = UiSnapshot::build(simulation);
    let mut ui = GameUi::new();
    ui.build(&snapshot, &[], &NameplateConfig::default(), (1920, 1080));
    ui.list().text().to_owned()
}

#[test]
fn the_panel_names_the_dungeon_and_its_requirement() {
    let simulation = crypt();
    let snapshot = UiSnapshot::build(&simulation);
    let dungeon = snapshot.dungeon.as_ref().expect("a dungeon view");
    assert_eq!(dungeon.name, "GRAVEYARD CRYPT");
    assert_eq!(dungeon.percent, 0);
    let text = drawn_text(&simulation);
    assert!(text.contains("GRAVEYARD CRYPT"), "{text}");
    assert!(text.contains("COMPLETION"), "{text}");
    assert!(text.contains("0%"), "{text}");
    assert!(text.contains("REQUIRED: 100%"), "{text}");
}

#[test]
fn at_one_hundred_per_cent_the_panel_says_the_way_is_open() {
    let mut simulation = crypt();
    // Every pack but the boss: far more than required, and clamped at a hundred.
    let boss = simulation
        .dungeon_run()
        .and_then(|run| run.rules().boss_encounter)
        .expect("a boss");
    let members: Vec<_> = simulation
        .encounters()
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != boss)
        .flat_map(|(_, encounter)| encounter.members().to_vec())
        .collect();
    for member in members {
        simulation.slay(member);
    }
    simulation.step_once();
    let text = drawn_text(&simulation);
    assert!(text.contains("100%"), "{text}");
    assert!(
        text.contains("THE PATH TO THE CRYPT LORD IS OPEN"),
        "{text}"
    );
}

#[test]
fn a_level_that_is_not_a_dungeon_draws_no_panel() {
    let simulation =
        engine_game::arena::training_arena(&engine_game::arena::ArenaConfig::default());
    assert!(UiSnapshot::build(&simulation).dungeon.is_none());
    assert!(!drawn_text(&simulation).contains("COMPLETION"));
}
