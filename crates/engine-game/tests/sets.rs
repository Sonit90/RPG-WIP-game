//! Equipment sets against a live simulation: the stats reach the sheet, and each effect kind
//! changes the ability it names and nothing else.
//!
//! The registries are process-wide and set once, so this file installs one fixture world and
//! every test reads it. The set rules themselves (validation, parsing) are unit-tested in
//! `character::sets`; the shipped file is checked in `engine-realm`.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::sync::Once;

use engine_game::arena::{self, ArenaConfig};
use engine_game::character::{
    CatalogItem, CharacterSheet, EquipSlot, Equipment, ItemFamily, ItemIcon, ItemProps,
    ItemQuality, ItemStack, SetBonus, SetDef, SetEffect, Stat, active_sets, item_by_key,
    register_catalog_items, register_sets,
};
use engine_game::combat::DamageType;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::AbilityId;

fn piece(key: &str, family: ItemFamily, set: Option<&str>) -> CatalogItem {
    CatalogItem {
        key: key.to_owned(),
        name: key.to_owned(),
        family,
        quality: ItemQuality::Epic,
        item_level: 12,
        icon: ItemIcon::of_family(family),
        stats: vec![(Stat::Stamina, 10.0)],
        active: None,
        set: set.map(str::to_owned),
    }
}

fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        use ItemFamily::{Back, Chest, Head, Necklace, Ring, Wrist};
        register_catalog_items(vec![
            piece("s_head", Head, Some("s_major")),
            piece("s_chest", Chest, Some("s_major")),
            piece("s_back", Back, Some("s_major")),
            piece("s_ring", Ring, Some("s_minor")),
            piece("s_neck", Necklace, Some("s_minor")),
            piece("s_loose", Wrist, None),
        ]);
        register_sets(vec![
            SetDef {
                id: "s_major".to_owned(),
                name: "Test Major".to_owned(),
                pieces: vec![
                    "s_head".to_owned(),
                    "s_chest".to_owned(),
                    "s_back".to_owned(),
                ],
                bonuses: vec![
                    SetBonus {
                        count: 2,
                        stats: vec![(Stat::Haste, 10.0), (Stat::MainStat, 4.0)],
                        effects: vec![SetEffect::DamagePct {
                            ability: FireAbility::Firebolt,
                            value: 0.1,
                        }],
                    },
                    SetBonus {
                        count: 3,
                        stats: vec![(Stat::Stamina, 7.0)],
                        effects: vec![
                            SetEffect::CooldownPct {
                                ability: FireAbility::Meteor,
                                value: 0.2,
                            },
                            SetEffect::ResourceGenPct { value: 0.5 },
                        ],
                    },
                ],
            },
            SetDef {
                id: "s_minor".to_owned(),
                name: "Test Minor".to_owned(),
                pieces: vec!["s_ring".to_owned(), "s_neck".to_owned()],
                bonuses: vec![SetBonus {
                    count: 2,
                    stats: vec![(Stat::Versatility, 5.0)],
                    effects: Vec::new(),
                }],
            },
        ]);
    });
}

fn stack(key: &str) -> ItemStack {
    ItemStack::one(item_by_key(key).expect("a fixture item").id)
}

fn wearing(slots: &[(EquipSlot, &str)]) -> Equipment {
    install();
    let mut equipment = Equipment::empty();
    for (slot, key) in slots {
        equipment.equip(*slot, stack(key)).expect("fits");
    }
    equipment
}

const HEAD: (EquipSlot, &str) = (EquipSlot::Head, "s_head");
const CHEST: (EquipSlot, &str) = (EquipSlot::Chest, "s_chest");
const BACK: (EquipSlot, &str) = (EquipSlot::Back, "s_back");

#[test]
fn only_distinct_pieces_count_and_thresholds_switch_tiers_on() {
    assert!(active_sets(&wearing(&[])).is_empty());
    let one = active_sets(&wearing(&[HEAD]));
    let major = one.get("s_major").expect("one piece is progress");
    assert_eq!((major.equipped, major.tiers_active()), (1, 0));
    assert!(one.effects().is_none());

    let two = active_sets(&wearing(&[HEAD, CHEST]));
    assert_eq!(two.get("s_major").expect("worn").tiers_active(), 1);
    let three = active_sets(&wearing(&[HEAD, CHEST, BACK]));
    assert_eq!(three.get("s_major").expect("worn").tiers_active(), 2);
}

#[test]
fn two_copies_of_one_piece_are_one_piece() {
    install();
    let mut equipment = Equipment::empty();
    equipment
        .equip(EquipSlot::Ring1, stack("s_ring"))
        .expect("fits");
    let rolled = ItemStack::with_props(
        item_by_key("s_ring").expect("item").id,
        ItemProps::new(110, 10, false),
    );
    equipment.equip(EquipSlot::Ring2, rolled).expect("fits");
    let sets = active_sets(&equipment);
    assert_eq!(sets.get("s_minor").expect("worn").equipped, 1);
    assert!(sets.stats().is_empty());
}

