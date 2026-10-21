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
use engine_game::loot::{
    BossTable, CurrencyRules, EpicRules, GoldRules, LootRules, LootTable, MobLootRules,
    QualityBucket, QualityRules, WeightedItem,
};
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
    let (simulation, players, _, events) = finished_crypt_with(extra, None);
    (simulation, players, events)
}

/// [`finished_crypt`] with loot rules set from the start. Also answers every event from the
/// kills that came before the boss's, in order.
fn finished_crypt_with(
    extra: usize,
    rules: Option<(LootRules, u32, u32)>,
) -> (
    Simulation,
    Vec<EntityId>,
    Vec<CombatEvent>,
    Vec<CombatEvent>,
) {
    let mut simulation = build_level_file(level_path(), DungeonScaling::default())
        .expect("the crypt builds")
        .0;
    if let Some((rules, key_level, bonus)) = rules {
        simulation.set_loot_rules(rules, key_level, bonus);
    }
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
    // Kills made with `slay` announce themselves into the current event stream, which the
    // next step clears: read it after every slay, never after a step.
    let mut early = Vec::new();
    for index in 0..simulation.encounters().len() {
        if index == boss_index {
            continue;
        }
        for member in simulation.encounters()[index].members().to_vec() {
            simulation.slay(member);
        }
        early.extend_from_slice(simulation.events());
        simulation.step_once();
    }
    let boss = simulation.encounters()[boss_index].members()[0];
    let boss_at = simulation.world().position(boss).expect("a boss");
    put(&mut simulation, first, boss_at + Vec3::new(0.0, 0.0, 6.0));
    run(&mut simulation, 90);
    for add in simulation.dungeon_run().expect("a run").adds().to_vec() {
        simulation.slay(add);
    }
    early.extend_from_slice(simulation.events());
    simulation.step_once();
    simulation.slay(boss);
    let mut events = simulation.events().to_vec();
    events.extend(run(&mut simulation, 3));
    (simulation, players, early, events)
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

/// Each chest item as `(player, item, claimed)`: the legacy chest holds one per player.
fn rolls_of(simulation: &Simulation) -> impl Iterator<Item = (EntityId, ItemId, bool)> + '_ {
    simulation
        .chest_loot()
        .map(|(player, stack, claimed)| (player, stack.item, claimed))
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
    let a: Vec<_> = rolls_of(&first).collect();
    let b: Vec<_> = rolls_of(&second).collect();
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
    let (_, item, _) = rolls_of(&simulation).next().expect("a roll");
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
    assert!(rolls_of(&simulation).all(|(_, _, claimed)| claimed));
    assert!(simulation.loot_chest().expect("a chest").opened());
}

#[test]
fn each_player_gets_their_own_item() {
    let (mut simulation, players, _) = finished_crypt(1);
    land(&mut simulation);
    let rolls: Vec<_> = rolls_of(&simulation).collect();
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
    let (_, item, _) = rolls_of(&simulation).next().expect("a roll");

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
    assert!(rolls_of(&simulation).all(|(_, _, claimed)| claimed));
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
    let (_, item, _) = rolls_of(&simulation).next().expect("a roll");
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
    let (_, item, _) = rolls_of(&simulation).next().expect("a roll");

    let before = simulation.events().len();
    simulation.mail_unclaimed_loot();
    simulation.mail_unclaimed_loot();
    let events = simulation.events()[before..].to_vec();
    assert_eq!(mailed(&events), vec![(me, item, MailReason::Unclaimed)]);
    assert_eq!(simulation.mailbox().len(), 1);
    assert!(rolls_of(&simulation).all(|(_, _, claimed)| claimed));

    // The reset that follows finds nothing left to mail.
    simulation.reset_encounter();
    assert_eq!(simulation.mailbox().len(), 1, "mailed again on reset");
}

