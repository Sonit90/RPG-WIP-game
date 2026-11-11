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
    CharacterSheet, EquipSlot, Inventory, ItemStack, MailItem, MailReason, MailSender, Roster,
    item_by_key,
};
use engine_game::hero::Hero;
use engine_settings::character::{
    CharacterConfig, EquippedEntry, HeroLoadout, InventoryEntry, MailEntry,
};

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
    for letter in sheet.mailbox().letters() {
        let Some(def) = engine_game::character::item(letter.item) else {
            continue;
        };
        loadout.mail.push(MailEntry {
            id: letter.id,
            item: def.key.to_owned(),
            sender: sender_key(letter.sender).to_owned(),
            reason: reason_key(letter.reason).to_owned(),
            received_at: letter.received_at,
            read: letter.read,
        });
    }
    loadout.next_mail_id = sheet.mailbox().next_id();
    loadout
}

fn sender_key(sender: MailSender) -> &'static str {
    match sender {
        MailSender::DungeonChest => "dungeon_chest",
        MailSender::System => "system",
        MailSender::Dungeon => "dungeon",
    }
}

fn reason_key(reason: MailReason) -> &'static str {
    match reason {
        MailReason::BagFull => "bag_full",
        MailReason::Unclaimed => "unclaimed",
        MailReason::SlotRemoved => "slot_removed",
        MailReason::DungeonDrop => "dungeon_drop",
    }
}

/// A sender this build does not know is filed under the only sender there is.
fn sender_of(key: &str) -> MailSender {
    match key {
        "dungeon_chest" => MailSender::DungeonChest,
        "system" => MailSender::System,
        "dungeon" => MailSender::Dungeon,
        other => {
            log::warn!("character: unknown mail sender {other:?}");
            MailSender::DungeonChest
        }
    }
}

/// A reason this build does not know reads as the gentler one: the player simply has an item.
fn reason_of(key: &str) -> MailReason {
    match key {
        "bag_full" => MailReason::BagFull,
        "unclaimed" => MailReason::Unclaimed,
        "slot_removed" => MailReason::SlotRemoved,
        "dungeon_drop" => MailReason::DungeonDrop,
        other => {
            log::warn!("character: unknown mail reason {other:?}");
            MailReason::Unclaimed
        }
    }
}

