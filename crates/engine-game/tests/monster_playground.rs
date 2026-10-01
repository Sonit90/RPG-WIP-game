//! The Monster Combat Playground, headless: choosing, spawning, fighting and resetting
//! against the real simulation in the training arena the playground scene is built on.

#![allow(
    clippy::expect_used,
    reason = "integration tests may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{FireAbility, TalentEffects};
use engine_game::playground::{
    HealthMode, MonsterPlayground, MonsterSelection, PlaygroundConfig, formation,
};
use engine_game::sim::Simulation;
use engine_game::status::{Status, StatusKind};
use engine_game::world::horizontal_distance;

/// The playground scene's arena: the training floor with its one dummy.
fn arena() -> Simulation {
    arena::training_arena(&ArenaConfig::with_group(0))
}

/// A playground installed on a fresh arena.
fn playground() -> (Simulation, MonsterPlayground) {
    let mut simulation = arena();
    let mut playground = MonsterPlayground::default();
    playground.install(&mut simulation);
    (simulation, playground)
}

fn spawn(simulation: &mut Simulation, playground: &mut MonsterPlayground) -> usize {
    let origin = simulation.player_position();
    playground.spawn_selected(simulation, origin, 0.0)
}

fn choose(playground: &mut MonsterPlayground, kinds: &[EnemyKind]) {
    let selection = playground.selection_mut();
    selection.clear();
    for kind in kinds {
        selection.select(*kind);
    }
}

/// Every pair of members, edge to edge, is at least `gap` apart.
fn assert_apart(simulation: &Simulation, ids: &[EntityId], gap: f32) {
    let world = simulation.world();
    for (index, a) in ids.iter().enumerate() {
        for b in &ids[index + 1..] {
            let distance = horizontal_distance(
                world.position(*a).expect("a member"),
                world.position(*b).expect("a member"),
            );
            let needed = world.radius(*a) + world.radius(*b) + gap - 1e-3;
            assert!(
                distance >= needed,
                "{a:?} and {b:?} are {distance:.2} m apart; they need {needed:.2}"
            );
        }
    }
}

fn members(playground: &MonsterPlayground) -> Vec<EntityId> {
    playground
        .encounter()
        .members()
        .iter()
        .map(|member| member.entity)
        .collect()
}

// ---------------------------------------------------------------------------------------
// Selection

#[test]
fn one_monster_can_be_selected() {
    let mut selection = MonsterSelection::new();
    assert!(selection.select(EnemyKind::CryptArcher));
    assert!(
        !selection.select(EnemyKind::CryptArcher),
        "selecting twice is once"
    );
    assert_eq!(selection.len(), 1);
    assert!(selection.contains(EnemyKind::CryptArcher));
}

#[test]
fn several_monsters_can_be_selected_in_order() {
    let mut selection = MonsterSelection::new();
    selection.select(EnemyKind::CryptArcher);
    selection.select(EnemyKind::SkeletonWarrior);
    selection.toggle(EnemyKind::GraveBrute);
    assert_eq!(
        selection.kinds(),
        [
            EnemyKind::CryptArcher,
            EnemyKind::SkeletonWarrior,
            EnemyKind::GraveBrute
        ]
    );
}

#[test]
fn a_monster_can_be_deselected_and_the_selection_cleared() {
    let mut selection = MonsterSelection::new();
    selection.select(EnemyKind::CryptArcher);
    selection.select(EnemyKind::SkeletonWarrior);
    assert!(selection.deselect(EnemyKind::CryptArcher));
    assert!(!selection.deselect(EnemyKind::CryptArcher));
    assert!(!selection.toggle(EnemyKind::SkeletonWarrior), "toggled off");
    assert!(selection.is_empty());
    selection.select(EnemyKind::GraveBrute);
    selection.select(EnemyKind::CryptGhoul);
    selection.clear();
    assert!(selection.is_empty());
}

// ---------------------------------------------------------------------------------------
// Formation

