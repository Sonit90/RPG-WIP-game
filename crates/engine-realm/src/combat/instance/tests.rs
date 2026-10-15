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
        talents: Vec::new(),
        talent_points: 5,
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
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let states = fight.player_states();
    assert_eq!(states.len(), 3);
    assert!(states.iter().all(|s| s.alive && s.health > 0.0));
    let tank = fight
        .resolve(&EntityRef::Player("tank".into()))
        .expect("tank");
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    assert_eq!(
        fight.sim.with_player(tank, |s| s.hero()),
        Some(Hero::Warrior)
    );
    assert_eq!(
        fight.sim.with_player(healer, |s| s.hero()),
        Some(Hero::Druid)
    );
    let monsters = fight.monster_states();
    assert!(
        monsters.len() > 10,
        "the crypt is populated: {}",
        monsters.len()
    );
    for (index, monster) in monsters.iter().enumerate() {
        assert_eq!(
            monster.key, index as u64,
            "placed monsters are keyed by spawn order"
        );
    }
    // Two fights from the same level agree on every key.
    let again = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0)).monster_states();
    assert_eq!(
        monsters.iter().map(|m| (m.key, m.kind)).collect::<Vec<_>>(),
        again.iter().map(|m| (m.key, m.kind)).collect::<Vec<_>>()
    );
}

#[test]
fn the_start_barrier_holds_until_it_is_lifted() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let frames: Vec<InputFrame> = (1..=600).map(walk).collect();
    let (body, last) = fight.apply_frames("dps", &frames).expect("dps is here");
    assert_eq!(last, 600);
    assert!(
        body.position.z > -32.0,
        "held at the barrier: {:?}",
        body.position
    );
    fight.open_start_barrier();
    let frames: Vec<InputFrame> = (601..=900).map(walk).collect();
    let (body, _) = fight.apply_frames("dps", &frames).expect("dps is here");
    assert!(
        body.position.z < -33.0,
        "through the doorway: {:?}",
        body.position
    );
}

#[test]
fn an_action_names_its_target_and_the_damage_comes_back_named() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let monster = fight
        .monster_states()
        .into_iter()
        .next()
        .expect("a monster");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
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
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
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
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
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
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
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
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
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
    let mut rebuilt =
        CombatInstance::restore(&scene, &players, &barriers, (1.0, 1.0), true, &state);
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

#[test]
fn the_talents_in_a_player_spec_reach_the_simulation() {
    let (scene, barriers, mut players) = crypt();
    players[1].talents = vec!["swift_wrath".into(), "verdant_sunfire".into()];
    players[1].talent_points = 6;
    players[2].talents = vec!["infernal_capacity".into()];
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    let tank = fight
        .resolve(&EntityRef::Player("tank".into()))
        .expect("tank");

    let effects = fight
        .sim
        .with_player(healer, |sim| *sim.druid().effects())
        .expect("druid");
    assert_eq!(effects.swift_wrath_every, 4);
    assert!(effects.sunfire_radius_scale > 1.0);
    let ignite = fight
        .sim
        .with_player(mage, |sim| sim.mage().effects().ignite_max_stacks)
        .expect("mage");
    assert_eq!(ignite, 6);
    let tank_points = fight
        .sim
        .with_player(tank, |sim| sim.talent_grid().purchased_count())
        .expect("warrior");
    assert_eq!(tank_points, 0);
}

#[test]
fn talents_that_do_not_fit_are_dropped_whole() {
    let (scene, barriers, mut players) = crypt();
    // Six points cannot hold seven; and an unknown key spoils the whole set too.
    players[1].talents = vec!["swift_wrath".into(), "deep_exposure".into()];
    players[1].talent_points = 6;
    players[2].talents = vec!["infernal_capacity".into(), "no_such_talent".into()];
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    assert_eq!(
        fight
            .sim
            .with_player(healer, |sim| sim.talent_grid().purchased_count()),
        Some(0)
    );
    assert_eq!(
        fight
            .sim
            .with_player(mage, |sim| sim.mage().effects().ignite_max_stacks),
        Some(5)
    );
}

