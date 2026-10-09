//! The boss's chest: it drops once when the dungeon completes, rolls one item per player
//! from the loot pool, and each player takes their own item exactly once.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::character::{ItemId, ItemStack, MailReason, loot_pool};
use engine_game::command::PlayerCommand;
use engine_game::dungeon::{DungeonScaling, GRAVEYARD_CRYPT_FILE, build_level_file};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::{CHEST_FALL_SECONDS, CHEST_INTERACT_RANGE, Simulation};
use engine_game::world::EntityDesc;

fn level_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(GRAVEYARD_CRYPT_FILE)
}

fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

fn put(simulation: &mut Simulation, entity: EntityId, position: Vec3) {
    if let Some(body) = simulation.world_mut().body_mut(entity) {
        *body = engine_game::Body::at(position);
    }
}

/// Plays the crypt to its end with `extra` more players in the party, and returns the
/// simulation and every event from the completion tick onwards.
fn finished_crypt(extra: usize) -> (Simulation, Vec<EntityId>, Vec<CombatEvent>) {
    let mut simulation = build_level_file(level_path(), DungeonScaling::default())
        .expect("the crypt builds")
        .0;
    let first = simulation.player().entity;
    simulation.world_mut().set_max_health(first, 1_000_000.0);
    simulation.world_mut().restore(first);
    let mut players = vec![first];
    for _ in 0..extra {
        let entity = simulation
            .world_mut()
            .spawn(EntityDesc::player(Vec3::new(0.0, 0.0, 0.0)));
        simulation.add_player(entity, Hero::Warrior);
        players.push(entity);
    }
    simulation.step_once();

    let boss_index = simulation
        .dungeon_run()
        .and_then(|run| run.rules().boss_encounter)
        .expect("a boss");
    for index in 0..simulation.encounters().len() {
        if index == boss_index {
            continue;
        }
        for member in simulation.encounters()[index].members().to_vec() {
            simulation.slay(member);
        }
        simulation.step_once();
    }
    let boss = simulation.encounters()[boss_index].members()[0];
    let boss_at = simulation.world().position(boss).expect("a boss");
    put(&mut simulation, first, boss_at + Vec3::new(0.0, 0.0, 6.0));
    run(&mut simulation, 90);
    for add in simulation.dungeon_run().expect("a run").adds().to_vec() {
        simulation.slay(add);
    }
    simulation.step_once();
    simulation.slay(boss);
    let events = run(&mut simulation, 3);
    (simulation, players, events)
}

fn awarded(events: &[CombatEvent]) -> Vec<(EntityId, ItemId)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::LootAwarded { player, item, .. } => Some((*player, *item)),
            _ => None,
        })
        .collect()
}

fn mailed(events: &[CombatEvent]) -> Vec<(EntityId, ItemId, MailReason)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::LootMailed {
                player,
                item,
                reason,
                ..
            } => Some((*player, *item, *reason)),
            _ => None,
        })
        .collect()
}

fn count(events: &[CombatEvent], pick: impl Fn(&CombatEvent) -> bool) -> usize {
    events.iter().filter(|event| pick(event)).count()
}

/// Stands `player` next to the chest.
fn stand_at_chest(simulation: &mut Simulation, player: EntityId) {
    let at = simulation.loot_chest().expect("a chest").position();
    put(simulation, player, at + Vec3::new(1.0, 0.0, 0.0));
}

fn open(simulation: &mut Simulation, player: EntityId) -> Vec<CombatEvent> {
    assert!(simulation.push_command_for(player, PlayerCommand::OpenChest));
    run(simulation, 2)
}

fn bag(simulation: &mut Simulation) -> &mut engine_game::character::Inventory {
    let hero = simulation.hero();
    simulation.roster_mut().sheet_mut(hero).inventory_mut()
}

fn land(simulation: &mut Simulation) {
    run(simulation, (CHEST_FALL_SECONDS * 60.0) as u32 + 10);
}

fn carries(simulation: &mut Simulation, player: EntityId, item: ItemId) -> bool {
    simulation
        .with_player(player, |sim| {
            sim.sheet()
                .inventory()
                .contents()
                .any(|(_, stack)| stack.item == item)
        })
        .expect("a player")
}

#[test]
fn the_chest_drops_once_when_the_boss_dies() {
    let (mut simulation, players, events) = finished_crypt(0);
    let spawned: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::ChestSpawned {
                drop_height,
                fall_seconds,
                ..
            } => Some((*drop_height, *fall_seconds)),
            _ => None,
        })
        .collect();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].0, engine_game::sim::CHEST_DROP_HEIGHT);
    assert_eq!(spawned[0].1, CHEST_FALL_SECONDS);
    let chest = simulation.loot_chest().expect("a chest");
    assert!(!chest.opened());
    assert_eq!(chest.claims().len(), players.len());

    let later = run(&mut simulation, 120);
    assert_eq!(
        count(&later, |e| matches!(e, CombatEvent::ChestSpawned { .. })),
        0,
        "the chest dropped again"
    );
}