#[test]
fn a_reset_mails_what_nobody_took() {
    let (mut simulation, ..) = finished_crypt(0);
    let (_, item, _) = rolls_of(&simulation).next().expect("a roll");
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
    let rolls: Vec<_> = rolls_of(&simulation).collect();
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

// ---------------------------------------------------------------------------------------
// The data-driven rules: personal rolls from the boss's table, monster drops.
// ---------------------------------------------------------------------------------------

/// Registers a small catalog and answers rules over it, shaped like the shipped file.
fn rules(mob_equipment_chance: f32) -> LootRules {
    use engine_game::character::{
        CatalogItem, ItemFamily, ItemIcon, ItemQuality, Stat, item_by_key, register_catalog_items,
    };
    let row = |key: &str, quality, value| CatalogItem {
        key: key.to_owned(),
        name: key.to_owned(),
        family: ItemFamily::Ring,
        quality,
        item_level: 10,
        icon: ItemIcon::Ring,
        stats: vec![(Stat::Stamina, value)],
        active: None,
        set: None,
        sockets: 0,
        gem_level: 0,
    };
    register_catalog_items(vec![
        row("it_rare_ring", ItemQuality::Rare, 10.0),
        row("it_epic_ring", ItemQuality::Epic, 15.0),
    ]);
    let table = LootTable {
        entries: ["it_rare_ring", "it_epic_ring"]
            .map(|key| WeightedItem {
                item: item_by_key(key).expect("registered").id,
                weight: 1.0,
            })
            .to_vec(),
    };
    LootRules {
        equipment_per_completion: 2,
        quality: QualityRules {
            buckets: vec![
                QualityBucket {
                    min: 50,
                    max: 99,
                    weight: 95.0,
                },
                QualityBucket {
                    min: 100,
                    max: 110,
                    weight: 5.0,
                },
            ],
            high_from: 95,
            high_bucket_weight_per_key_level: 0.1,
        },
        epic: EpicRules {
            base_chance: 0.025,
            per_key_level: 0.002,
            max_chance: 0.08,
        },
        // Keyed by the crypt's boss archetype id.
        boss_tables: vec![BossTable {
            boss: "lord_ashgrave".to_owned(),
            table: table.clone(),
        }],
        mob: MobLootRules {
            equipment_chance: mob_equipment_chance,
            table,
        },
        currency: CurrencyRules {
            resource: "aether_shard",
            per_mob_chance: 0.0,
            elite_chance: 0.0,
            amount: 1,
        },
        gold: GoldRules {
            resource: "gold",
            per_mob_min: 2,
            per_mob_max: 2,
            boss_amount: 40,
        },
        sharpening: engine_game::loot::SharpeningDrops::none(),
        gems: engine_game::loot::GemDrops::none(),
    }
}

fn mob_loot(events: &[CombatEvent]) -> Vec<(EntityId, ItemId)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::MobLoot { player, item, .. } => Some((*player, *item)),
            _ => None,
        })
        .collect()
}

fn gold(events: &[CombatEvent]) -> Vec<(EntityId, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::MobCurrency {
                player,
                resource: "gold",
                amount,
                ..
            } => Some((*player, *amount)),
            _ => None,
        })
        .collect()
}

#[test]
fn every_player_gets_exactly_the_configured_number_of_rolls() {
    let (mut simulation, players, _, events) = finished_crypt_with(2, Some((rules(0.0), 1, 0)));
    let chest = simulation.loot_chest().expect("a chest");
    assert_eq!(chest.claims().len(), 3);
    for claim in chest.claims() {
        assert_eq!(claim.items.len(), 2, "not two rolls each");
    }
    assert_eq!(simulation.chest_loot().count(), 6);
    // Nothing is awarded until the chest is opened.
    assert!(awarded(&events).is_empty());

    land(&mut simulation);
    for &player in &players {
        stand_at_chest(&mut simulation, player);
    }
    let first = open(&mut simulation, players[0]);
    let got: Vec<_> = first
        .iter()
        .filter_map(|event| match event {
            CombatEvent::LootAwarded {
                player,
                item,
                props,
                index,
                ..
            } => Some((*player, *item, *props, *index)),
            _ => None,
        })
        .collect();
    assert_eq!(got.len(), 2);
    assert_eq!(
        [got[0].3, got[1].3],
        [0, 1],
        "items are indexed in roll order"
    );
    assert!(got.iter().all(|(player, ..)| *player == players[0]));
    let expected: Vec<_> = simulation
        .chest_loot()
        .filter(|(player, ..)| *player == players[0])
        .map(|(_, stack, _)| (stack.item, stack.props))
        .collect();
    assert_eq!(
        got.iter().map(|g| (g.1, g.2)).collect::<Vec<_>>(),
        expected,
        "the bag got something other than what was rolled"
    );
    // The bag holds both.
    assert_eq!(simulation.sheet().inventory().contents().count(), 2);
    // Exactly once: a second open takes nothing more.
    let again = open(&mut simulation, players[0]);
    assert!(awarded(&again).is_empty() && mailed(&again).is_empty());
    assert_eq!(simulation.sheet().inventory().contents().count(), 2);
}

#[test]
fn a_full_bag_mails_each_item_that_does_not_fit_with_its_props() {
    let (mut simulation, players, ..) = finished_crypt_with(0, Some((rules(0.0), 1, 0)));
    let me = players[0];
    let filler = ItemStack::one(loot_pool()[0].id);
    while bag(&mut simulation).add(filler).is_some() {}
    land(&mut simulation);
    stand_at_chest(&mut simulation, me);
    let events = open(&mut simulation, me);
    assert!(awarded(&events).is_empty());
    assert_eq!(mailed(&events).len(), 2);
    let letters = simulation.mailbox().letters();
    assert_eq!(letters.len(), 2);
    let rolled: Vec<_> = simulation
        .chest_loot()
        .map(|(_, stack, _)| (stack.item, stack.props))
        .collect();
    assert_eq!(
        letters
            .iter()
            .map(|letter| (letter.item, letter.props))
            .collect::<Vec<_>>(),
        rolled
    );
}

