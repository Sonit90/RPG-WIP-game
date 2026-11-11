//! One-off exporter: the Ashen Reliquary's Rust layout and packs, written as a level file.
//!
//! The reliquary was built in code ([`engine_environment::dungeon::ashen_reliquary`] plus
//! [`engine_game::dungeon::PACKS`]) before dungeons were data. This writes the same dungeon as
//! `assets/scenes/ashen_reliquary.json` so the realm and the editor can load it like the crypt.
//!
//! Run on demand, never in CI, because it rewrites a checked-in asset:
//! `cargo test -p engine-game --test export_ashen_reliquary -- --ignored`
//!
//! The other test is ordinary: it keeps the checked-in file honest against the Rust it came from.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::dungeon::ashen_reliquary;
use engine_environment::{
    BossGate, DungeonSettings, EnvironmentScene, MonsterPlacement, SpawnPoint,
};
use engine_game::dungeon::{BOSS_PACK, PACKS};

const FILE: &str = "scenes/ashen_reliquary.json";

fn path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(FILE)
}

/// The reliquary as a scene with its monsters and dungeon block attached.
fn exported() -> EnvironmentScene {
    let mut scene = ashen_reliquary();
    scene.push_spawn(SpawnPoint::new(engine_environment::dungeon::ENTRANCE));

    let mut required = 0.0;
    for (index, pack) in PACKS.iter().enumerate() {
        for spawn in pack.encounter().spawns() {
            let placement = MonsterPlacement::new(spawn.kind.id(), spawn.position)
                .with_yaw(std::f32::consts::PI)
                .in_pack(pack.name);
            scene.push_monster(placement);
            if index != BOSS_PACK {
                required += spawn.kind.archetype().completion;
            }
        }
    }

    let mut dungeon = DungeonSettings::new("THE ASHEN RELIQUARY", required);
    dungeon.boss_pack = Some(PACKS[BOSS_PACK].name.to_owned());
    // Across the corridor between the vault and the boss arena: its walls stand at x = ±8,
    // so a gate of that half-width closes it exactly.
    let corridor =
        (PACKS[BOSS_PACK - 1].area().north_edge() + PACKS[BOSS_PACK].area().south_edge()) * 0.5;
    dungeon.boss_gate = Some(BossGate {
        center: Vec3::new(0.0, 0.0, corridor),
        half_extents: Vec3::new(8.0, 3.0, 0.6),
        yaw: 0.0,
    });
    scene.dungeon = Some(dungeon);
    scene
}

#[test]
#[ignore = "rewrites assets/scenes/ashen_reliquary.json"]
fn write_the_level_file() {
    engine_environment::scene_file::save(&exported(), path()).expect("the level file is written");
}

#[test]
fn the_checked_in_file_matches_the_rust_layout() {
    let file =
        engine_environment::scene_file::load(path()).expect("the reliquary's level file loads");
    let rust = exported();
    assert_eq!(file.monsters.len(), rust.monsters.len());
    assert_eq!(file.objects.len(), rust.objects.len());
    assert_eq!(file.dungeon, rust.dungeon);
}

#[test]
fn the_reliquary_loads_like_the_crypt_and_has_a_boss() {
    let scene =
        engine_environment::scene_file::load(path()).expect("the reliquary's level file loads");
    let mut simulation = engine_game::sim::Simulation::new(engine_game::sim::SimConfig::default());
    let report = engine_game::dungeon::populate_from_scene(
        &mut simulation,
        &scene,
        engine_game::dungeon::DungeonScaling::default(),
    );
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(report.packs, PACKS.len());
    assert_eq!(
        report.monsters,
        PACKS.iter().map(|pack| pack.len()).sum::<usize>()
    );
    assert_eq!(report.boss_encounter, Some(BOSS_PACK));
    assert!(report.total_completion > 0.0);
}