#[test]
fn rolls_are_deterministic_and_come_from_the_pool() {
    let (first, ..) = finished_crypt(2);
    let (second, ..) = finished_crypt(2);
    let a: Vec<_> = first.chest_loot().collect();
    let b: Vec<_> = second.chest_loot().collect();
    assert_eq!(a.len(), 3);
    assert_eq!(a, b, "the same fight rolled different loot");
    for (_, item, claimed) in a {
        assert!(!claimed);
        assert!(loot_pool().iter().any(|def| def.id == item));
    }
}

#[test]
fn it_cannot_be_opened_before_it_lands_or_from_afar() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    stand_at_chest(&mut simulation, me);

    // Still falling.
    let early = open(&mut simulation, me);
    assert!(awarded(&early).is_empty(), "opened mid-fall");
    assert!(!simulation.loot_chest().expect("a chest").opened());

    land(&mut simulation);
    let at = simulation.loot_chest().expect("a chest").position();
    put(
        &mut simulation,
        me,
        at + Vec3::new(CHEST_INTERACT_RANGE + 2.0, 0.0, 0.0),
    );
    let far = open(&mut simulation, me);
    assert!(awarded(&far).is_empty(), "opened from out of range");
    assert!(!simulation.loot_chest().expect("a chest").opened());

    stand_at_chest(&mut simulation, me);
    let near = open(&mut simulation, me);
    assert_eq!(awarded(&near).len(), 1);
}

#[test]
fn opening_twice_gives_one_item_and_one_open_event() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    land(&mut simulation);
    stand_at_chest(&mut simulation, me);
    let (_, item, _) = simulation.chest_loot().next().expect("a roll");
    let bag_before = simulation.sheet().inventory().count();

    let first = open(&mut simulation, me);
    let second = open(&mut simulation, me);
    assert_eq!(awarded(&first), vec![(me, item)]);
    assert!(awarded(&second).is_empty());
    assert_eq!(
        count(&first, |e| matches!(e, CombatEvent::ChestOpened { .. })),
        1
    );
    assert_eq!(
        count(&second, |e| matches!(e, CombatEvent::ChestOpened { .. })),
        0
    );
    assert_eq!(simulation.sheet().inventory().count(), bag_before + 1);
    assert!(carries(&mut simulation, me, item));
    assert!(simulation.chest_loot().all(|(_, _, claimed)| claimed));
    assert!(simulation.loot_chest().expect("a chest").opened());
}

#[test]
fn each_player_gets_their_own_item() {
    let (mut simulation, players, _) = finished_crypt(1);
    land(&mut simulation);
    let rolls: Vec<_> = simulation.chest_loot().collect();
    assert_eq!(rolls.len(), 2);

    for &player in &players {
        stand_at_chest(&mut simulation, player);
    }
    let first = open(&mut simulation, players[0]);
    assert_eq!(awarded(&first), vec![(players[0], rolls[0].1)]);
    assert!(
        !carries(&mut simulation, players[1], rolls[1].1) || rolls[0].1 == rolls[1].1,
        "the second player was paid by the first player's open"
    );

    let second = open(&mut simulation, players[1]);
    assert_eq!(awarded(&second), vec![(players[1], rolls[1].1)]);
    assert_eq!(
        count(&second, |e| matches!(e, CombatEvent::ChestOpened { .. })),
        0,
        "the chest opens once"
    );
    assert!(carries(&mut simulation, players[0], rolls[0].1));
    assert!(carries(&mut simulation, players[1], rolls[1].1));
}

#[test]
fn a_full_bag_mails_the_item_and_spends_the_claim() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    land(&mut simulation);
    stand_at_chest(&mut simulation, me);
    let (_, item, _) = simulation.chest_loot().next().expect("a roll");

    // Fill every free cell with filler.
    let filler = ItemStack::one(loot_pool()[0].id);
    while bag(&mut simulation).add(filler).is_some() {}

    let refused = open(&mut simulation, me);
    assert!(awarded(&refused).is_empty());
    assert_eq!(
        mailed(&refused),
        vec![(me, item, MailReason::BagFull)],
        "the item was not mailed"
    );
    assert!(simulation.chest_loot().all(|(_, _, claimed)| claimed));
    let letters = simulation.mailbox().letters();
    assert_eq!(letters.len(), 1);
    assert_eq!(letters[0].item, item);
    assert_eq!(letters[0].reason, MailReason::BagFull);
    assert!(!letters[0].read);

    // Opening again does nothing: the claim is spent.
    let again = open(&mut simulation, me);
    assert!(mailed(&again).is_empty());
    assert_eq!(simulation.mailbox().len(), 1);
}