/// Replaces every character in `roster` with what `config` saved for it.
///
/// A hero the file does not mention is left exactly as it was, which is what lets a save
/// written before a hero existed still load: the new hero keeps what the caller gave it.
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
///
/// **An item saved in a slot that has since been removed** (`trinket3`, `trinket4`) is not
/// lost: it goes to the first free bag cell, or, when the bag is full, into the mailbox.
#[must_use]
pub fn sheet_of(hero: Hero, loadout: &HeroLoadout) -> CharacterSheet {
    let mut sheet = CharacterSheet::bare(hero);
    let mut rehome = Vec::new();

    for entry in &loadout.equipment {
        let Some(slot) = EquipSlot::from_key(&entry.slot) else {
            if EquipSlot::REMOVED_KEYS.contains(&entry.slot.as_str())
                && let Some(def) = item_by_key(&entry.item)
            {
                rehome.push(ItemStack::one(def.id));
                continue;
            }
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

    // An item this build no longer has cannot be shown or taken, so its letter goes with it;
    // every other letter keeps its id, and the counter never falls behind them.
    for entry in &loadout.mail {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!("character: {:?} is not an item this build has", entry.item);
            continue;
        };
        sheet.mailbox_mut().restore(MailItem {
            id: entry.id,
            item: def.id,
            props: engine_game::character::ItemProps::STANDARD,
            sender: sender_of(&entry.sender),
            reason: reason_of(&entry.reason),
            received_at: entry.received_at,
            read: entry.read,
        });
    }
    sheet.mailbox_mut().reserve_ids_up_to(loadout.next_mail_id);

    if !rehome.is_empty() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64());
        for stack in rehome {
            if sheet.rehome_removed_slot_item(stack, now).is_none() {
                log::info!("character: a bag was full; mailed an item from a removed slot");
            }
        }
    }

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

    /// Every hero wearing four white pieces and carrying five, as new heroes once started.
    fn dressed_roster() -> Roster {
        let mut roster = Roster::bare();
        for hero in Hero::ALL {
            let sheet = roster.sheet_mut(hero);
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
        roster
    }

    fn old_save_with_trinkets_in_slots_three_and_four() -> CharacterConfig {
        let trinket = starting_item(ItemFamily::Trinket).expect("a trinket");
        let mut config = CharacterConfig::default();
        config.set_loadout(HeroLoadout {
            hero: "warrior".to_owned(),
            equipment: vec![
                EquippedEntry::new("trinket1", trinket.key),
                EquippedEntry::new("trinket3", trinket.key),
                EquippedEntry::new("trinket4", trinket.key),
            ],
            inventory: vec![InventoryEntry::new(0, trinket.key, 1)],
            ..HeroLoadout::default()
        });
        config
    }

    #[test]
    fn an_item_in_a_removed_trinket_slot_moves_to_the_first_free_bag_cell() {
        let mut roster = Roster::bare();
        apply_config(
            &old_save_with_trinkets_in_slots_three_and_four(),
            &mut roster,
        );
        let sheet = roster.sheet(Hero::Warrior);
        assert!(sheet.equipment().get(EquipSlot::Trinket1).is_some());
        assert_eq!(sheet.equipment().count(), 1);
        // Cell 0 was taken; the two displaced trinkets take 1 and 2.
        assert_eq!(sheet.inventory().count(), 3);
        assert!(sheet.inventory().get(1).is_some());
        assert!(sheet.inventory().get(2).is_some());
        assert!(sheet.mailbox().is_empty());
    }

    #[test]
    fn an_item_in_a_removed_trinket_slot_is_mailed_when_the_bag_is_full() {
        let trinket = starting_item(ItemFamily::Trinket).expect("a trinket");
        let mut config = old_save_with_trinkets_in_slots_three_and_four();
        let mut loadout = config.loadout("warrior").expect("warrior").clone();
        loadout.inventory = (0..engine_game::character::INVENTORY_SLOTS)
            .map(|cell| InventoryEntry::new(cell, trinket.key, 1))
            .collect();
        config.set_loadout(loadout);

        let mut roster = Roster::bare();
        apply_config(&config, &mut roster);
        let sheet = roster.sheet(Hero::Warrior);
        assert_eq!(
            sheet.inventory().count(),
            engine_game::character::INVENTORY_SLOTS
        );
        assert_eq!(sheet.mailbox().len(), 2);
        for letter in sheet.mailbox().letters() {
            assert_eq!(letter.reason, MailReason::SlotRemoved);
            assert_eq!(letter.item, trinket.id);
        }

        // The migrated roster saves and loads again without losing or doubling anything.
        let mut again = Roster::bare();
        apply_config(&config_of(&roster), &mut again);
        assert_eq!(again, roster);
    }

    #[test]
    fn a_roster_survives_a_round_trip_through_the_file_format() {
        let mut roster = dressed_roster();
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
    fn mail_survives_a_round_trip_through_the_file_format() {
        let ring = starting_item(ItemFamily::Ring).expect("a ring");
        let mut roster = dressed_roster();
        let mailbox = roster.sheet_mut(Hero::Warrior).mailbox_mut();
        let first = mailbox.send(ring.id, MailSender::DungeonChest, MailReason::BagFull, 42.5);
        let second = mailbox.send(
            ring.id,
            MailSender::DungeonChest,
            MailReason::Unclaimed,
            99.0,
        );
        mailbox.mark_read(first);
        let ids_before = (first, second);

        let config = config_of(&roster);
        let mut loaded = Roster::bare();
        apply_config(&config, &mut loaded);

        assert_eq!(loaded, roster, "the mailbox changed on the way through");
        let mailbox = loaded.sheet(Hero::Warrior).mailbox();
        assert_eq!(mailbox.len(), 2);
        assert!(mailbox.get(ids_before.0).is_some_and(|letter| letter.read));
        assert_eq!(mailbox.unread_count(), 1);
        assert!(loaded.sheet(Hero::Druid).mailbox().is_empty());
    }

    #[test]
    fn ids_are_not_reused_after_a_letter_is_taken_and_saved() {
        let ring = starting_item(ItemFamily::Ring).expect("a ring");
        let mut roster = dressed_roster();
        let sheet = roster.sheet_mut(Hero::Druid);
        let id = sheet.mailbox_mut().send(
            ring.id,
            MailSender::DungeonChest,
            MailReason::Unclaimed,
            0.0,
        );
        sheet.take_mail(id).expect("room in the bag");

        let mut loaded = Roster::bare();
        apply_config(&config_of(&roster), &mut loaded);
        let next = loaded.sheet_mut(Hero::Druid).mailbox_mut().send(
            ring.id,
            MailSender::DungeonChest,
            MailReason::Unclaimed,
            0.0,
        );
        assert!(next > id, "id {id} was handed out twice");
    }

    #[test]
    fn every_hero_is_written_including_one_wearing_nothing() {
        let mut roster = dressed_roster();
        *roster.sheet_mut(Hero::Warrior) = CharacterSheet::bare(Hero::Warrior);
        let config = config_of(&roster);

        assert_eq!(config.heroes.len(), Hero::ALL.len());
        let warrior = config.loadout("warrior").expect("the warrior is written");
        assert!(warrior.equipment.is_empty());

        let mut loaded = dressed_roster();
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

        let mut roster = dressed_roster();
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
            ..HeroLoadout::default()
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
            ..HeroLoadout::default()
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
            inventory: vec![InventoryEntry::new(900, "plain_copper_band", 1)],
            ..HeroLoadout::default()
        });

        let mut roster = Roster::bare();
        apply_config(&config, &mut roster);
        assert!(roster.sheet(Hero::FireMage).inventory().is_empty());
    }
}