/// Kills every living monster the way a damage ability would and returns the events the kills
/// announced (a kill made between steps announces itself into the current stream, which the next
/// step clears, so it is read here).
fn slay_all(fight: &mut CombatInstance, killer: EntityId) -> Vec<WireEvent> {
    let living: Vec<EntityId> = fight
        .sim
        .enemies()
        .map(|(entity, _)| entity)
        .filter(|entity| fight.sim.world().is_alive(*entity))
        .collect();
    for entity in living {
        fight.sim.apply_damage(
            killer,
            entity,
            1.0e7,
            AbilityId::from_fire(FireAbility::Firebolt),
            DamageType::Fire,
        );
    }
    let announced: Vec<engine_game::event::CombatEvent> = fight.sim.events().to_vec();
    let name = |entity: EntityId| fight.name(entity);
    announced
        .iter()
        .filter_map(|event| encode(event, &name))
        .collect()
}

/// Clears the crypt at `bonus` per cent reward bonus and returns every event of the run.
fn cleared_with_loot(bonus: u32) -> (CombatInstance, Vec<WireEvent>) {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let rules = crate::loot::builtin_rules().expect("the shipped rules build");
    fight.set_loot(rules.clone(), 5, bonus);
    assert_eq!(fight.reward_bonus(), bonus);
    fight.open_start_barrier();
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    let mut events = Vec::new();
    for _ in 0..400 {
        events.extend(slay_all(&mut fight, mage));
        events.extend(fight.advance(30));
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(fight.completed().is_some(), "the run completed");
    (fight, events)
}

fn gold_of(events: &[WireEvent], account: &str) -> u32 {
    let gold = crate::loot::builtin_rules()
        .expect("rules")
        .gold
        .resource
        .to_owned();
    crate::haul::drops_in(events)
        .into_iter()
        .filter_map(|drop| match drop {
            crate::haul::Drop::Currency {
                account: who,
                resource,
                amount,
            } if who == account && resource == gold => Some(amount),
            _ => None,
        })
        .sum()
}

#[test]
fn a_cleared_run_with_loot_rules_rolls_two_items_per_player_and_pays_gold() {
    let (fight, events) = cleared_with_loot(0);
    for account in ["tank", "healer", "dps"] {
        let items = fight.loot_items_of(account);
        assert_eq!(items.len(), 2, "{account} gets two chest items");
        for item in &items {
            assert!(
                (engine_game::character::QUALITY_MIN..=engine_game::character::QUALITY_MAX)
                    .contains(&item.props.quality),
                "{account}: quality {}",
                item.props.quality
            );
            assert!(!item.props.broken && item.props.enhancement == 0);
        }
        assert!(
            gold_of(&events, account) >= 1,
            "{account} was paid gold from kills and the boss"
        );
    }
    // Every drop names a player of the fight and a known item or resource.
    let drops = crate::haul::drops_in(&events);
    assert!(!drops.is_empty());
    for drop in drops {
        let account = match &drop {
            crate::haul::Drop::Item { account, .. }
            | crate::haul::Drop::Currency { account, .. } => account,
        };
        assert!(["tank", "healer", "dps"].contains(&account.as_str()));
    }
}

#[test]
fn the_reward_bonus_scales_what_the_fight_rolls() {
    let (_, plain) = cleared_with_loot(0);
    let (_, rich) = cleared_with_loot(100);
    let (plain_gold, rich_gold) = (gold_of(&plain, "tank"), gold_of(&rich, "tank"));
    assert!(
        rich_gold > plain_gold,
        "+100% gold: {rich_gold} against {plain_gold}"
    );
}

#[test]
fn without_loot_rules_the_chest_is_the_legacy_single_item_and_monsters_drop_nothing() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    assert_eq!(fight.reward_bonus(), 0);
    fight.open_start_barrier();
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    let mut events = Vec::new();
    for _ in 0..400 {
        events.extend(slay_all(&mut fight, mage));
        events.extend(fight.advance(30));
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(crate::haul::drops_in(&events).is_empty());
    assert_eq!(fight.loot_items_of("tank").len(), 1);
}
