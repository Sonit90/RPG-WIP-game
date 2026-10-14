//! The character sheet against a live simulation: equip something, and watch the numbers the
//! rest of the game reads change.
//!
//! The unit suites cover each piece — the stat block's arithmetic, the item table's rules,
//! the move operation. This is the seam between them and the fight: a ring with Stamina on it
//! has to reach the player's health bar, a hero swap has to bring that hero's gear with it,
//! and a drag the rules refuse has to change nothing at all.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_game::arena::{self, ArenaConfig};
use engine_game::character::stat::{HEALTH_PER_STAMINA, StatGroup};
use engine_game::character::{
    BASE_MAIN_STAT, BASE_STAMINA, EquipSlot, ItemProps, ItemSlot, ItemStack, Stat, item_by_key,
    starting_item,
};
use engine_game::firemage::BASE_SPELL_POWER;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::warrior::PASSIVE_MAX_HEALTH_AT_ZERO;
use engine_game::{ItemFamily, PlayerCommand};

/// A running arena with one step taken, so everything is initialised.
///
/// Every hero is handed the white kit (four pieces worn, five in the bag): a new hero starts
/// with nothing, and these tests are about moving things around.
fn simulation() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    for hero in Hero::ALL {
        let sheet = simulation.roster_mut().sheet_mut(hero);
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let def = starting_item(slot.family()).expect("a white piece");
            sheet
                .equipment_mut()
                .equip(slot, ItemStack::one(def.id))
                .expect("fits");
        }
        for family in [
            ItemFamily::Head,
            ItemFamily::Shoulder,
            ItemFamily::Back,
            ItemFamily::Ring,
            ItemFamily::Trinket,
        ] {
            let def = starting_item(family).expect("a white piece");
            sheet.inventory_mut().add(ItemStack::one(def.id));
        }
    }
    simulation.step_once();
    simulation
}

#[test]
fn a_new_hero_has_no_items_at_all() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    for hero in Hero::ALL {
        let sheet = simulation.roster().sheet(hero);
        assert_eq!(sheet.equipment().count(), 0, "{hero:?}");
        assert_eq!(sheet.inventory().count(), 0, "{hero:?}");
    }
}

/// The player's maximum health as the world reports it.
fn max_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .map(engine_game::Health::max)
        .expect("the player has health")
}

/// The first bag cell holding an item that fits `slot`.
fn carried_for(simulation: &Simulation, slot: EquipSlot) -> usize {
    simulation
        .sheet()
        .inventory()
        .contents()
        .find(|(_, stack)| stack.fits(slot))
        .map(|(index, _)| index)
        .expect("something in the bag fits")
}

/// The first empty bag cell.
fn free_cell(simulation: &Simulation) -> usize {
    simulation
        .sheet()
        .inventory()
        .cells()
        .iter()
        .position(Option::is_none)
        .expect("a free cell")
}

#[test]
fn every_hero_starts_with_its_main_stat_stamina_and_no_secondary_stats() {
    let mut simulation = simulation();
    for (hero, main) in [
        (Hero::FireMage, Stat::Intellect),
        (Hero::Warrior, Stat::Strength),
        (Hero::Druid, Stat::Intellect),
    ] {
        simulation.set_hero(hero);
        let stats = simulation.character_stats();
        assert_eq!(simulation.sheet().main_stat(), main, "{hero:?}");
        assert_eq!(stats.get(main), BASE_MAIN_STAT);
        assert_eq!(stats.get(Stat::Stamina), BASE_STAMINA);
        for stat in Stat::ALL {
            if stat.group() == StatGroup::Secondary {
                assert_eq!(stats.get(stat), 0.0, "{hero:?} has {}", stat.name());
            }
        }
    }
}

#[test]
fn stamina_is_the_players_maximum_health_from_the_first_tick() {
    let simulation = simulation();
    assert_eq!(
        max_health(&simulation),
        BASE_STAMINA * HEALTH_PER_STAMINA,
        "the health bar and the character sheet disagree"
    );
}

