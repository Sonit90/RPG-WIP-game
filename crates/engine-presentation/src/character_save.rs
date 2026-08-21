//! The character file and the roster, in both directions.
//!
//! ```text
//! character.cfg  <->  CharacterConfig  <->  Roster  ->  the character panel
//!                     (names)              (items)
//! ```
//!
//! `engine-settings` links no engine crate, so what it stores is *names*: `"warrior"`,
//! `"ring1"`, `"plain_copper_band"`. `engine-game` owns what those names mean. This module is
//! the only place that knows both vocabularies, which is exactly the arrangement
//! [`crate::ui::action_bar`] already uses for the action bars, and for the same reason: the
//! translation is one function each way rather than a rule spread across a loader.
//!
//! # A name this build does not know is an empty slot
//!
//! Not an error, and not a refusal to load. A save written by a later build can name an item
//! that has since been removed; resolving it to nothing costs the player that item and
//! nothing else. The same is true of an item saved into a slot it no longer fits — the rule
//! is checked on the way in, so a file cannot put a cloak on a finger even by hand.

use engine_game::character::{
    CharacterSheet, EquipSlot, Inventory, ItemStack, Roster, item_by_key,
};
use engine_game::hero::Hero;
use engine_settings::character::{CharacterConfig, EquippedEntry, HeroLoadout, InventoryEntry};

/// Writes `roster` into a configuration ready to be saved.
///
/// Every hero is written, including one wearing nothing: an empty loadout is a fact about the
/// character — the player took everything off — and leaving it out would hand them their
/// starting gear back on the next launch.
#[must_use]
pub fn config_of(roster: &Roster) -> CharacterConfig {
    let mut config = CharacterConfig::default();
    for hero in Hero::ALL {
        config.set_loadout(loadout_of(roster.sheet(hero)));
    }
    config
}

/// Writes one character into a loadout.
#[must_use]
pub fn loadout_of(sheet: &CharacterSheet) -> HeroLoadout {
    let mut loadout = HeroLoadout::new(sheet.hero().config_name());
    for (slot, stack) in sheet.equipment().worn() {
        let Some(def) = stack.def() else {
            continue;
        };
        loadout
            .equipment
            .push(EquippedEntry::new(slot.key(), def.key));
    }
    for (index, stack) in sheet.inventory().contents() {
        let Some(def) = stack.def() else {
            continue;
        };
        loadout
            .inventory
            .push(InventoryEntry::new(index, def.key, stack.quantity));
    }
    loadout
}

/// Replaces every character in `roster` with what `config` saved for it.
///
/// A hero the file does not mention is left exactly as it was, which is what lets a save
/// written before a hero existed still load: the new hero keeps the starting gear the caller
/// dressed it in.
pub fn apply_config(config: &CharacterConfig, roster: &mut Roster) {
    for hero in Hero::ALL {
        let Some(loadout) = config.loadout(hero.config_name()) else {
            continue;
        };
        *roster.sheet_mut(hero) = sheet_of(hero, loadout);
    }
}