#[test]
fn taking_mail_moves_it_into_the_bag() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    land(&mut simulation);
    stand_at_chest(&mut simulation, me);
    let (_, item, _) = simulation.chest_loot().next().expect("a roll");
    let filler = ItemStack::one(loot_pool()[0].id);
    while bag(&mut simulation).add(filler).is_some() {}
    open(&mut simulation, me);
    let id = simulation.mailbox().letters()[0].id;

    // Still full: refused, letter kept.
    assert!(simulation.push_command_for(me, PlayerCommand::TakeMail { id }));
    let refused = run(&mut simulation, 2);
    assert_eq!(
        count(&refused, |e| matches!(e, CombatEvent::MailBagFull { .. })),
        1
    );
    assert_eq!(simulation.mailbox().len(), 1);

    // Make room.
    bag(&mut simulation).take(0);
    let count_before = simulation.sheet().inventory().count();
    assert!(simulation.push_command_for(me, PlayerCommand::TakeMail { id }));
    let taken = run(&mut simulation, 2);
    assert!(taken.iter().any(|e| matches!(
        e,
        CombatEvent::MailTaken { player, item: got, .. } if *player == me && *got == item
    )));
    assert!(simulation.mailbox().is_empty());
    assert_eq!(simulation.sheet().inventory().count(), count_before + 1);
}

#[test]
fn marking_mail_read_clears_the_unread_count() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    simulation.mail_unclaimed_loot();
    let id = simulation.mailbox().letters()[0].id;
    assert_eq!(simulation.sheet().unread_mail_count(), 1);
    assert!(simulation.push_command_for(me, PlayerCommand::MarkMailRead { id }));
    run(&mut simulation, 2);
    assert_eq!(simulation.sheet().unread_mail_count(), 0);
    assert_eq!(simulation.mailbox().len(), 1, "reading removed the letter");
}

#[test]
fn unclaimed_loot_is_mailed_once_however_often_it_is_asked() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    let (_, item, _) = simulation.chest_loot().next().expect("a roll");

    let before = simulation.events().len();
    simulation.mail_unclaimed_loot();
    simulation.mail_unclaimed_loot();
    let events = simulation.events()[before..].to_vec();
    assert_eq!(mailed(&events), vec![(me, item, MailReason::Unclaimed)]);
    assert_eq!(simulation.mailbox().len(), 1);
    assert!(simulation.chest_loot().all(|(_, _, claimed)| claimed));

    // The reset that follows finds nothing left to mail.
    simulation.reset_encounter();
    assert_eq!(simulation.mailbox().len(), 1, "mailed again on reset");
}

#[test]
fn a_reset_mails_what_nobody_took() {
    let (mut simulation, ..) = finished_crypt(0);
    let (_, item, _) = simulation.chest_loot().next().expect("a roll");
    simulation.reset_encounter();
    assert!(simulation.loot_chest().is_none());
    let letters = simulation.mailbox().letters();
    assert_eq!(letters.len(), 1);
    assert_eq!(
        (letters[0].item, letters[0].reason),
        (item, MailReason::Unclaimed)
    );
}

#[test]
fn a_claimed_item_is_not_mailed_again() {
    let (mut simulation, players, _) = finished_crypt(0);
    let me = players[0];
    land(&mut simulation);
    stand_at_chest(&mut simulation, me);
    open(&mut simulation, me);
    simulation.reset_encounter();
    assert!(simulation.mailbox().is_empty());
}

#[test]
fn each_players_mail_goes_to_their_own_sheet() {
    let (mut simulation, players, _) = finished_crypt(1);
    let rolls: Vec<_> = simulation.chest_loot().collect();
    simulation.mail_unclaimed_loot();

    let first = simulation
        .with_player(players[0], |sim| sim.mailbox().letters().to_vec())
        .expect("a player");
    let second = simulation
        .with_player(players[1], |sim| sim.mailbox().letters().to_vec())
        .expect("a player");
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(first[0].item, rolls[0].1);
    assert_eq!(second[0].item, rolls[1].1);
    // Only the hero the joiner rolled with holds the letter.
    let total = simulation
        .with_player(players[1], |sim| {
            sim.roster()
                .sheets()
                .map(|sheet| sheet.mailbox().len())
                .sum::<usize>()
        })
        .expect("a player");
    assert_eq!(total, 1);
}

#[test]
fn a_reset_clears_the_chest() {
    let (mut simulation, ..) = finished_crypt(0);
    assert!(simulation.loot_chest().is_some());
    simulation.reset_encounter();
    assert!(simulation.loot_chest().is_none());
}