#[test]
fn a_formation_never_overlaps_and_stands_in_front_facing_the_origin() {
    let config = PlaygroundConfig::default();
    let origin = Vec3::new(3.0, 0.0, -2.0);
    for yaw in [0.0_f32, 1.1, -2.5] {
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        for count in 1..=20 {
            let radii: Vec<f32> = (0..count)
                .map(|index| [0.5, 0.9, 0.6, 1.4][index % 4])
                .collect();
            let slots = formation(origin, yaw, 0.5, &radii, &config);
            assert_eq!(slots.len(), count);
            for (index, slot) in slots.iter().enumerate() {
                let offset = slot.position - origin;
                assert!(
                    offset.dot(forward) > 0.0,
                    "slot {index} of {count} is behind the origin"
                );
                assert!(
                    horizontal_distance(slot.position, origin)
                        >= 0.5 + radii[index] + config.distance - 1e-3,
                    "slot {index} of {count} crowds the player"
                );
                // Facing the origin: its forward points back at it.
                let facing = Vec3::new(-slot.yaw.sin(), 0.0, -slot.yaw.cos());
                assert!(facing.dot(origin - slot.position) > 0.0);
                for (other, other_slot) in slots.iter().enumerate().skip(index + 1) {
                    let distance = horizontal_distance(slot.position, other_slot.position);
                    assert!(
                        distance >= radii[index] + radii[other] + config.gap - 1e-3,
                        "{count}: slots {index} and {other} overlap ({distance:.2} m)"
                    );
                }
            }
        }
    }
}

#[test]
fn small_groups_form_rows_nearest_first() {
    let config = PlaygroundConfig::default();
    let slots = formation(Vec3::ZERO, 0.0, 0.5, &[0.5; 3], &config);
    // Yaw zero looks down -Z: two in front, one behind them.
    let near: Vec<_> = slots.iter().map(|slot| -slot.position.z).collect();
    assert!((near[0] - near[1]).abs() < 1e-4);
    assert!(near[2] > near[0] + 1.0);
    let six = formation(Vec3::ZERO, 0.0, 0.5, &[0.5; 6], &config);
    let rows: Vec<_> = six
        .iter()
        .map(|slot| (-slot.position.z * 10.0).round())
        .collect();
    assert_eq!(rows[0], rows[2]);
    assert_eq!(rows[3], rows[5]);
    assert!(rows[3] > rows[0]);
}

// ---------------------------------------------------------------------------------------
// Spawning

#[test]
fn spawning_one_monster_puts_one_real_enemy_in_front_of_the_player() {
    let (mut simulation, mut playground) = playground();
    choose(&mut playground, &[EnemyKind::SkeletonWarrior]);
    assert_eq!(spawn(&mut simulation, &mut playground), 1);
    let ids = members(&playground);
    assert_eq!(ids.len(), 1);
    let brain = simulation.brain(ids[0]).expect("an enemy with a brain");
    assert_eq!(brain.kind(), EnemyKind::SkeletonWarrior);
    let position = simulation.world().position(ids[0]).expect("a body");
    let origin = simulation.player_position();
    assert!(
        position.z < origin.z,
        "in front: the camera at yaw 0 looks down -Z"
    );
    assert!(
        (position.y - origin.y).abs() < 0.05,
        "on the floor the player stands on"
    );
}

#[test]
fn spawning_several_brings_every_selected_monster_once_with_its_own_entity() {
    let (mut simulation, mut playground) = playground();
    let kinds = [
        EnemyKind::CryptArcher,
        EnemyKind::SkeletonWarrior,
        EnemyKind::GraveBrute,
        EnemyKind::GraveAcolyte,
        EnemyKind::CryptSteward,
    ];
    choose(&mut playground, &kinds);
    assert_eq!(spawn(&mut simulation, &mut playground), kinds.len());

    let ids = members(&playground);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), kinds.len(), "every member is its own entity");
    for kind in kinds {
        let spawned = playground
            .encounter()
            .members()
            .iter()
            .filter(|member| member.kind == kind)
            .count();
        assert_eq!(spawned, 1, "{kind:?}");
    }
    for id in &ids {
        assert!(simulation.brain(*id).is_some(), "{id:?} is a real enemy");
        assert!(simulation.world().is_alive(*id));
    }
    assert_apart(&simulation, &ids, PlaygroundConfig::default().gap);
    let player = simulation.player().entity;
    for id in &ids {
        let gap = simulation
            .world()
            .distance_between(*id, player)
            .expect("both exist");
        assert!(
            gap >= simulation.world().radius(*id) + simulation.world().radius(player),
            "{id:?} stands inside the player"
        );
    }
    // The selection is not spent by spawning.
    assert_eq!(playground.selection().len(), kinds.len());
}

#[test]
fn the_whole_library_spawns_at_once_without_overlapping() {
    let (mut simulation, mut playground) = playground();
    let every: Vec<EnemyKind> = playground
        .library()
        .entries()
        .iter()
        .map(|entry| entry.kind)
        .collect();
    choose(&mut playground, &every);
    assert_eq!(spawn(&mut simulation, &mut playground), every.len());
    assert_apart(
        &simulation,
        &members(&playground),
        PlaygroundConfig::default().gap,
    );
}