#[test]
fn unopened_shares_are_mailed_item_by_item_when_the_chest_is_cleared() {
    let (mut simulation, ..) = finished_crypt_with(1, Some((rules(0.0), 1, 0)));
    simulation.mail_unclaimed_loot();
    assert!(simulation.chest_loot().all(|(_, _, claimed)| claimed));
    let sent = simulation
        .events()
        .iter()
        .filter(|event| matches!(event, CombatEvent::LootMailed { .. }))
        .count();
    assert_eq!(sent, 4, "two items for each of two players");
}

#[test]
fn the_same_seed_gives_the_same_chest_and_another_seed_a_different_one() {
    let loot = |seed: Option<u64>| {
        let mut simulation = build_level_file(level_path(), DungeonScaling::default())
            .expect("the crypt builds")
            .0;
        simulation.set_loot_rules(rules(0.0), 4, 0);
        if let Some(seed) = seed {
            simulation.reseed_loot(seed);
        }
        simulation.spawn_loot_chest_at(Vec3::ZERO);
        simulation
            .chest_loot()
            .map(|(player, stack, _)| (player, stack))
            .collect::<Vec<_>>()
    };
    assert_eq!(loot(None), loot(None));
    assert_eq!(loot(Some(9)), loot(Some(9)));
    assert_ne!(loot(Some(9)), loot(Some(10)));
}

#[test]
fn the_reward_bonus_can_add_one_extra_roll() {
    // A bonus of 100 per cent is a certain extra roll for every player.
    let (simulation, ..) = finished_crypt_with(1, Some((rules(0.0), 1, 100)));
    for claim in simulation.loot_chest().expect("a chest").claims() {
        assert_eq!(claim.items.len(), 3);
    }
}

#[test]
fn normal_monsters_drop_for_every_player_and_the_boss_pays_gold_only() {
    let (_, players, early, late) = finished_crypt_with(2, Some((rules(1.0), 1, 0)));
    let mobs = mob_loot(&early);
    assert!(!mobs.is_empty());
    // A certain chance: every kill drops one for each of the three players.
    assert_eq!(mobs.len() % 3, 0);
    for &player in &players {
        assert_eq!(
            mobs.iter().filter(|(who, _)| *who == player).count(),
            mobs.len() / 3
        );
    }
    // Gold is 2 per kill per player, and the boss's 40 comes with its death, not as equipment.
    assert!(gold(&early).iter().all(|(_, amount)| *amount == 2));
    assert_eq!(gold(&early).len(), mobs.len());
    let boss_gold = gold(&late);
    assert_eq!(boss_gold.len(), 3);
    assert!(boss_gold.iter().all(|(_, amount)| *amount == 40));
    assert!(mob_loot(&late).is_empty(), "the boss rolled the mob table");
}

#[test]
fn a_zero_chance_drops_no_equipment_and_a_modest_one_drops_some() {
    let (_, _, early, _) = finished_crypt_with(2, Some((rules(0.0), 1, 0)));
    assert!(mob_loot(&early).is_empty());
    assert!(
        !gold(&early).is_empty(),
        "gold is not governed by the equipment chance"
    );
    let rolls = gold(&early).len();
    assert!(
        rolls >= 90,
        "the crypt has at least thirty monsters, rolled {rolls}"
    );

    // 0.25 over (kills x three players) rolls: well inside a generous band.
    let (_, _, early, _) = finished_crypt_with(2, Some((rules(0.25), 1, 0)));
    let hits = mob_loot(&early).len() as f32;
    assert!(
        (hits / rolls as f32 - 0.25).abs() < 0.15,
        "{hits} drops from {rolls} rolls"
    );
}

#[test]
fn a_client_under_a_remote_authority_rolls_nothing() {
    let mut simulation = build_level_file(level_path(), DungeonScaling::default())
        .expect("the crypt builds")
        .0;
    simulation.set_loot_rules(rules(1.0), 1, 0);
    simulation.set_remote_authority(true);
    simulation.step_once();
    let boss_index = simulation
        .dungeon_run()
        .and_then(|run| run.rules().boss_encounter)
        .expect("a boss");
    let mut events = Vec::new();
    for index in 0..simulation.encounters().len() {
        if index == boss_index {
            continue;
        }
        for member in simulation.encounters()[index].members().to_vec() {
            simulation.slay(member);
        }
        events.extend_from_slice(simulation.events());
        simulation.step_once();
    }
    assert!(mob_loot(&events).is_empty() && gold(&events).is_empty());
}