#[test]
fn different_sets_and_loose_items_are_counted_apart() {
    let sets = active_sets(&wearing(&[
        HEAD,
        CHEST,
        (EquipSlot::Ring1, "s_ring"),
        (EquipSlot::Necklace, "s_neck"),
        (EquipSlot::Wrist, "s_loose"),
    ]));
    assert_eq!(sets.iter().count(), 2);
    assert_eq!(sets.get("s_major").expect("worn").equipped, 2);
    assert_eq!(sets.get("s_minor").expect("worn").equipped, 2);
    assert_eq!(sets.stats().get(Stat::Versatility), 5.0);
    assert_eq!(sets.stats().get(Stat::Haste), 10.0);
}

#[test]
fn set_stats_fold_into_gear_stats_unscaled_and_resolve_the_main_stat() {
    install();
    for hero in Hero::ALL {
        let mut sheet = CharacterSheet::bare(hero);
        let props = ItemProps::new(50, 0, false);
        for (slot, key) in [HEAD, CHEST] {
            sheet
                .equipment_mut()
                .equip(slot, ItemStack::with_props(stack(key).item, props))
                .expect("fits");
        }
        // The same two pieces at half quality: stamina is halved, the set is not.
        let gear = sheet.gear_stats();
        assert!((gear.get(Stat::Stamina) - 10.0).abs() < 1e-4, "{hero:?}");
        assert_eq!(gear.get(Stat::Haste), 10.0);
        assert_eq!(gear.get(hero.main_stat()), 4.0);
        assert_eq!(gear.get(Stat::MainStat), 0.0, "the placeholder leaked");
        assert_eq!(sheet.stats().get(Stat::Haste), 10.0);
        // One piece fewer, no bonus.
        sheet.equipment_mut().unequip(EquipSlot::Chest);
        assert_eq!(sheet.gear_stats().get(Stat::Haste), 0.0);
    }
}

fn mage_wearing(slots: &[(EquipSlot, &str)]) -> Simulation {
    install();
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    for (slot, key) in slots {
        simulation
            .roster_mut()
            .sheet_mut(Hero::FireMage)
            .equipment_mut()
            .equip(*slot, stack(key))
            .expect("fits");
    }
    simulation.refresh_character();
    simulation.select_nearest_target();
    simulation
}

fn damage_of(simulation: &mut Simulation, ability: FireAbility) -> f32 {
    let player = simulation.player().entity;
    let target = simulation.player().target.expect("a dummy is selected");
    simulation.apply_damage(
        player,
        target,
        1000.0,
        AbilityId::from_fire(ability),
        DamageType::Fire,
    );
    simulation
        .events()
        .iter()
        .rev()
        .find_map(|event| event.as_damage().map(|damage| damage.amount))
        .expect("the hit landed")
}

#[test]
fn damage_pct_raises_only_the_named_ability_and_only_at_its_tier() {
    let mut none = mage_wearing(&[]);
    let mut one = mage_wearing(&[HEAD]);
    let mut two = mage_wearing(&[HEAD, CHEST]);
    let base = damage_of(&mut none, FireAbility::Firebolt);
    assert!((damage_of(&mut one, FireAbility::Firebolt) - base).abs() < 1e-3);
    let boosted = damage_of(&mut two, FireAbility::Firebolt);
    assert!((boosted / base - 1.1).abs() < 1e-3, "{boosted} / {base}");
    let other_base = damage_of(&mut none, FireAbility::Pyroblast);
    let other = damage_of(&mut two, FireAbility::Pyroblast);
    assert!((other - other_base).abs() < 1e-3, "an unnamed ability changed");
}

#[test]
fn cooldown_pct_shortens_the_named_cooldown_and_is_lost_with_the_piece() {
    let plain = mage_wearing(&[]);
    let mut full = mage_wearing(&[HEAD, CHEST, BACK]);
    let effects = *plain.mage().effects();
    let cooldown = |sim: &Simulation, ability| sim.mage().abilities.cooldown(ability, &effects);
    let base = cooldown(&plain, FireAbility::Meteor);
    assert!(base > 0.0);
    assert!((cooldown(&full, FireAbility::Meteor) - base * 0.8).abs() < 1e-4);
    assert_eq!(
        cooldown(&full, FireAbility::Pyroblast),
        cooldown(&plain, FireAbility::Pyroblast)
    );
    // Taking a piece off drops the tier.
    full.roster_mut()
        .sheet_mut(Hero::FireMage)
        .equipment_mut()
        .unequip(EquipSlot::Back);
    full.refresh_character();
    assert!((cooldown(&full, FireAbility::Meteor) - base).abs() < 1e-4);
}

#[test]
fn resource_gen_pct_scales_what_a_cast_pays_into_heat() {
    let heat_after_firebolt = |slots: &[(EquipSlot, &str)]| {
        let mut simulation = mage_wearing(slots);
        simulation.step_until_ready();
        simulation.cast(FireAbility::Firebolt).expect("cast");
        for _ in 0..600 {
            if simulation.casting().is_none() {
                break;
            }
            simulation.step_once();
        }
        simulation.mage().heat.value()
    };
    let base = heat_after_firebolt(&[]);
    let full = heat_after_firebolt(&[HEAD, CHEST, BACK]);
    assert!(base > 0.0);
    assert!((full / base - 1.5).abs() < 0.05, "{full} / {base}");
}