#[test]
fn a_slot_inside_a_wall_moves_to_ground_the_player_can_walk_to() {
    let (mut simulation, mut playground) = playground();
    let origin = simulation.player_position();
    // A wall right across where the first row would stand.
    let world = simulation.collision_world_mut();
    world.add_box(
        Vec3::new(origin.x - 6.0, 0.0, origin.z - 7.5),
        Vec3::new(origin.x + 6.0, 4.0, origin.z - 5.5),
        "TEST WALL",
    );
    world.rebuild_broad_phase();
    choose(
        &mut playground,
        &[EnemyKind::SkeletonWarrior, EnemyKind::CryptGhoul],
    );
    spawn(&mut simulation, &mut playground);
    for id in members(&playground) {
        let position = simulation.world().position(id).expect("a body");
        let inside = position.z < origin.z - 5.5 + 0.5
            && position.z > origin.z - 7.5 - 0.5
            && (position.x - origin.x).abs() < 6.5;
        assert!(!inside, "{id:?} stands in the wall at {position:?}");
        assert!(
            simulation.walkable_between(origin, position),
            "{id:?} is somewhere the player cannot walk to"
        );
    }
}

#[test]
fn passive_spawns_wait_and_the_rest_come_for_the_player() {
    let (mut simulation, mut playground) = playground();
    choose(&mut playground, &[EnemyKind::SkeletonWarrior]);
    playground.set_passive(true);
    spawn(&mut simulation, &mut playground);
    simulation.step_many(90);
    let waiting = members(&playground)[0];
    assert!(
        !simulation
            .brain(waiting)
            .expect("a brain")
            .state()
            .is_combat(),
        "a passive spawn waits to be attacked"
    );

    playground.set_passive(false);
    spawn(&mut simulation, &mut playground);
    simulation.step_many(90);
    let hostile = members(&playground)[0];
    let brain = simulation.brain(hostile).expect("a brain");
    assert!(brain.state().is_combat(), "{:?}", brain.state());
    assert_eq!(brain.target(), Some(simulation.player().entity));
    assert!(simulation.player_in_combat());
}

#[test]
fn the_monster_level_scales_spawns_like_a_level_placement() {
    let (mut simulation, mut playground) = playground();
    choose(&mut playground, &[EnemyKind::GraveBrute]);
    spawn(&mut simulation, &mut playground);
    let base = simulation
        .world()
        .health(members(&playground)[0])
        .expect("health")
        .max();
    playground.set_monster_level(5);
    spawn(&mut simulation, &mut playground);
    let levelled = simulation
        .world()
        .health(members(&playground)[0])
        .expect("health")
        .max();
    assert!(levelled > base * 1.4, "{base} -> {levelled}");
}

// ---------------------------------------------------------------------------------------
// Fighting, and resetting

#[test]
fn the_monsters_fight_with_their_real_abilities() {
    let (mut simulation, mut playground) = playground();
    choose(
        &mut playground,
        &[EnemyKind::SkeletonWarrior, EnemyKind::CryptArcher],
    );
    spawn(&mut simulation, &mut playground);
    let player = simulation.player().entity;
    let ids = members(&playground);
    let mut hit_by = Vec::new();
    for _ in 0..(60 * 10) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::Damage(damage) = event
                && damage.target == player
                && ids.contains(&damage.source)
                && !hit_by.contains(&damage.source)
            {
                hit_by.push(damage.source);
            }
        }
    }
    assert_eq!(hit_by.len(), ids.len(), "every monster landed something");
    assert!(simulation.player_alive(), "test health keeps the player up");
}

#[test]
fn resetting_takes_every_playground_monster_and_nothing_else() {
    let (mut simulation, mut playground) = playground();
    let baseline = simulation.world().len();
    // An enemy the playground did not spawn: it must survive the reset.
    let outsider = simulation.spawn_enemy(
        EnemyKind::CryptGhoul,
        simulation.player_position() + Vec3::new(30.0, 0.0, 30.0),
        0.0,
    );
    choose(
        &mut playground,
        &[EnemyKind::CryptArcher, EnemyKind::GraveBrute],
    );
    spawn(&mut simulation, &mut playground);
    let ids = members(&playground);
    simulation.push_command(engine_game::command::PlayerCommand::SelectTarget { entity: ids[0] });
    simulation.step_many(60 * 3);

    assert_eq!(playground.reset_encounter(&mut simulation), ids.len());
    assert!(playground.encounter().is_empty());
    for id in &ids {
        assert!(
            !simulation.world().contains(*id),
            "{id:?} is still in the world"
        );
        assert!(simulation.brain(*id).is_none());
    }
    assert!(
        simulation.world().contains(outsider),
        "an outsider was taken"
    );
    assert_eq!(simulation.world().len(), baseline + 1);
    let target = simulation.player().target;
    assert!(
        target.is_none_or(|target| !ids.contains(&target)),
        "the player still targets a reset monster"
    );
    // The arena's own dummy is untouched.
    assert!(simulation.world().len() > 1);
    // The selection survives a reset.
    assert_eq!(playground.selection().len(), 2);
}

