//! Monsters and walls: going round them, and not shooting through them.
//!
//! Each test builds the training arena's floor, puts one monster on one side of a wall and
//! the player on the other, and lets the fight run in fixed ticks.

#![allow(
    clippy::expect_used,
    reason = "integration tests may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::Simulation;
use engine_game::world::horizontal_distance;

/// A monster `distance` metres in front of an unkillable player, and a wall between them
/// from `x = -10` to `x = gap_at`, with open floor past its end.
fn walled_duel(kind: EnemyKind, distance: f32, gap_at: f32) -> (Simulation, EntityId) {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("WALLED", vec![EnemySpawn::new(kind, spawn)]);
    let mut simulation = arena::encounter_arena(&config, encounter);
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    let wall_z = config.player_spawn.z - distance * 0.5;
    let world = simulation.collision_world_mut();
    world.add_box(
        Vec3::new(config.player_spawn.x - 10.0, 0.0, wall_z - 0.4),
        Vec3::new(config.player_spawn.x + gap_at, 4.0, wall_z + 0.4),
        "TEST WALL",
    );
    world.rebuild_broad_phase();
    let enemy = simulation
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    (simulation, enemy)
}

fn damage_from(events: &[CombatEvent], source: EntityId) -> f32 {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.source == source => Some(damage.amount),
            _ => None,
        })
        .sum()
}

#[test]
fn a_melee_monster_walks_round_the_end_of_a_wall_to_reach_the_player() {
    let (mut simulation, ghoul) = walled_duel(EnemyKind::CryptGhoul, 7.0, 4.0);
    let player = simulation.player().entity;
    let mut went_round = false;
    let mut dealt = 0.0;
    for _ in 0..(60 * 12) {
        simulation.step_once();
        dealt += damage_from(simulation.events(), ghoul);
        let position = simulation.world().position(ghoul).expect("a ghoul");
        went_round |= position.x > 4.0;
    }
    let gap = horizontal_distance(
        simulation.world().position(ghoul).expect("a ghoul"),
        simulation.world().position(player).expect("a player"),
    );
    assert!(went_round, "the ghoul never went round the end of the wall");
    // Centre to centre: melee reach is measured edge to edge, and two bodies are a metre wide.
    assert!(gap < 4.0, "the ghoul is still {gap:.1} m away");
    assert!(dealt > 0.0, "the ghoul never hit the player");
}

#[test]
fn a_caster_does_not_shoot_through_a_wall_and_comes_round_to_see() {
    let (mut simulation, caster) = walled_duel(EnemyKind::SkeletonBonecaster, 9.0, 4.0);
    let player = simulation.player().entity;
    let mut through_wall = 0.0;
    let mut in_sight = 0.0;
    for _ in 0..(60 * 15) {
        simulation.step_once();
        let dealt = damage_from(simulation.events(), caster);
        let visible = engine_game::los::has_line_of_sight(
            simulation.collision_world(),
            simulation.world(),
            caster,
            player,
        );
        if visible {
            in_sight += dealt;
        } else {
            through_wall += dealt;
        }
    }
    assert_eq!(through_wall, 0.0, "damage landed through the wall");
    assert!(
        in_sight > 0.0,
        "the caster never came round to where it could see"
    );
}

#[test]
fn an_enemy_cast_on_a_victim_who_steps_behind_a_wall_is_lost() {
    use engine_game::sim::CastEnd;
    // No wall yet: the caster sees the player and starts casting.
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -9.0);
    let encounter = Encounter::with_spawns(
        "LOS",
        vec![EnemySpawn::new(EnemyKind::SkeletonBonecaster, spawn)],
    );
    let mut simulation = arena::encounter_arena(&config, encounter);
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    let caster = simulation
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("a caster");
    let mut casting = None;
    for _ in 0..(60 * 20) {
        simulation.step_once();
        if let Some(cast) = simulation
            .brain(caster)
            .and_then(|brain| brain.cast())
            .filter(|cast| !cast.shape.is_area() && cast.target == player)
        {
            casting = Some(cast.ability);
            break;
        }
    }
    let bolt = casting.expect("the caster never started a single-target cast");
    // A wall drops between them mid-cast.
    let world = simulation.collision_world_mut();
    world.add_box(
        Vec3::new(
            config.player_spawn.x - 20.0,
            0.0,
            config.player_spawn.z - 4.9,
        ),
        Vec3::new(
            config.player_spawn.x + 20.0,
            4.0,
            config.player_spawn.z - 4.1,
        ),
        "SUDDEN WALL",
    );
    world.rebuild_broad_phase();
    let mut lost = false;
    for _ in 0..(60 * 4) {
        simulation.step_once();
        lost |= simulation.events().iter().any(|event| {
            matches!(
                event,
                CombatEvent::CastEnded { source, end: CastEnd::LineOfSightLost, .. }
                    if *source == caster
            )
        });
        // The bolt in the air does not land. (A circle already telegraphed on the floor
        // on this side of the wall still does: it was placed while the caster could see.)
        assert!(
            !simulation.events().iter().any(|event| matches!(
                event,
                CombatEvent::Damage(damage)
                    if damage.source == caster && damage.ability.enemy() == Some(bolt)
            )),
            "the bolt went through the wall"
        );
    }
    assert!(lost, "the cast was not reported lost to the wall");
}

#[test]
fn the_crypt_can_be_walked_between_its_packs() {
    use engine_game::dungeon::{DungeonScaling, GRAVEYARD_CRYPT_FILE, build_level_file};
    use engine_game::nav::Navigator;
    // The shipped level, as the game builds it: walls, props, ramps and the boss gate.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(GRAVEYARD_CRYPT_FILE);
    let (simulation, _) =
        build_level_file(path, DungeonScaling::default()).expect("the crypt builds");
    let homes: Vec<Vec3> = simulation
        .enemies()
        .map(|(_, brain)| brain.home())
        .collect();
    let mut navigator = Navigator::default();
    let mut corners = Vec::new();
    let (mut asked, mut found) = (0, 0);
    for (index, from) in homes.iter().enumerate() {
        for to in homes.iter().skip(index + 1) {
            let distance = horizontal_distance(*from, *to);
            // Further than a pull drags a monster, and near enough to be one fight.
            if !(20.0..=70.0).contains(&distance) {
                continue;
            }
            asked += 1;
            if navigator.find_path(simulation.collision_world(), *from, *to, &mut corners) {
                found += 1;
                // Every leg of every way found is one a body can walk.
                let mut previous = *from;
                for corner in &corners {
                    assert!(
                        navigator.walkable_line(simulation.collision_world(), previous, *corner),
                        "{previous:?} -> {corner:?}"
                    );
                    previous = *corner;
                }
            }
        }
    }
    assert!(
        asked > 50,
        "the level has too few packs to say anything: {asked}"
    );
    // A few packs are behind the sealed boss gate, which is exactly what a gate is for.
    assert!(
        found * 100 >= asked * 90,
        "only {found} of {asked} routes between packs were found"
    );
}
