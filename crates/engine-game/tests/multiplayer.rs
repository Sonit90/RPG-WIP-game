//! Several players in one simulation: what an online dungeon instance runs.
//!
//! The single-player simulation is the same code; these tests check that a second and third
//! player are real players — their own commands, class state, casts, defences and deaths —
//! and that nothing one player does leaks into another's state.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::world::EntityDesc;

struct Party {
    sim: Simulation,
    mage: EntityId,
    warrior: EntityId,
    druid: EntityId,
    enemy: EntityId,
}

/// A fire mage (the first player), a warrior and a druid, and one enemy `distance` metres in
/// front of the mage.
fn party(kind: EnemyKind, distance: f32) -> Party {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("PARTY", vec![EnemySpawn::new(kind, spawn)]);
    let mut sim = arena::encounter_arena(&config, encounter);
    let enemy = sim
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    let mage = sim.player().entity;
    let warrior = sim.world_mut().spawn(EntityDesc::player(
        config.player_spawn + Vec3::new(3.0, 0.0, 0.0),
    ));
    let druid = sim.world_mut().spawn(EntityDesc::player(
        config.player_spawn + Vec3::new(-3.0, 0.0, 0.0),
    ));
    sim.add_player(warrior, Hero::Warrior);
    sim.add_player(druid, Hero::Druid);
    Party {
        sim,
        mage,
        warrior,
        druid,
        enemy,
    }
}

fn run(sim: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        sim.step_once();
        events.extend_from_slice(sim.events());
    }
    events
}

fn health(sim: &Simulation, entity: EntityId) -> f32 {
    sim.world().health(entity).map_or(0.0, |h| h.current())
}

#[test]
fn every_player_is_known_and_keeps_their_own_hero() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    assert_eq!(party.sim.player_count(), 3);
    assert_eq!(
        party.sim.player_entities(),
        vec![party.mage, party.warrior, party.druid]
    );
    assert_eq!(party.sim.hero(), Hero::FireMage);
    assert_eq!(
        party.sim.with_player(party.warrior, |s| s.hero()),
        Some(Hero::Warrior)
    );
    assert_eq!(
        party.sim.with_player(party.druid, |s| s.hero()),
        Some(Hero::Druid)
    );
    // Swapping in and out leaves the active player as it was.
    assert_eq!(party.sim.player().entity, party.mage);
    assert!(party.sim.with_player(party.enemy, |_| ()).is_none());
}

#[test]
fn each_player_moves_with_their_own_intent() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    let before = |sim: &Simulation, e| sim.world().position(e).expect("alive");
    let (m0, w0, d0) = (
        before(&party.sim, party.mage),
        before(&party.sim, party.warrior),
        before(&party.sim, party.druid),
    );
    let walk = PlayerCommand::Move {
        forward: 1.0,
        right: 0.0,
        yaw: 0.0,
        face_camera: false,
    };
    assert!(party.sim.push_command_for(party.warrior, walk));
    run(&mut party.sim, 60);
    let moved = |a: Vec3, b: Vec3| (a - b).length();
    assert!(
        moved(before(&party.sim, party.warrior), w0) > 2.0,
        "the warrior walked"
    );
    assert!(
        moved(before(&party.sim, party.mage), m0) < 0.05,
        "the mage stood still"
    );
    assert!(
        moved(before(&party.sim, party.druid), d0) < 0.05,
        "the druid stood still"
    );
}

#[test]
fn each_player_casts_their_own_abilities() {
    let mut party = party(EnemyKind::SkeletonWarrior, 12.0);
    let enemy = party.enemy;
    for player in [party.mage, party.druid] {
        party
            .sim
            .push_command_for(player, PlayerCommand::SelectTarget { entity: enemy });
        // Slot 0: Firebolt for the mage, Moonfire for the druid.
        party
            .sim
            .push_command_for(player, PlayerCommand::UseAbility(0));
    }
    let events = run(&mut party.sim, 240);
    let dealt_by = |source: EntityId| {
        events.iter().any(|event| {
            matches!(event, CombatEvent::Damage(d) if d.source == source && d.target == enemy && d.amount > 0.0)
        })
    };
    assert!(dealt_by(party.mage), "the mage's Firebolt landed");
    assert!(dealt_by(party.druid), "the druid's Moonfire landed");
    assert!(
        !dealt_by(party.warrior),
        "the warrior did nothing and dealt nothing"
    );
}

#[test]
fn enemies_attack_whichever_player_they_choose_and_the_damage_lands_on_them() {
    // A skeleton right beside the warrior, the others far behind.
    let mut party = party(EnemyKind::SkeletonWarrior, 3.0);
    let near = party.sim.world().position(party.enemy).expect("alive") + Vec3::new(0.0, 0.0, 1.5);
    let warrior = party.warrior;
    party.sim.with_player(warrior, |sim| {
        if let Some(body) = sim.world_mut().body_mut(warrior) {
            body.position = near;
        }
    });
    for (entity, offset) in [(party.mage, 25.0), (party.druid, 28.0)] {
        if let Some(body) = party.sim.world_mut().body_mut(entity) {
            body.position += Vec3::new(0.0, 0.0, offset);
        }
    }
    let warrior_max = health(&party.sim, warrior);
    let events = run(&mut party.sim, 600);
    let hit_warrior = events.iter().any(|event| {
        matches!(event, CombatEvent::Damage(d) if d.target == warrior && d.source == party.enemy)
    });
    assert!(hit_warrior, "the skeleton fought the warrior");
    assert!(
        health(&party.sim, warrior) < warrior_max
            || events
                .iter()
                .any(|e| matches!(e, CombatEvent::Damage(d) if d.target == warrior))
    );
}

