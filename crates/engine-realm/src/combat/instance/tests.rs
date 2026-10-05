//! The server's fight, headless: the real crypt with three players in it.

use super::*;
use crate::catalog::Catalog;
use crate::movement::barriers_of;
use engine_game::ability::AbilityId;
use engine_game::combat::DamageType;

fn crypt() -> (EnvironmentScene, Vec<Barrier>, Vec<PlayerSpec>) {
    let catalog = Catalog::builtin().expect("catalog");
    let dungeon = catalog.dungeon("graveyard_crypt").expect("crypt").clone();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(&dungeon.scene);
    let scene = engine_environment::scene_file::load(path).expect("the crypt loads");
    let barriers = barriers_of(&scene, &dungeon);
    let players = [
        ("tank", Hero::Warrior),
        ("healer", Hero::Druid),
        ("dps", Hero::FireMage),
    ]
    .iter()
    .enumerate()
    .map(|(slot, (account, hero))| PlayerSpec {
        account_id: (*account).to_owned(),
        name: account.to_uppercase(),
        hero: *hero,
        loadout: None,
        spawn: dungeon.spawn_point(slot),
    })
    .collect();
    (scene, barriers, players)
}

fn walk(seq: u32) -> InputFrame {
    InputFrame {
        seq,
        forward: 1.0,
        right: 0.0,
        yaw: 0.0,
        face_camera: true,
        jump: false,
    }
}

#[test]
fn the_party_enters_with_its_own_classes_and_the_monsters_are_keyed() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    let states = fight.player_states();
    assert_eq!(states.len(), 3);
    assert!(states.iter().all(|s| s.alive && s.health > 0.0));
    let tank = fight.resolve(&EntityRef::Player("tank".into())).expect("tank");
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    assert_eq!(fight.sim.with_player(tank, |s| s.hero()), Some(Hero::Warrior));
    assert_eq!(fight.sim.with_player(healer, |s| s.hero()), Some(Hero::Druid));
    let monsters = fight.monster_states();
    assert!(monsters.len() > 10, "the crypt is populated: {}", monsters.len());
    for (index, monster) in monsters.iter().enumerate() {
        assert_eq!(monster.key, index as u64, "placed monsters are keyed by spawn order");
    }
    // Two fights from the same level agree on every key.
    let again = CombatInstance::new(&scene, &players, &barriers).monster_states();
    assert_eq!(
        monsters.iter().map(|m| (m.key, m.kind)).collect::<Vec<_>>(),
        again.iter().map(|m| (m.key, m.kind)).collect::<Vec<_>>()
    );
}

#[test]
fn the_start_barrier_holds_until_it_is_lifted() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    let frames: Vec<InputFrame> = (1..=600).map(walk).collect();
    let (body, last) = fight.apply_frames("dps", &frames).expect("dps is here");
    assert_eq!(last, 600);
    assert!(body.position.z > -32.0, "held at the barrier: {:?}", body.position);
    fight.open_start_barrier();
    let frames: Vec<InputFrame> = (601..=900).map(walk).collect();
    let (body, _) = fight.apply_frames("dps", &frames).expect("dps is here");
    assert!(body.position.z < -33.0, "through the doorway: {:?}", body.position);
}

#[test]
fn an_action_names_its_target_and_the_damage_comes_back_named() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    fight.open_start_barrier();
    let monster = fight.monster_states().into_iter().next().expect("a monster");
    let mage = fight.resolve(&EntityRef::Player("dps".into())).expect("dps");
    // Stand the mage fifteen metres from it.
    let near = monster.position + Vec3::new(0.0, 0.0, 15.0);
    if let Some(body) = fight.sim.world_mut().body_mut(mage) {
        *body = Body::at(near);
    }
    fight
        .act(
            "dps",
            &CombatAction {
                target: EntityRef::Monster(monster.key),
                kind: ActionKind::Ability(0),
            },
        )
        .expect("a well-formed press");
    let events = fight.advance(180);
    let hit = events.iter().any(|event| {
        matches!(event, WireEvent::Damage { source, target, amount, .. }
            if *source == EntityRef::Player("dps".into())
                && *target == EntityRef::Monster(monster.key)
                && *amount > 0.0)
    });
    assert!(hit, "Firebolt landed on the monster the client named");
    let after = fight
        .monster_states()
        .into_iter()
        .find(|m| m.key == monster.key)
        .expect("still there");
    assert!(after.health < monster.health);
}