#[test]
fn equipping_ten_stamina_raises_the_maximum_health_by_a_hundred() {
    let mut simulation = simulation();
    let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
    let cell = free_cell(&simulation);
    simulation
        .roster_mut()
        .sheet_mut(Hero::FireMage)
        .inventory_mut()
        .put(cell, Some(ItemStack::one(ring.id)));

    let before = max_health(&simulation);
    simulation
        .move_item(
            ItemSlot::Inventory(cell),
            ItemSlot::Equipment(EquipSlot::Ring1),
        )
        .expect("a ring goes on a finger");

    let after = max_health(&simulation);
    assert_eq!(after - before, 10.0 * HEALTH_PER_STAMINA);
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        BASE_STAMINA + 10.0
    );

    // And taking it off puts the number back.
    simulation
        .move_item(
            ItemSlot::Equipment(EquipSlot::Ring1),
            ItemSlot::Inventory(cell),
        )
        .expect("it comes off again");
    assert_eq!(max_health(&simulation), before);
}

#[test]
fn an_items_quality_enhancement_and_breakage_reach_the_players_health() {
    let mut simulation = simulation();
    let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
    let cell = free_cell(&simulation);
    let stamina_with = |simulation: &mut Simulation, props: ItemProps| {
        simulation
            .roster_mut()
            .sheet_mut(Hero::FireMage)
            .inventory_mut()
            .put(cell, Some(ItemStack::with_props(ring.id, props)));
        simulation
            .move_item(
                ItemSlot::Inventory(cell),
                ItemSlot::Equipment(EquipSlot::Ring1),
            )
            .expect("a ring goes on a finger");
        let stamina = simulation.character_stats().get(Stat::Stamina) - BASE_STAMINA;
        simulation
            .move_item(
                ItemSlot::Equipment(EquipSlot::Ring1),
                ItemSlot::Inventory(cell),
            )
            .expect("it comes off again");
        stamina
    };
    // A base of ten Stamina: 106 per cent quality with +7 is 113 per cent; a break leaves 85.
    assert!((stamina_with(&mut simulation, ItemProps::new(106, 7, false)) - 11.3).abs() < 1e-3);
    assert!((stamina_with(&mut simulation, ItemProps::new(100, 0, true)) - 8.5).abs() < 1e-3);
    assert!((stamina_with(&mut simulation, ItemProps::default()) - 10.0).abs() < 1e-3);
}

#[test]
fn changing_gear_does_not_heal_or_kill_the_character() {
    let mut simulation = simulation();
    let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
    let cell = free_cell(&simulation);
    simulation
        .roster_mut()
        .sheet_mut(Hero::FireMage)
        .inventory_mut()
        .put(cell, Some(ItemStack::one(ring.id)));

    // Hurt the player to half health, then change what they are wearing twice.
    let player = simulation.player().entity;
    let half = max_health(&simulation) * 0.5;
    simulation.world_mut().damage(player, half);
    let fraction = |simulation: &Simulation| {
        simulation
            .world()
            .health(player)
            .map(engine_game::Health::fraction)
            .unwrap_or(0.0)
    };
    assert!((fraction(&simulation) - 0.5).abs() < 1e-4);

    simulation
        .move_item(
            ItemSlot::Inventory(cell),
            ItemSlot::Equipment(EquipSlot::Ring1),
        )
        .expect("on");
    assert!(
        (fraction(&simulation) - 0.5).abs() < 1e-4,
        "equipping healed the player"
    );

    simulation
        .move_item(
            ItemSlot::Equipment(EquipSlot::Ring1),
            ItemSlot::Inventory(cell),
        )
        .expect("off");
    assert!(
        (fraction(&simulation) - 0.5).abs() < 1e-4,
        "unequipping hurt the player"
    );
    assert!(
        simulation.world().is_alive(player),
        "taking a stamina ring off killed the character"
    );
}