#[test]
fn resetting_mid_fight_leaves_nothing_of_the_fight_behind() {
    let (mut simulation, mut playground) = playground();
    let player = simulation.player().entity;
    choose(
        &mut playground,
        &[
            EnemyKind::CryptGhoul,
            EnemyKind::SkeletonBonecaster,
            EnemyKind::GraveAcolyte,
            EnemyKind::CryptArcher,
        ],
    );
    spawn(&mut simulation, &mut playground);
    let ids = members(&playground);
    // Fight until somebody is casting and the player carries a monster's effect, or give up
    // after twenty seconds and reset whatever state the fight is in.
    let mut casting = false;
    let mut afflicted = false;
    for _ in 0..(60 * 20) {
        simulation.step_once();
        casting |= ids.iter().any(|id| {
            simulation
                .brain(*id)
                .is_some_and(|brain| brain.cast().is_some())
        });
        afflicted |= simulation
            .world()
            .statuses(player)
            .is_some_and(|statuses| statuses.iter().any(|status| ids.contains(&status.source)));
        if casting && afflicted {
            break;
        }
    }
    assert!(casting, "nobody cast anything in twenty seconds");
    // One of the player's own spells in flight at a member, too.
    simulation.push_command(engine_game::command::PlayerCommand::SelectTarget { entity: ids[1] });
    simulation.push_command(engine_game::command::PlayerCommand::UseAbility(0));
    simulation.step_once();

    playground.reset_encounter(&mut simulation);

    let statuses = simulation.world().statuses(player).expect("a player");
    assert!(
        statuses.iter().all(|status| !ids.contains(&status.source)),
        "a reset monster's effect is still on the player"
    );
    assert!(
        simulation
            .player()
            .cast
            .and_then(|cast| cast.target)
            .is_none_or(|target| !ids.contains(&target)),
        "the player is still casting at a reset monster"
    );
    assert!(
        simulation.enemy_hazards().is_empty(),
        "ground a reset monster left is still burning"
    );
    assert!(!simulation.player_in_combat());
    assert_eq!(simulation.enemy_count(), 0);
    // And the world carries on with no trace: no damage from anybody who is gone.
    for _ in 0..(60 * 5) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::Damage(damage) = event {
                assert!(!ids.contains(&damage.source), "{damage:?} after the reset");
            }
        }
    }
}

#[test]
fn a_reset_also_takes_dead_monsters_and_killing_lets_every_death_play() {
    let (mut simulation, mut playground) = playground();
    choose(
        &mut playground,
        &[EnemyKind::SkeletonWarrior, EnemyKind::GraveBrute],
    );
    spawn(&mut simulation, &mut playground);
    simulation.step_many(30);
    assert_eq!(playground.kill_encounter(&mut simulation), 2);
    simulation.step_once();
    let ids = members(&playground);
    assert_eq!(ids.len(), 2, "the dead are still the encounter's");
    assert_eq!(playground.encounter().living(&simulation), 0);
    assert!(ids.iter().all(|id| simulation.world().contains(*id)));
    playground.reset_encounter(&mut simulation);
    assert!(ids.iter().all(|id| !simulation.world().contains(*id)));
}

#[test]
fn members_something_else_took_away_are_forgotten() {
    let (mut simulation, mut playground) = playground();
    choose(&mut playground, &[EnemyKind::SkeletonWarrior]);
    spawn(&mut simulation, &mut playground);
    simulation.clear_enemies();
    playground.observe(&simulation);
    assert!(playground.encounter().is_empty(), "a stale id was kept");
    // And a spawn after that is clean.
    assert_eq!(spawn(&mut simulation, &mut playground), 1);
}