#[test]
fn malformed_and_foreign_actions_are_refused() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    let press = |kind| CombatAction {
        target: EntityRef::None,
        kind,
    };
    assert_eq!(
        fight.act("intruder", &press(ActionKind::Ability(0))),
        Err(ActionError::NotInFight)
    );
    assert_eq!(
        fight.act("dps", &press(ActionKind::Ability(99))),
        Err(ActionError::Malformed)
    );
    assert_eq!(
        fight.act("dps", &press(ActionKind::Extra(5))),
        Err(ActionError::Malformed)
    );
    assert_eq!(
        fight.act("dps", &press(ActionKind::Ultimate([f32::NAN, 0.0, 0.0]))),
        Err(ActionError::Malformed)
    );
    assert!(fight.act("dps", &press(ActionKind::Target)).is_ok());
}

#[test]
fn only_changed_monsters_are_written_again() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    let (first, gone) = fight.changed_monsters();
    assert!(!first.is_empty());
    assert!(gone.is_empty());
    fight.advance(6);
    let (second, _) = fight.changed_monsters();
    assert!(
        second.len() < first.len() / 2,
        "an idle crypt barely changes: {}",
        second.len()
    );
}

#[test]
fn the_whole_crypt_can_be_cleared_and_the_run_completes() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    fight.open_start_barrier();
    let mage = fight.resolve(&EntityRef::Player("dps".into())).expect("dps");
    let mut saw_unlock = false;
    for _ in 0..400 {
        let living: Vec<EntityId> = fight
            .sim
            .enemies()
            .map(|(entity, _)| entity)
            .filter(|entity| fight.sim.world().is_alive(*entity))
            .collect();
        for entity in living {
            fight.sim.apply_damage(
                mage,
                entity,
                1.0e7,
                AbilityId::from_fire(FireAbility::Firebolt),
                DamageType::Fire,
            );
        }
        let events = fight.advance(30);
        let _ = events;
        saw_unlock |= fight.unlocked();
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(saw_unlock, "the gate opened");
    assert!(
        fight.completed().is_some(),
        "the last boss died and the run completed"
    );
}

#[test]
fn a_rebuilt_fight_keeps_keys_the_dead_and_the_bodies() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers);
    fight.open_start_barrier();
    let monsters = fight.monster_states();
    let victim = monsters[0].key;
    let frames: Vec<InputFrame> = (1..=60).map(walk).collect();
    let (body, _) = fight.apply_frames("tank", &frames).expect("tank");
    let records: Vec<MonsterRecord> = monsters
        .iter()
        .map(|m| MonsterRecord {
            key: m.key,
            alive: m.key != victim,
            health: m.health * 0.5,
            position: m.position,
        })
        .collect();
    let state = Restore {
        tick: 1234,
        credited: 5.0,
        unlocked: false,
        monsters: records,
        players: vec![PlayerRecord {
            account_id: "tank".into(),
            body,
            health: 100.0,
            last_seq: 60,
        }],
    };
    let mut rebuilt = CombatInstance::restore(&scene, &players, &barriers, true, &state);
    assert_eq!(rebuilt.tick(), 1234);
    assert!(rebuilt.resolve(&EntityRef::Monster(victim)).is_none());
    let survivor = rebuilt
        .monster_states()
        .into_iter()
        .find(|m| m.key == monsters[1].key)
        .expect("kept");
    assert!((survivor.health - monsters[1].health * 0.5).abs() < 1.0);
    let tank = rebuilt
        .player_states()
        .into_iter()
        .find(|p| p.account_id == "tank")
        .expect("tank");
    assert_eq!(tank.last_seq, 60);
    assert!((tank.body.position - body.position).length() < 1e-3);
    assert!((tank.health - 100.0).abs() < 1e-3);
    assert!((rebuilt.credited() - 5.0).abs() < 1e-3);
}