#[test]
fn the_main_stat_is_what_the_class_spends_as_spell_power() {
    let mut simulation = simulation();
    assert_eq!(simulation.mage().stats().spell_power, BASE_SPELL_POWER);

    // The bridge is stated as "gear is the difference", so a main stat above the base has to
    // show up as spell power above the base.
    let sheet = simulation.roster_mut().sheet_mut(Hero::FireMage);
    let stats_before = sheet.stats().get(Stat::Intellect);
    assert_eq!(stats_before, BASE_MAIN_STAT);
    simulation.refresh_character();
    assert_eq!(
        simulation.mage().stats().spell_power,
        simulation.character_stats().get(Stat::Intellect)
    );
}

#[test]
fn a_drag_the_rules_refuse_changes_nothing_and_is_reported() {
    let mut simulation = simulation();
    let before = simulation.sheet().clone();
    let cloak = carried_for(&simulation, EquipSlot::Back);

    let refused = simulation.move_item(
        ItemSlot::Inventory(cloak),
        ItemSlot::Equipment(EquipSlot::Ring1),
    );
    assert!(refused.is_err(), "a cloak reached a finger");
    assert_eq!(
        simulation.sheet(),
        &before,
        "a refused drag changed the sheet"
    );
}

#[test]
fn a_move_arriving_as_a_command_is_applied_on_the_next_tick() {
    let mut simulation = simulation();
    let ring = carried_for(&simulation, EquipSlot::Ring1);

    simulation.push_command(PlayerCommand::MoveItem {
        from: ItemSlot::Inventory(ring),
        to: ItemSlot::Equipment(EquipSlot::Ring1),
    });
    assert_eq!(
        simulation.sheet().equipment().get(EquipSlot::Ring1),
        None,
        "the command was applied before the tick"
    );

    simulation.step_once();
    assert!(
        simulation
            .sheet()
            .equipment()
            .get(EquipSlot::Ring1)
            .is_some(),
        "the command never landed"
    );
    assert_eq!(simulation.sheet().inventory().get(ring), None);
}

#[test]
fn an_illegal_move_in_the_command_stream_is_ignored_rather_than_fatal() {
    let mut simulation = simulation();
    let before = simulation.sheet().clone();
    let cloak = carried_for(&simulation, EquipSlot::Back);

    simulation.push_command(PlayerCommand::MoveItem {
        from: ItemSlot::Inventory(cloak),
        to: ItemSlot::Equipment(EquipSlot::Trinket2),
    });
    simulation.step_once();
    assert_eq!(simulation.sheet(), &before);
}

#[test]
fn swapping_hero_swaps_the_gear_the_bag_and_the_stats() {
    let mut simulation = simulation();

    // Undress the FireMage's chest, then become the Warrior.
    let cell = free_cell(&simulation);
    simulation
        .move_item(
            ItemSlot::Equipment(EquipSlot::Chest),
            ItemSlot::Inventory(cell),
        )
        .expect("the chest comes off");

    assert!(simulation.set_hero(Hero::Warrior));
    assert_eq!(simulation.sheet().hero(), Hero::Warrior);
    assert!(
        simulation
            .sheet()
            .equipment()
            .get(EquipSlot::Chest)
            .is_some(),
        "the Warrior inherited the FireMage's undressing"
    );
    let stats = simulation.character_stats();
    assert_eq!(stats.get(Stat::Strength), BASE_MAIN_STAT);
    assert_eq!(stats.get(Stat::Intellect), 0.0);
    // The Warrior's health is Stamina *and* what Deflection is currently worth. A fresh
    // Warrior is at zero Deflection, which is where the passive bonus is at its largest:
    // five per cent. See `engine_game::warrior::deflection`.
    let expected = BASE_STAMINA * HEALTH_PER_STAMINA * (1.0 + PASSIVE_MAX_HEALTH_AT_ZERO);
    assert!(
        (max_health(&simulation) - expected).abs() < 1e-3,
        "{} against {expected}",
        max_health(&simulation)
    );

    // And back: the FireMage is still missing a chestpiece.
    assert!(simulation.set_hero(Hero::FireMage));
    assert_eq!(simulation.sheet().equipment().get(EquipSlot::Chest), None);
    assert!(simulation.sheet().inventory().get(cell).is_some());
}