#[test]
fn select_spawn_reset_repeats_without_leaking_anything() {
    let (mut simulation, mut playground) = playground();
    let baseline = simulation.world().len();
    let groups: [&[EnemyKind]; 4] = [
        &[EnemyKind::CryptArcher, EnemyKind::SkeletonWarrior],
        &[EnemyKind::GraveBrute],
        &[
            EnemyKind::GraveAcolyte,
            EnemyKind::CryptGhoul,
            EnemyKind::SkeletonBonecaster,
        ],
        &[EnemyKind::CryptSteward, EnemyKind::EmpoweredWarrior],
    ];
    for round in 0..3 {
        for group in groups {
            choose(&mut playground, group);
            assert_eq!(spawn(&mut simulation, &mut playground), group.len());
            assert_eq!(simulation.enemy_count(), group.len(), "round {round}");
            simulation.step_many(60 * 2);
            playground.observe(&simulation);
            playground.reset_encounter(&mut simulation);
            assert!(playground.encounter().is_empty());
            assert_eq!(simulation.enemy_count(), 0, "round {round}");
            assert_eq!(simulation.world().len(), baseline, "round {round}");
            assert!(!simulation.player_in_combat());
        }
    }
}

#[test]
fn spawning_again_replaces_the_current_encounter() {
    let (mut simulation, mut playground) = playground();
    choose(
        &mut playground,
        &[EnemyKind::CryptArcher, EnemyKind::GraveBrute],
    );
    spawn(&mut simulation, &mut playground);
    let first = members(&playground);
    choose(&mut playground, &[EnemyKind::CryptGhoul]);
    spawn(&mut simulation, &mut playground);
    assert!(first.iter().all(|id| !simulation.world().contains(*id)));
    assert_eq!(simulation.enemy_count(), 1);
}

// ---------------------------------------------------------------------------------------
// The player

#[test]
fn test_health_is_the_configured_value_and_normal_health_is_the_sheets() {
    let normal_max = {
        let simulation = arena();
        simulation
            .world()
            .health(simulation.player().entity)
            .expect("health")
            .max()
    };
    let config = PlaygroundConfig {
        test_health: 250_000.0,
        ..PlaygroundConfig::default()
    };
    let mut simulation = arena();
    let mut playground = MonsterPlayground::new(config);
    playground.install(&mut simulation);
    let player = simulation.player().entity;
    let health = |simulation: &Simulation| {
        let health = simulation.world().health(player).expect("health");
        (health.current(), health.max())
    };
    assert_eq!(playground.health_mode(), HealthMode::Test);
    assert_eq!(health(&simulation), (250_000.0, 250_000.0));

    simulation.world_mut().damage(player, 1_000.0);
    playground.set_health_mode(&mut simulation, HealthMode::Normal);
    assert_eq!(health(&simulation), (normal_max, normal_max));
    // Held through a gear refresh, either way.
    simulation.refresh_character();
    assert_eq!(health(&simulation).1, normal_max);

    playground.set_health_mode(&mut simulation, HealthMode::Test);
    simulation.refresh_character();
    assert_eq!(health(&simulation), (250_000.0, 250_000.0));
}

#[test]
fn resetting_the_player_restores_health_cooldowns_and_clears_effects() {
    let (mut simulation, mut playground) = playground();
    let player = simulation.player().entity;
    playground.set_health_mode(&mut simulation, HealthMode::Normal);
    let max = simulation.world().health(player).expect("health").max();

    simulation.world_mut().damage(player, max * 0.5);
    let effects = TalentEffects::default();
    let on_cooldown = FireAbility::ALL
        .into_iter()
        .find(|ability| ability.definition().cooldown_with(&effects) > 0.0)
        .expect("some ability has a cooldown");
    while simulation.mage().abilities.is_ready(on_cooldown) {
        simulation.mage_mut().abilities.spend(on_cooldown, &effects);
    }
    simulation
        .world_mut()
        .statuses_mut(player)
        .expect("statuses")
        .apply(Status::new(
            StatusKind::Bleeding,
            EntityId::new(40, 1),
            AbilityId::NONE,
            30.0,
            5.0,
        ));

    playground.reset_player(&mut simulation);

    let health = simulation.world().health(player).expect("health");
    assert_eq!((health.current(), health.max()), (max, max));
    assert!(simulation.mage().abilities.is_ready(on_cooldown));
    assert!(
        simulation
            .world()
            .statuses(player)
            .expect("statuses")
            .is_empty()
    );
    assert!(simulation.player().cast.is_none());
    assert_eq!(
        playground.health_mode(),
        HealthMode::Normal,
        "the mode is kept"
    );
}

#[test]
fn resetting_a_dead_player_brings_them_back() {
    let (mut simulation, mut playground) = playground();
    playground.set_health_mode(&mut simulation, HealthMode::Normal);
    simulation.kill_player();
    simulation.step_once();
    assert!(!simulation.player_alive());
    playground.reset_player(&mut simulation);
    assert!(simulation.player_alive());
    let health = simulation
        .world()
        .health(simulation.player().entity)
        .expect("health");
    assert_eq!(health.current(), health.max());
}
