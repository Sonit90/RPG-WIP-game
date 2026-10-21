//! The server's fight, headless: the real crypt with three players in it.

use super::*;
use crate::catalog::Catalog;
use crate::movement::barriers_of;
use engine_game::ability::AbilityId;
use engine_game::combat::DamageType;

fn crypt() -> (EnvironmentScene, Vec<Barrier>, Vec<PlayerSpec>) {
    dungeon_party("graveyard_crypt")
}

fn dungeon_party(id: &str) -> (EnvironmentScene, Vec<Barrier>, Vec<PlayerSpec>) {
    let catalog = Catalog::builtin().expect("catalog");
    let dungeon = catalog.dungeon(id).expect("dungeon").clone();
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
    assert_eq!(
        fight.act("dps", &press(ActionKind::Trinket(2))),
        Err(ActionError::Malformed)
    );
    assert!(fight.act("dps", &press(ActionKind::Trinket(1))).is_ok());
    assert_eq!(
        fight.act("intruder", &press(ActionKind::Trinket(0))),
        Err(ActionError::NotInFight)
    );
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
fn the_drowned_catacombs_load_on_the_realm_path_and_can_be_cleared() {
    let (scene, barriers, players) = dungeon_party("drowned_catacombs");
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let monsters = fight.monster_states();
    assert_eq!(
        monsters.len(),
        scene.monsters.len(),
        "no unknown ids dropped"
    );
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
        let _ = fight.advance(30);
        saw_unlock |= fight.unlocked();
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(saw_unlock, "the gate opened");
    assert!(
        fight.completed().is_some(),
        "the Abbess died and the run completed"
    );
}

#[test]
fn the_bone_foundry_loads_on_the_realm_path_and_can_be_cleared() {
    let (scene, barriers, players) = dungeon_party("bone_foundry");
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let monsters = fight.monster_states();
    assert_eq!(
        monsters.len(),
        scene.monsters.len(),
        "no unknown ids dropped"
    );
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
        let _ = fight.advance(30);
        saw_unlock |= fight.unlocked();
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(saw_unlock, "the gate opened");
    assert!(
        fight.completed().is_some(),
        "the Overseer died and the run completed"
    );
}

#[test]
fn the_sanctum_of_ash_loads_on_the_realm_path_and_can_be_cleared() {
    let (scene, barriers, players) = dungeon_party("sanctum_of_ash");
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let monsters = fight.monster_states();
    assert_eq!(
        monsters.len(),
        scene.monsters.len(),
        "no unknown ids dropped"
    );
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
        let _ = fight.advance(30);
        saw_unlock |= fight.unlocked();
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(saw_unlock, "the gate opened");
    assert!(
        fight.completed().is_some(),
        "the Pontiff died and the run completed"
    );
}

#[test]
fn the_censer_warden_pays_each_player_one_item_from_its_own_table_where_it_falls() {
    let (scene, barriers, players) = dungeon_party("sanctum_of_ash");
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let rules = crate::loot::builtin_rules().expect("the shipped rules build");
    fight.set_loot(rules.clone(), 5, 0);
    fight.open_start_barrier();
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    let warden = fight
        .sim
        .encounters()
        .iter()
        .find(|encounter| encounter.name() == "GALLERY OF CENSERS")
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the Warden's pack");
    fight.sim.apply_damage(
        mage,
        warden,
        1.0e7,
        AbilityId::from_fire(FireAbility::Firebolt),
        DamageType::Fire,
    );
    let announced: Vec<engine_game::event::CombatEvent> = fight.sim.events().to_vec();
    let name = |entity: EntityId| fight.name(entity);
    let events: Vec<WireEvent> = announced
        .iter()
        .filter_map(|event| encode(event, &name))
        .collect();
    let table = rules.boss_table("censer_warden").expect("its table");
    let rows: Vec<&str> = table
        .entries
        .iter()
        .filter_map(|entry| engine_game::character::item(entry.item))
        .map(|def| def.key)
        .collect();
    let drops = crate::haul::drops_in(&events);
    for account in ["tank", "healer", "dps"] {
        let items: Vec<&str> = drops
            .iter()
            .filter_map(|drop| match drop {
                crate::haul::Drop::Item {
                    account: who, item, ..
                } if who == account => Some(item.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 1, "{account}: {items:?}");
        assert!(
            rows.contains(&items[0]),
            "{account}: {} is not its",
            items[0]
        );
        assert!(gold_of(&events, account) >= 1, "{account} was paid gold");
    }
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
fn a_warriors_talents_reach_the_simulation_and_stay_with_the_warrior() {
    let (scene, barriers, mut players) = crypt();
    players[0].talents = vec!["unyielding".into()];
    players[0].talent_points = 5;
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let tank = fight
        .resolve(&EntityRef::Player("tank".into()))
        .expect("tank");
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    let stacks = |fight: &mut CombatInstance, id| {
        fight
            .sim
            .with_player(id, |sim| sim.warrior_effects().last_stand_fervor_stacks)
            .expect("player")
    };
    assert_eq!(stacks(&mut fight, tank), 2);
    assert_eq!(stacks(&mut fight, healer), 0);
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

#[test]
fn a_channel_aim_steers_the_servers_beam_and_the_beam_crosses_the_wire() {
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
    let near = monster.position + Vec3::new(0.0, 0.0, 12.0);
    if let Some(body) = fight.sim.world_mut().body_mut(mage) {
        *body = Body::at(near);
    }
    fight.sim.with_player(mage, |sim| {
        sim.mage_mut().gain_energy(100.0);
    });
    let press = |kind| CombatAction {
        target: EntityRef::None,
        kind,
    };

    // The press that casts it: aimed at the monster.
    let aim = [monster.position.x, monster.position.y, monster.position.z];
    fight
        .act("dps", &press(ActionKind::Ultimate(aim)))
        .expect("a well-formed press");
    let events = fight.advance(90);
    assert!(
        events.iter().any(|event| matches!(
            event,
            WireEvent::BeamChannelStarted { entity, duration, .. }
                if *entity == EntityRef::Player("dps".into()) && (*duration - 6.0).abs() < 1e-6
        )),
        "the channel was announced by account"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            WireEvent::Damage { ability, source, .. }
                if *source == EntityRef::Player("dps".into())
                    && *ability == AbilityRef::Player(FireAbility::FireBeam.index() as u16)
        )),
        "the beam hurt something and the wire named the ability"
    );
    let channelling = fight
        .sim
        .with_player(mage, |sim| sim.is_channelling_beam())
        .expect("dps is here");
    assert!(channelling);

    // Steering is not a targeting press and moves the server's beam, not the client's.
    let before = fight
        .sim
        .with_player(mage, |sim| sim.beam().yaw())
        .expect("dps is here");
    let away = [
        monster.position.x + 30.0,
        monster.position.y,
        monster.position.z,
    ];
    fight
        .act("dps", &press(ActionKind::ChannelAim(away)))
        .expect("a well-formed aim");
    let events = fight.advance(60);
    let after = fight
        .sim
        .with_player(mage, |sim| sim.beam().yaw())
        .expect("dps is here");
    assert!(
        (after - before).abs() > 0.5,
        "the beam turned from {before} to {after}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, WireEvent::BeamYaw { .. })),
        "the new aim was reported to everyone else"
    );

    // Pressing the ultimate again cancels, and the ending crosses the wire.
    fight
        .act("dps", &press(ActionKind::Ultimate(aim)))
        .expect("a well-formed press");
    let events = fight.advance(2);
    assert!(events.iter().any(|event| matches!(
        event,
        WireEvent::BeamChannelEnded { reason, .. }
            if *reason == engine_game::firemage::BeamEnd::Cancelled.index()
    )));

    // A malformed aim is refused before it reaches the simulation.
    assert_eq!(
        fight.act("dps", &press(ActionKind::ChannelAim([f32::NAN, 0.0, 0.0]))),
        Err(ActionError::Malformed)
    );
    assert_eq!(
        fight.act("intruder", &press(ActionKind::ChannelAim([0.0; 3]))),
        Err(ActionError::NotInFight)
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
    cleared_with_seed(bonus, None)
}

/// As [`cleared_with_loot`], with the loot rolled from `seed` when one is given.
fn cleared_with_seed(bonus: u32, seed: Option<u64>) -> (CombatInstance, Vec<WireEvent>) {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let rules = crate::loot::builtin_rules().expect("the shipped rules build");
    fight.set_loot(rules.clone(), 5, bonus);
    if let Some(seed) = seed {
        fight.seed_loot(seed);
    }
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
        // The chest may also hold a gem; the equipment is two rolls.
        let items: Vec<_> = fight
            .loot_items_of(account)
            .into_iter()
            .filter(|item| {
                engine_game::character::item_by_key(item.key)
                    .is_some_and(|def| def.family != engine_game::character::ItemFamily::Gem)
            })
            .collect();
        assert_eq!(items.len(), 2, "{account} gets two chest items");
        for item in &items {
            assert!(
                (engine_game::character::QUALITY_MIN..=engine_game::character::QUALITY_MAX)
                    .contains(&item.props.quality),
                "{account}: quality {}",
                item.props.quality
            );
            assert!(!item.props.broken && item.props.sharpening <= 9);
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

/// Every player's chest items, as (item key, quality), for comparing two runs.
fn chests_of(fight: &CombatInstance) -> Vec<(String, u8)> {
    ["tank", "healer", "dps"]
        .iter()
        .flat_map(|account| fight.loot_items_of(account))
        .map(|item| (item.key.to_string(), item.props.quality))
        .collect()
}

#[test]
fn the_loot_seed_decides_the_chest_and_the_same_seed_rolls_it_again() {
    let (first, _) = cleared_with_seed(0, Some(11));
    let (again, _) = cleared_with_seed(0, Some(11));
    assert_eq!(chests_of(&first), chests_of(&again), "one seed, one chest");
    let chest = chests_of(&first);
    let differs = (12..20).any(|seed| chests_of(&cleared_with_seed(0, Some(seed)).0) != chest);
    assert!(differs, "other seeds roll other chests");
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

#[test]
fn a_reroll_changes_the_live_modifiers_and_the_reward_bonus() {
    use crate::modifiers::KeyModifier;
    let (scene, barriers, players) = crypt();
    let catalog = Catalog::builtin().expect("catalog");
    let rules = catalog.modifiers();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    assert_eq!(fight.reward_bonus(), 0);
    let mods = vec![KeyModifier::base("rotating_beams")];
    fight.set_modifiers(rules.resolve(5, &mods));
    assert_eq!(fight.simulation().modifiers().active().len(), 1);
    let before = fight.simulation().modifiers().active()[0].magnitude;
    // The reroll: new magnitude, resolved again and set on the live fight.
    let stronger = vec![KeyModifier {
        id: "rotating_beams".into(),
        magnitude_pct: 20,
    }];
    fight.set_modifiers(rules.resolve(5, &stronger));
    fight.set_reward_bonus(25);
    let after = fight.simulation().modifiers().active()[0].magnitude;
    assert!((after / before - 1.2).abs() < 1e-4, "{before} -> {after}");
    assert_eq!(fight.reward_bonus(), 25);
}

#[test]
fn the_operator_can_clear_half_the_dungeon_to_unlock_a_reroll() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    let (killed, total) = fight.kill_progress();
    assert_eq!(killed, 0);
    assert!(total > 2);
    let slain = fight.slay_placed_until(0.5);
    let (killed, total_after) = fight.kill_progress();
    assert_eq!(
        total_after, total,
        "the count of placed monsters does not change"
    );
    assert_eq!(killed, slain);
    assert!(killed * 2 >= total, "{killed} of {total}");
    // Asking again for the same share slays nobody more.
    assert_eq!(fight.slay_placed_until(0.5), 0);
}

fn accounts_in(fight: &mut CombatInstance) -> Vec<String> {
    fight
        .player_states()
        .into_iter()
        .map(|state| state.account_id)
        .collect()
}

fn kill(fight: &mut CombatInstance, account: &str) {
    let entity = fight
        .resolve(&EntityRef::Player(account.into()))
        .expect("a member");
    fight.sim.world_mut().damage(entity, 1.0e7);
}

#[test]
fn a_member_who_is_not_the_active_player_leaves_the_fight() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    assert!(fight.remove_member("healer"));
    assert!(!fight.remove_member("healer"), "only once");
    assert!(!fight.remove_member("stranger"));
    assert_eq!(accounts_in(&mut fight), ["tank", "dps"]);
    assert!(fight.resolve(&EntityRef::Player("healer".into())).is_none());
    assert!(!fight.simulation().world().contains(healer));
    assert!(!fight.simulation().is_player_entity(healer));
    assert_eq!(fight.simulation().player_count(), 2);
    assert_eq!(
        fight.act(
            "healer",
            &CombatAction {
                target: EntityRef::None,
                kind: ActionKind::Ability(0)
            }
        ),
        Err(ActionError::NotInFight)
    );
    assert!(fight.apply_frames("healer", &[walk(1)]).is_none());
    fight.advance(60);
}

#[test]
fn the_active_first_member_can_leave_and_the_others_carry_on() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let tank = fight
        .resolve(&EntityRef::Player("tank".into()))
        .expect("tank");
    assert_eq!(fight.simulation().player().entity, tank);
    assert!(fight.remove_member("tank"));
    assert_eq!(accounts_in(&mut fight), ["healer", "dps"]);
    assert!(!fight.simulation().world().contains(tank));
    assert_ne!(fight.simulation().player().entity, tank);
    // The classes stayed with their players.
    let healer = fight
        .resolve(&EntityRef::Player("healer".into()))
        .expect("healer");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    assert_eq!(
        fight.sim.with_player(healer, |s| s.hero()),
        Some(Hero::Druid)
    );
    assert_eq!(
        fight.sim.with_player(mage, |s| s.hero()),
        Some(Hero::FireMage)
    );
    // The remaining players still move and act.
    let (body, last) = fight.apply_frames("dps", &[walk(1), walk(2)]).expect("dps");
    assert_eq!(last, 2);
    assert!(body.position.z.is_finite());
    fight.advance(60);
    assert_eq!(accounts_in(&mut fight), ["healer", "dps"]);
    // The last member stays.
    assert!(fight.remove_member("healer"));
    assert!(!fight.remove_member("dps"));
    assert_eq!(accounts_in(&mut fight), ["dps"]);
}

#[test]
fn monsters_that_fought_a_leaver_turn_on_whoever_is_left() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let monster = fight.monster_states().into_iter().next().expect("monster");
    let entity = fight
        .resolve(&EntityRef::Monster(monster.key))
        .expect("monster");
    let tank = fight
        .resolve(&EntityRef::Player("tank".into()))
        .expect("tank");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    for (who, offset) in [(tank, 3.0), (mage, 6.0)] {
        if let Some(body) = fight.sim.world_mut().body_mut(who) {
            *body = Body::at(monster.position + Vec3::new(0.0, 0.0, offset));
        }
    }
    fight.sim.apply_damage(
        tank,
        entity,
        1.0,
        AbilityId::from_fire(FireAbility::Firebolt),
        DamageType::Fire,
    );
    fight.advance(60);
    let target_of = |fight: &CombatInstance| {
        fight
            .monster_states()
            .into_iter()
            .find(|m| m.key == monster.key)
            .map(|m| m.target)
    };
    assert_eq!(
        target_of(&fight),
        Some(EntityRef::Player("tank".into())),
        "the one who pulled is fought"
    );
    assert!(fight.remove_member("tank"));
    for _ in 0..4 {
        fight.advance(30);
        assert_ne!(
            target_of(&fight),
            Some(EntityRef::Player("tank".into())),
            "nobody fights a body that is gone"
        );
    }
    assert_eq!(
        target_of(&fight),
        Some(EntityRef::Player("dps".into())),
        "the monster found the player still standing there"
    );
    let brain = fight.sim.brain(entity).expect("brain");
    assert!(!brain.threat().contains(tank));
}

#[test]
fn a_wipe_needs_only_the_remaining_players_down() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    assert!(fight.remove_member("healer"));
    kill(&mut fight, "tank");
    assert!(!fight.wiped(), "the mage stands");
    kill(&mut fight, "dps");
    assert!(fight.wiped(), "every remaining player is down");
}

#[test]
fn a_run_completes_without_the_leaver_and_the_chest_is_not_theirs() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    assert!(fight.remove_member("tank"));
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    for _ in 0..400 {
        slay_all(&mut fight, mage);
        fight.advance(30);
        if fight.completed().is_some() {
            break;
        }
    }
    assert!(fight.completed().is_some(), "the run completed");
    assert!(fight.loot_of("dps").is_some());
    assert!(fight.loot_of("healer").is_some());
    assert!(fight.loot_of("tank").is_none(), "the leaver rolls nothing");
    assert!(fight.loot_items_of("tank").is_empty());
}

#[test]
fn the_meter_keeps_what_a_leaver_did_but_no_longer_lists_them() {
    let (scene, barriers, players) = crypt();
    let mut fight = CombatInstance::new(&scene, &players, &barriers, (1.0, 1.0));
    fight.open_start_barrier();
    let monster = fight.monster_states().into_iter().next().expect("monster");
    let mage = fight
        .resolve(&EntityRef::Player("dps".into()))
        .expect("dps");
    if let Some(body) = fight.sim.world_mut().body_mut(mage) {
        *body = Body::at(monster.position + Vec3::new(0.0, 0.0, 15.0));
    }
    fight
        .act(
            "dps",
            &CombatAction {
                target: EntityRef::Monster(monster.key),
                kind: ActionKind::Ability(0),
            },
        )
        .expect("a press");
    fight.advance(180);
    let before = fight.meter().blob("dps").expect("blob").overall.damage;
    assert!(before > 0, "the mage did damage");
    assert!(fight.remove_member("dps"));
    let after = fight.meter().blob("dps").expect("still measured");
    assert_eq!(after.overall.damage, before);
    assert!(fight.meter().accounts().all(|a| a != "dps"));
    assert!(fight.meter().blobs().iter().all(|(a, _)| a != "dps"));
    let rows = fight.meter_rows(1_000_000_000, true);
    assert!(rows.iter().all(|(account, _)| account != "dps"));
    assert!(
        fight
            .meter()
            .damage_report("dps", Vec::new())
            .is_some_and(|report| report.total_damage == before)
    );
}

#[test]
fn a_restored_fight_does_not_bring_a_departed_member_back() {
    let (scene, barriers, players) = crypt();
    let remaining: Vec<PlayerSpec> = players
        .into_iter()
        .filter(|spec| spec.account_id != "healer")
        .collect();
    let state = Restore {
        tick: 600,
        players: vec![PlayerRecord {
            account_id: "healer".into(),
            body: Body::at(Vec3::ZERO),
            health: 50.0,
            last_seq: 3,
        }],
        ..Restore::default()
    };
    let mut rebuilt =
        CombatInstance::restore(&scene, &remaining, &barriers, (1.0, 1.0), true, &state);
    assert_eq!(accounts_in(&mut rebuilt), ["tank", "dps"]);
    assert!(
        rebuilt
            .resolve(&EntityRef::Player("healer".into()))
            .is_none()
    );
}