#[test]
fn a_druid_heals_another_player() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    let mage = party.mage;
    let full = health(&party.sim, mage);
    party.sim.world_mut().damage(mage, full * 0.5);
    let wounded = health(&party.sim, mage);
    party
        .sim
        .push_command_for(party.druid, PlayerCommand::SelectTarget { entity: mage });
    // Slot 3: Rejuvenation, healing over time.
    party
        .sim
        .push_command_for(party.druid, PlayerCommand::UseAbility(3));
    run(&mut party.sim, 600);
    assert!(
        health(&party.sim, mage) > wounded + 1.0,
        "the mage was healed"
    );
}

#[test]
fn a_wipe_is_every_player_down() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    assert!(!party.sim.all_players_dead());
    for entity in [party.mage, party.warrior] {
        let full = health(&party.sim, entity);
        party.sim.world_mut().damage(entity, full * 10.0);
    }
    assert!(!party.sim.all_players_dead(), "the druid still stands");
    let full = health(&party.sim, party.druid);
    party.sim.world_mut().damage(party.druid, full * 10.0);
    assert!(party.sim.all_players_dead());
}

#[test]
fn removing_a_player_leaves_the_rest() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    party.sim.remove_player(party.warrior);
    assert_eq!(party.sim.player_count(), 2);
    assert!(!party.sim.is_player_entity(party.warrior));
    assert_eq!(
        party.sim.with_player(party.druid, |s| s.hero()),
        Some(Hero::Druid)
    );
    run(&mut party.sim, 30);
}

#[test]
fn under_a_remote_authority_presses_go_to_the_server_and_nothing_is_decided_locally() {
    let mut party = party(EnemyKind::SkeletonWarrior, 12.0);
    let enemy = party.enemy;
    let before = health(&party.sim, enemy);
    party.sim.set_remote_authority(true);
    party
        .sim
        .push_command(PlayerCommand::SelectTarget { entity: enemy });
    party.sim.push_command(PlayerCommand::UseAbility(0));
    let events = run(&mut party.sim, 240);
    assert_eq!(
        party.sim.player().target,
        Some(enemy),
        "targeting stays local"
    );
    assert_eq!(
        party.sim.take_remote_requests(),
        vec![PlayerCommand::UseAbility(0)]
    );
    assert!(party.sim.take_remote_requests().is_empty());
    assert!(!events.iter().any(|e| matches!(e, CombatEvent::Damage(_))));
    assert!((health(&party.sim, enemy) - before).abs() < f32::EPSILON);
    // What the server says is what the world shows.
    party.sim.present_health(enemy, before * 0.25, before);
    assert!((health(&party.sim, enemy) - before * 0.25).abs() < 1e-3);
}

#[test]
fn removing_the_active_player_promotes_the_next_one_with_their_own_state() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    assert!(party.sim.remove_player(party.mage));
    assert_eq!(party.sim.player_count(), 2);
    assert!(!party.sim.is_player_entity(party.mage));
    assert!(!party.sim.world().contains(party.mage));
    assert_eq!(party.sim.player().entity, party.warrior);
    assert_eq!(party.sim.hero(), Hero::Warrior);
    assert_eq!(
        party.sim.player_entities(),
        vec![party.warrior, party.druid]
    );
    assert_eq!(
        party.sim.with_player(party.druid, |s| s.hero()),
        Some(Hero::Druid)
    );
    assert!(!party.sim.party().contains(party.mage));
    run(&mut party.sim, 60);
}

#[test]
fn the_last_player_cannot_be_removed() {
    let mut party = party(EnemyKind::SkeletonWarrior, 60.0);
    assert!(party.sim.remove_player(party.warrior));
    assert!(party.sim.remove_player(party.druid));
    assert!(!party.sim.remove_player(party.mage));
    assert_eq!(party.sim.player_count(), 1);
    assert!(party.sim.world().contains(party.mage));
}

#[test]
fn a_removed_player_is_forgotten_by_the_enemies_and_the_wipe_check() {
    let mut party = party(EnemyKind::SkeletonWarrior, 8.0);
    let (enemy, mage, warrior) = (party.enemy, party.mage, party.warrior);
    party
        .sim
        .push_command_for(party.druid, PlayerCommand::SelectTarget { entity: warrior });
    party
        .sim
        .push_command(PlayerCommand::SelectTarget { entity: enemy });
    party.sim.push_command(PlayerCommand::UseAbility(0));
    run(&mut party.sim, 120);
    assert!(
        party
            .sim
            .brain(enemy)
            .is_some_and(|b| b.threat().contains(mage))
    );
    assert!(party.sim.remove_player(mage));
    assert!(
        party
            .sim
            .brain(enemy)
            .is_some_and(|b| !b.threat().contains(mage) && b.target() != Some(mage))
    );
    // A selection of a removed body is cleared everywhere.
    assert!(party.sim.remove_player(warrior));
    assert_eq!(
        party.sim.with_player(party.druid, |s| s.player().target),
        Some(None)
    );
    run(&mut party.sim, 120);
    // Only the druid is left: a wipe is the druid down.
    assert!(!party.sim.all_players_dead());
    let full = health(&party.sim, party.druid);
    party.sim.world_mut().damage(party.druid, full * 10.0);
    assert!(party.sim.all_players_dead());
}