#[test]
fn a_ring_fits_either_finger_and_a_trinket_either_of_the_two() {
    let mut simulation = simulation();
    let ring = carried_for(&simulation, EquipSlot::Ring1);
    simulation
        .move_item(
            ItemSlot::Inventory(ring),
            ItemSlot::Equipment(EquipSlot::Ring2),
        )
        .expect("either finger takes it");
    simulation
        .move_item(
            ItemSlot::Equipment(EquipSlot::Ring2),
            ItemSlot::Equipment(EquipSlot::Ring1),
        )
        .expect("and it moves between them");

    let trinket = carried_for(&simulation, EquipSlot::Trinket1);
    for slot in [EquipSlot::Trinket1, EquipSlot::Trinket2] {
        let from = simulation
            .sheet()
            .inventory()
            .get(trinket)
            .map(|_| ItemSlot::Inventory(trinket))
            .unwrap_or_else(|| {
                ItemSlot::Equipment(
                    EquipSlot::ALL
                        .into_iter()
                        .find(|worn| {
                            worn.family() == ItemFamily::Trinket
                                && simulation.sheet().equipment().get(*worn).is_some()
                        })
                        .expect("the trinket is somewhere"),
                )
            });
        simulation
            .move_item(from, ItemSlot::Equipment(slot))
            .unwrap_or_else(|error| panic!("{} refused a trinket: {error:?}", slot.name()));
    }
}

#[test]
fn no_sequence_of_moves_ever_changes_how_many_items_a_character_owns() {
    let mut simulation = simulation();
    let count = |simulation: &Simulation| {
        simulation.sheet().equipment().count() + simulation.sheet().inventory().count()
    };
    let expected = count(&simulation);

    // Every worn slot against every bag cell, both directions. Most are refused; none of
    // them may lose or duplicate anything.
    for worn in EquipSlot::ALL {
        for cell in 0..simulation.sheet().inventory().len() {
            let _ = simulation.move_item(ItemSlot::Equipment(worn), ItemSlot::Inventory(cell));
            let _ = simulation.move_item(ItemSlot::Inventory(cell), ItemSlot::Equipment(worn));
            assert_eq!(count(&simulation), expected, "{} / {cell}", worn.name());
        }
    }
}

#[test]
fn the_white_kit_is_white_pieces_that_grant_nothing() {
    let mut simulation = simulation();
    for hero in Hero::ALL {
        simulation.set_hero(hero);
        let sheet = simulation.sheet();
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let stack = sheet
                .equipment()
                .get(slot)
                .unwrap_or_else(|| panic!("{hero:?} has no {}", slot.name()));
            let def = stack.def().expect("a real item");
            assert_eq!(Some(def.id), starting_item(slot.family()).map(|d| d.id));
            assert!(def.grants_nothing());
        }
        assert!(
            sheet.gear_stats().is_empty(),
            "{hero:?} has stats from gear"
        );
    }
}

#[test]
fn spirit_is_zero_and_nothing_the_sheet_does_makes_it_proc() {
    let mut simulation = simulation();
    assert_eq!(simulation.character_stats().get(Stat::Spirit), 0.0);

    // A full fight's worth of ticks, with the panel's own numbers read every one of them.
    for _ in 0..600 {
        simulation.step_once();
        let _ = simulation.character_stats();
    }
    assert_eq!(simulation.spirit().stacks(), 0, "Spirit procced at zero");
    assert_eq!(
        simulation.character_stats().get(Stat::Intellect),
        BASE_MAIN_STAT,
        "something scaled the main stat"
    );
}