/// Builds one character from a loadout.
///
/// Every entry is checked on the way in — the slot has to exist, the item has to exist, and
/// the item has to fit — so a hand-edited file produces a legal character or a poorer one,
/// never an illegal one.
#[must_use]
pub fn sheet_of(hero: Hero, loadout: &HeroLoadout) -> CharacterSheet {
    let mut sheet = CharacterSheet::bare(hero);

    for entry in &loadout.equipment {
        let Some(slot) = EquipSlot::from_key(&entry.slot) else {
            log::warn!("character: {:?} is not a slot this build has", entry.slot);
            continue;
        };
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!("character: {:?} is not an item this build has", entry.item);
            continue;
        };
        if sheet
            .equipment_mut()
            .equip(slot, ItemStack::one(def.id))
            .is_err()
        {
            log::warn!(
                "character: {} does not go in the {} slot; leaving it empty",
                def.name,
                slot.name()
            );
        }
    }

    let mut inventory = Inventory::empty();
    for entry in &loadout.inventory {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!("character: {:?} is not an item this build has", entry.item);
            continue;
        };
        if entry.index >= inventory.len() {
            log::warn!(
                "character: cell {} is past the end of the bag; dropping {}",
                entry.index,
                def.name
            );
            continue;
        }
        inventory.put(entry.index, Some(ItemStack::new(def.id, entry.quantity)));
    }
    *sheet.inventory_mut() = inventory;

    sheet
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::character::{ItemFamily, ItemSlot, starting_item};

    #[test]
    fn a_roster_survives_a_round_trip_through_the_file_format() {
        let mut roster = Roster::starting();
        let ring = starting_item(ItemFamily::Ring).expect("a ring");
        let (index, _) = roster
            .sheet(Hero::Druid)
            .inventory()
            .contents()
            .find(|(_, stack)| stack.item == ring.id)
            .expect("the druid carries a ring");
        roster
            .sheet_mut(Hero::Druid)
            .move_item(
                ItemSlot::Inventory(index),
                ItemSlot::Equipment(EquipSlot::Ring2),
            )
            .expect("the ring goes on");

        let config = config_of(&roster);
        let mut loaded = Roster::bare();
        apply_config(&config, &mut loaded);

        assert_eq!(loaded, roster, "the roster changed on the way through");
        assert_eq!(
            loaded.sheet(Hero::Druid).equipment().get(EquipSlot::Ring2),
            Some(ItemStack::one(ring.id))
        );
    }

    #[test]
    fn every_hero_is_written_including_one_wearing_nothing() {
        let mut roster = Roster::starting();
        *roster.sheet_mut(Hero::Warrior) = CharacterSheet::bare(Hero::Warrior);
        let config = config_of(&roster);

        assert_eq!(config.heroes.len(), Hero::ALL.len());
        let warrior = config.loadout("warrior").expect("the warrior is written");
        assert!(warrior.equipment.is_empty());

        let mut loaded = Roster::starting();
        apply_config(&config, &mut loaded);
        assert_eq!(
            loaded.sheet(Hero::Warrior).equipment().count(),
            0,
            "an empty save handed the gear back"
        );
    }

    #[test]
    fn positions_in_the_bag_are_kept_exactly() {
        let mut roster = Roster::bare();
        let trinket = starting_item(ItemFamily::Trinket).expect("a trinket");
        roster
            .sheet_mut(Hero::FireMage)
            .inventory_mut()
            .put(11, Some(ItemStack::one(trinket.id)));

        let mut loaded = Roster::bare();
        apply_config(&config_of(&roster), &mut loaded);
        assert_eq!(
            loaded.sheet(Hero::FireMage).inventory().get(11),
            Some(ItemStack::one(trinket.id))
        );
        assert_eq!(loaded.sheet(Hero::FireMage).inventory().get(0), None);
    }

    #[test]
    fn a_hero_the_file_does_not_mention_keeps_what_it_had() {
        let mut config = CharacterConfig::default();
        config.set_loadout(HeroLoadout::new("firemage"));

        let mut roster = Roster::starting();
        apply_config(&config, &mut roster);
        assert_eq!(roster.sheet(Hero::FireMage).equipment().count(), 0);
        assert_eq!(
            roster.sheet(Hero::Druid).equipment().count(),
            4,
            "an unmentioned hero was undressed"
        );
    }

    #[test]
    fn an_item_the_build_does_not_have_resolves_to_an_empty_slot() {
        let mut config = CharacterConfig::default();
        config.set_loadout(HeroLoadout {
            hero: "warrior".to_owned(),
            equipment: vec![
                EquippedEntry::new("chest", "sword_of_a_later_patch"),
                EquippedEntry::new("legs", "simple_cloth_leggings"),
            ],
            inventory: vec![InventoryEntry::new(0, "also_not_a_thing", 1)],
        });

        let mut roster = Roster::bare();
        apply_config(&config, &mut roster);
        let sheet = roster.sheet(Hero::Warrior);
        assert_eq!(sheet.equipment().get(EquipSlot::Chest), None);
        assert!(sheet.equipment().get(EquipSlot::Legs).is_some());
        assert!(sheet.inventory().is_empty());
    }

    #[test]
    fn a_file_cannot_put_an_item_in_a_slot_it_does_not_fit() {
        let mut config = CharacterConfig::default();
        config.set_loadout(HeroLoadout {
            hero: "druid".to_owned(),
            // Hand-edited: a cloak on a finger.
            equipment: vec![EquippedEntry::new("ring1", "worn_travellers_cloak")],
            inventory: Vec::new(),
        });

        let mut roster = Roster::bare();
        apply_config(&config, &mut roster);
        assert_eq!(
            roster.sheet(Hero::Druid).equipment().get(EquipSlot::Ring1),
            None,
            "the file dressed the druid illegally"
        );
    }

    #[test]
    fn a_cell_past_the_end_of_the_bag_is_dropped_rather_than_panicking() {
        let mut config = CharacterConfig::default();
        config.set_loadout(HeroLoadout {
            hero: "firemage".to_owned(),
            equipment: Vec::new(),
            inventory: vec![InventoryEntry::new(900, "plain_copper_band", 1)],
        });

        let mut roster = Roster::bare();
        apply_config(&config, &mut roster);
        assert!(roster.sheet(Hero::FireMage).inventory().is_empty());
    }
}
