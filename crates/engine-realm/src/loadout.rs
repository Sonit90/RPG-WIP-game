//! What a persistent character wears and carries, and the gear score that follows from it.
//!
//! ```text
//! persistence DB ─► Loadout (names) ─► CharacterSheet (engine-game items) ─► gear score, stats
//! ```
//!
//! A [`Loadout`] is the stored form: slot keys and item keys, exactly the vocabulary
//! `character.cfg` already uses, so the offline save and the online save describe a character
//! the same way. [`sheet_of`] is the only way back into gameplay types, and it checks every
//! entry on the way in — a stored row naming a removed item, or an item in a slot it does not
//! fit, becomes an empty slot rather than an illegal character.
//!
//! The gear score is **derived**, never stored and never accepted from a client: the
//! persistence API computes it from the loadout it owns with [`gear_score`], and the realm
//! module reads it from the API. Nothing a client sends can change it.

use engine_game::character::MailItem as SimMail;
use engine_game::character::{
    CharacterSheet, EquipSlot, INVENTORY_SLOTS, Inventory, ItemSlot, ItemStack, MailReason,
    MailSender, MoveError, item_by_key,
};
use engine_game::hero::Hero;
use serde::{Deserialize, Serialize};

use crate::error::RealmError;

/// One worn item, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquippedItem {
    /// The slot's stable key (`"head"`, `"ring1"` …).
    pub slot: String,
    /// The item's stable key.
    pub item: String,
}

/// One carried stack, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BagItem {
    /// The bag cell.
    pub index: u32,
    /// The item's stable key.
    pub item: String,
    /// How many.
    pub quantity: u16,
}

/// One piece of mail: an item waiting to be taken into the bag.
///
/// Mail is how loot that could not be handed over in the dungeon (the bag was full, or the
/// chest was never opened) still reaches the character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailItem {
    /// Stable within the character; never reused.
    pub id: u64,
    /// The attached item's stable key.
    pub item: String,
    /// Who it is from.
    pub sender: String,
    /// The subject line.
    pub subject: String,
    /// Why it was mailed: [`MAIL_BAG_FULL`] or [`MAIL_UNCLAIMED`].
    pub reason: String,
    /// When it arrived, unix seconds.
    pub received_at: i64,
    /// Whether the player has opened it.
    pub read: bool,
}

fn mail_subject(item_name: &str) -> String {
    format!("{item_name} from the dungeon chest")
}

fn removed_slot_subject(item_name: &str) -> String {
    format!("{item_name} from a removed slot")
}

/// Mailed because the bag had no room when the item was claimed.
pub const MAIL_BAG_FULL: &str = "bag_full";
/// Mailed because the chest was never opened.
pub const MAIL_UNCLAIMED: &str = "unclaimed";
/// Mailed because the item was worn in an equipment slot that no longer exists and the bag
/// had no room for it.
pub const MAIL_SLOT_REMOVED: &str = "slot_removed";
/// Who dungeon loot mail is from.
pub const MAIL_SENDER_CHEST: &str = "Dungeon Chest";
/// Who housekeeping mail is from.
pub const MAIL_SENDER_SYSTEM: &str = "System";

/// Everything a character wears and carries, in stored form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loadout {
    /// What is worn.
    pub equipment: Vec<EquippedItem>,
    /// What is carried.
    pub inventory: Vec<BagItem>,
    /// What is waiting in the mailbox. Absent in rows saved before mail existed.
    #[serde(default)]
    pub mail: Vec<MailItem>,
}

/// Where an item is, in stored form: a slot key or a bag cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotRef {
    /// A worn slot, by key.
    Equipment {
        /// The slot's key.
        slot: String,
    },
    /// A bag cell.
    Inventory {
        /// The cell index.
        index: u32,
    },
}

impl SlotRef {
    /// The gameplay address this names, if it names one.
    #[must_use]
    pub fn resolve(&self) -> Option<ItemSlot> {
        match self {
            Self::Equipment { slot } => EquipSlot::from_key(slot).map(ItemSlot::Equipment),
            Self::Inventory { index } => usize::try_from(*index).ok().map(ItemSlot::Inventory),
        }
    }
}

/// What a new character starts with: nothing worn, nothing carried.
#[must_use]
pub fn starting_loadout(hero: Hero) -> Loadout {
    loadout_of(&CharacterSheet::starting(hero))
}

/// The stored form of a character sheet.
#[must_use]
pub fn loadout_of(sheet: &CharacterSheet) -> Loadout {
    let mut loadout = Loadout::default();
    for (slot, stack) in sheet.equipment().worn() {
        if let Some(def) = stack.def() {
            loadout.equipment.push(EquippedItem {
                slot: slot.key().to_owned(),
                item: def.key.to_owned(),
            });
        }
    }
    for (index, stack) in sheet.inventory().contents() {
        if let (Some(def), Ok(index)) = (stack.def(), u32::try_from(index)) {
            loadout.inventory.push(BagItem {
                index,
                item: def.key.to_owned(),
                quantity: stack.quantity,
            });
        }
    }
    for letter in sheet.mailbox().letters() {
        let Some(def) = engine_game::character::item(letter.item) else {
            continue;
        };
        loadout.mail.push(MailItem {
            id: letter.id,
            item: def.key.to_owned(),
            sender: letter.sender.name().to_owned(),
            subject: match letter.reason {
                MailReason::SlotRemoved => removed_slot_subject(def.name),
                _ => mail_subject(def.name),
            },
            reason: match letter.reason {
                MailReason::BagFull => MAIL_BAG_FULL,
                MailReason::Unclaimed => MAIL_UNCLAIMED,
                MailReason::SlotRemoved => MAIL_SLOT_REMOVED,
            }
            .to_owned(),
            received_at: letter.received_at as i64,
            read: letter.read,
        });
    }
    loadout
}

/// A legal character sheet from a stored loadout. Entries that cannot be honoured are
/// dropped and logged, never trusted.
#[must_use]
pub fn sheet_of(hero: Hero, loadout: &Loadout) -> CharacterSheet {
    // Rows saved while there were four trinket slots still name `trinket3` / `trinket4`.
    // Rehome them first so the item reaches the bag (or the mailbox) rather than vanishing.
    let migrated;
    let loadout = if has_removed_slots(loadout) {
        let mut copy = loadout.clone();
        migrate_removed_slots(&mut copy, 0);
        migrated = copy;
        &migrated
    } else {
        loadout
    };
    let mut sheet = CharacterSheet::bare(hero);
    for entry in &loadout.equipment {
        let (Some(slot), Some(def)) = (EquipSlot::from_key(&entry.slot), item_by_key(&entry.item))
        else {
            log::warn!("loadout: dropped {:?} in {:?}", entry.item, entry.slot);
            continue;
        };
        if sheet
            .equipment_mut()
            .equip(slot, ItemStack::one(def.id))
            .is_err()
        {
            log::warn!("loadout: {} does not fit {}", def.key, slot.key());
        }
    }
    let mut inventory = Inventory::empty();
    for entry in &loadout.inventory {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!("loadout: dropped unknown item {:?}", entry.item);
            continue;
        };
        let Ok(index) = usize::try_from(entry.index) else {
            continue;
        };
        if index >= inventory.len() || inventory.get(index).is_some() {
            log::warn!("loadout: bag cell {index} is unusable; dropped {}", def.key);
            continue;
        }
        let quantity = entry.quantity.clamp(1, def.max_stack.max(1));
        inventory.put(index, Some(ItemStack::new(def.id, quantity)));
    }
    *sheet.inventory_mut() = inventory;
    for letter in &loadout.mail {
        let Some(def) = item_by_key(&letter.item) else {
            log::warn!(
                "loadout: dropped mail {} with unknown item {:?}",
                letter.id,
                letter.item
            );
            continue;
        };
        sheet.mailbox_mut().restore(SimMail {
            id: letter.id,
            item: def.id,
            sender: if letter.sender == MAIL_SENDER_SYSTEM {
                MailSender::System
            } else {
                MailSender::DungeonChest
            },
            reason: match letter.reason.as_str() {
                MAIL_BAG_FULL => MailReason::BagFull,
                MAIL_SLOT_REMOVED => MailReason::SlotRemoved,
                _ => MailReason::Unclaimed,
            },
            received_at: letter.received_at as f64,
            read: letter.read,
        });
    }
    sheet
}

/// Gear score: ten times the summed item level of everything worn.
///
/// Summed rather than averaged over the slots, so an empty slot costs what the item in it
/// would have been worth and a character with nothing worn is 0 rather than a fraction. Ten
/// times so a single item level is a visible step in a three-digit number.
#[must_use]
pub fn gear_score_of(sheet: &CharacterSheet) -> u32 {
    let levels: u32 = sheet
        .equipment()
        .worn()
        .filter_map(|(_, stack)| stack.def())
        .map(|def| u32::from(def.item_level))
        .sum();
    levels.saturating_mul(10)
}

/// The gear score of a stored loadout, after the same validation [`sheet_of`] applies — so an
/// illegal row cannot raise it.
#[must_use]
pub fn gear_score(hero: Hero, loadout: &Loadout) -> u32 {
    gear_score_of(&sheet_of(hero, loadout))
}

/// Moves an item, server-side. Returns the new loadout or why nothing changed.
///
/// # Errors
///
/// A persistence error with a player-facing sentence when an address is unknown, empty, the
/// same, or the item does not fit where it was dropped.
pub fn move_item(
    hero: Hero,
    loadout: &Loadout,
    from: &SlotRef,
    to: &SlotRef,
) -> Result<Loadout, RealmError> {
    let (Some(from), Some(to)) = (from.resolve(), to.resolve()) else {
        return Err(RealmError::protocol("That slot does not exist."));
    };
    // Rehome anything in a removed slot first, so the mail it may create is kept below.
    let mut loadout = loadout.clone();
    migrate_removed_slots(&mut loadout, 0);
    let loadout = &loadout;
    let mut sheet = sheet_of(hero, loadout);
    sheet.move_item(from, to).map_err(|error| {
        RealmError::persistence(match error {
            MoveError::Empty => "There is nothing to move.",
            MoveError::SameSlot => "The item is already there.",
            MoveError::NoSuchSlot => "That slot does not exist.",
            MoveError::Incompatible => "That item does not go there.",
        })
    })?;
    let mut moved = loadout_of(&sheet);
    // Moving gear never touches the mailbox.
    moved.mail.clone_from(&loadout.mail);
    Ok(moved)
}

/// Why an item could not be put in the bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BagError {
    /// Every cell is taken.
    Full,
    /// No such item.
    UnknownItem,
}

/// Puts one `key` item in the first free bag cell and returns the cell. Used by the
/// persistence API to deliver a chest drop; the key is validated against the item tables.
///
/// # Errors
///
/// [`BagError::UnknownItem`] for a key no item has; [`BagError::Full`] when all
/// [`INVENTORY_SLOTS`] cells are taken (the loadout is left as it was).
pub fn add_to_bag(loadout: &mut Loadout, key: &str) -> Result<u32, BagError> {
    let def = item_by_key(key).ok_or(BagError::UnknownItem)?;
    let free = (0..INVENTORY_SLOTS)
        .filter_map(|cell| u32::try_from(cell).ok())
        .find(|cell| loadout.inventory.iter().all(|entry| entry.index != *cell))
        .ok_or(BagError::Full)?;
    loadout.inventory.push(BagItem {
        index: free,
        item: def.key.to_owned(),
        quantity: 1,
    });
    loadout.inventory.sort_by_key(|entry| entry.index);
    Ok(free)
}

/// Why mail could not be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeMailError {
    /// There is no such mail.
    NotFound,
    /// The bag has no free cell; the mail stays.
    BagFull,
}

/// Puts a `key` item in the mailbox and returns the mail's id.
///
/// # Errors
///
/// [`BagError::UnknownItem`] for a key no item has.
pub fn add_to_mail(
    loadout: &mut Loadout,
    key: &str,
    reason: &str,
    received_at: i64,
) -> Result<u64, BagError> {
    let def = item_by_key(key).ok_or(BagError::UnknownItem)?;
    Ok(push_mail(
        loadout,
        def.key,
        MAIL_SENDER_CHEST,
        mail_subject(def.name),
        reason,
        received_at,
    ))
}

fn push_mail(
    loadout: &mut Loadout,
    key: &str,
    sender: &str,
    subject: String,
    reason: &str,
    received_at: i64,
) -> u64 {
    let id = loadout
        .mail
        .iter()
        .map(|m| m.id)
        .max()
        .map_or(1, |id| id + 1);
    loadout.mail.push(MailItem {
        id,
        item: key.to_owned(),
        sender: sender.to_owned(),
        subject,
        reason: reason.to_owned(),
        received_at,
        read: false,
    });
    id
}

/// Whether any worn entry names an equipment slot that no longer exists.
#[must_use]
pub fn has_removed_slots(loadout: &Loadout) -> bool {
    loadout
        .equipment
        .iter()
        .any(|entry| EquipSlot::REMOVED_KEYS.contains(&entry.slot.as_str()))
}

/// **Rehomes every item worn in a removed slot** (`trinket3`, `trinket4`): into the first free
/// bag cell, or into the mailbox ([`MAIL_SLOT_REMOVED`], from the System) when the bag is full.
/// Items whose key no item has are dropped, as everywhere else. `received_at` is the unix time
/// stamped on any letter. Returns how many items moved.
pub fn migrate_removed_slots(loadout: &mut Loadout, received_at: i64) -> usize {
    let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut loadout.equipment)
        .into_iter()
        .partition(|entry| EquipSlot::REMOVED_KEYS.contains(&entry.slot.as_str()));
    loadout.equipment = kept;
    let mut moved = 0;
    for entry in removed {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!(
                "loadout: dropped unknown item {:?} in {:?}",
                entry.item,
                entry.slot
            );
            continue;
        };
        if add_to_bag(loadout, def.key).is_err() {
            push_mail(
                loadout,
                def.key,
                MAIL_SENDER_SYSTEM,
                removed_slot_subject(def.name),
                MAIL_SLOT_REMOVED,
                received_at,
            );
        }
        moved += 1;
    }
    moved
}

/// Moves mail `mail_id`'s attachment into the first free bag cell and deletes the mail.
/// Returns the cell.
///
/// # Errors
///
/// [`TakeMailError::NotFound`] or [`TakeMailError::BagFull`]; the loadout is unchanged.
pub fn take_mail(loadout: &mut Loadout, mail_id: u64) -> Result<u32, TakeMailError> {
    let index = loadout
        .mail
        .iter()
        .position(|m| m.id == mail_id)
        .ok_or(TakeMailError::NotFound)?;
    let key = loadout.mail[index].item.clone();
    match add_to_bag(loadout, &key) {
        Ok(cell) => {
            loadout.mail.remove(index);
            Ok(cell)
        }
        Err(BagError::Full) => Err(TakeMailError::BagFull),
        // An item that no longer exists cannot be taken; the mail is dropped, not stuck.
        Err(BagError::UnknownItem) => {
            loadout.mail.remove(index);
            Err(TakeMailError::NotFound)
        }
    }
}

/// Marks mail read. `false` if there is no such mail.
pub fn mark_mail_read(loadout: &mut Loadout, mail_id: u64) -> bool {
    loadout
        .mail
        .iter_mut()
        .find(|m| m.id == mail_id)
        .map(|m| m.read = true)
        .is_some()
}

/// A character's derived stats as `(key, value)` pairs, for a snapshot or a tooltip.
#[must_use]
pub fn stats_of(sheet: &CharacterSheet) -> Vec<(String, f32)> {
    let stats = sheet.stats();
    engine_game::character::Stat::ALL
        .into_iter()
        .map(|stat| (stat.key().to_owned(), stats.get(stat)))
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_game::character::{ItemFamily, starting_item};

    /// A loadout with the white kit new heroes used to start with: four pieces worn.
    fn kit_loadout() -> Loadout {
        let mut loadout = Loadout::default();
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let def = starting_item(slot.family()).expect("a white piece");
            loadout.equipment.push(EquippedItem {
                slot: slot.key().to_owned(),
                item: def.key.to_owned(),
            });
        }
        loadout
    }

    #[test]
    fn starting_loadout_is_empty_and_round_trips() {
        for hero in Hero::ALL {
            let loadout = starting_loadout(hero);
            assert_eq!(loadout, Loadout::default());
            assert_eq!(loadout_of(&sheet_of(hero, &loadout)), loadout);
        }
    }

    #[test]
    fn starting_gear_score_is_zero() {
        for hero in Hero::ALL {
            assert_eq!(gear_score(hero, &starting_loadout(hero)), 0);
        }
        assert_eq!(gear_score(Hero::Warrior, &kit_loadout()), 40);
    }

    fn trinket_key() -> &'static str {
        starting_item(ItemFamily::Trinket).expect("a trinket").key
    }

    fn old_loadout_with_four_trinkets() -> Loadout {
        let mut loadout = kit_loadout();
        for slot in ["trinket1", "trinket2", "trinket3", "trinket4"] {
            loadout.equipment.push(EquippedItem {
                slot: slot.into(),
                item: trinket_key().into(),
            });
        }
        loadout
    }

    #[test]
    fn items_in_removed_slots_move_to_the_first_free_bag_cells() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        old.inventory.push(BagItem {
            index: 0,
            item: trinket_key().into(),
            quantity: 1,
        });
        let sheet = sheet_of(hero, &old);
        assert!(sheet.equipment().get(EquipSlot::Trinket1).is_some());
        assert!(sheet.equipment().get(EquipSlot::Trinket2).is_some());
        // Cell 0 was taken, so the two displaced trinkets land in 1 and 2.
        assert_eq!(sheet.inventory().count(), 3);
        assert!(sheet.inventory().get(1).is_some() && sheet.inventory().get(2).is_some());
        assert!(sheet.mailbox().is_empty());
        // And the stored form no longer names the removed slots.
        let stored = loadout_of(&sheet);
        assert!(!has_removed_slots(&stored));
    }

    #[test]
    fn items_in_removed_slots_are_mailed_when_the_bag_is_full() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        let key = engine_game::character::loot_pool()[0].key;
        while add_to_bag(&mut old, key).is_ok() {}
        let sheet = sheet_of(hero, &old);
        assert_eq!(sheet.inventory().count(), INVENTORY_SLOTS);
        assert_eq!(sheet.mailbox().len(), 2);
        for letter in sheet.mailbox().letters() {
            assert_eq!(letter.reason, MailReason::SlotRemoved);
            assert_eq!(letter.sender, MailSender::System);
        }
        let stored = loadout_of(&sheet);
        assert_eq!(stored.mail.len(), 2);
        assert_eq!(stored.mail[0].reason, MAIL_SLOT_REMOVED);
        // Round trips through the stored form.
        assert_eq!(sheet_of(hero, &stored).mailbox().len(), 2);
    }

    #[test]
    fn migrating_a_stored_loadout_keeps_the_mail_across_a_move() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        let key = engine_game::character::loot_pool()[0].key;
        while add_to_bag(&mut old, key).is_ok() {}
        let from = SlotRef::Inventory { index: 0 };
        let to = SlotRef::Inventory { index: 1 };
        let moved = move_item(hero, &old, &from, &to).expect("two full cells swap");
        assert!(!has_removed_slots(&moved));
        assert_eq!(moved.mail.len(), 2, "the rehomed items were lost");
    }

    #[test]
    fn migrating_in_place_reports_how_many_items_moved() {
        let mut old = old_loadout_with_four_trinkets();
        assert!(has_removed_slots(&old));
        assert_eq!(migrate_removed_slots(&mut old, 77), 2);
        assert!(!has_removed_slots(&old));
        assert_eq!(old.inventory.len(), 2);
        assert_eq!(migrate_removed_slots(&mut old, 77), 0);
    }

    #[test]
    fn forged_rows_cannot_raise_the_gear_score() {
        let hero = Hero::Warrior;
        let mut loadout = kit_loadout();
        let honest = gear_score(hero, &loadout);
        // An item that does not exist, and a real item in a slot it does not fit.
        loadout.equipment.push(EquippedItem {
            slot: "trinket1".into(),
            item: "legendary_sword_of_lies".into(),
        });
        let chest = loadout
            .equipment
            .iter()
            .find(|entry| entry.slot == "chest")
            .map(|entry| entry.item.clone())
            .expect("the starting set has a chest");
        loadout.equipment.push(EquippedItem {
            slot: "ring1".into(),
            item: chest,
        });
        assert_eq!(gear_score(hero, &loadout), honest);
    }

    #[test]
    fn loot_goes_to_the_first_free_cell_until_the_bag_is_full() {
        let key = engine_game::character::loot_pool()[0].key;
        let mut loadout = Loadout::default();
        loadout.inventory.push(BagItem {
            index: 1,
            item: key.into(),
            quantity: 1,
        });
        // Cell 0 is free, cell 1 is taken.
        assert_eq!(add_to_bag(&mut loadout, key), Ok(0));
        assert_eq!(add_to_bag(&mut loadout, key), Ok(2));
        assert_eq!(
            add_to_bag(&mut loadout, "no_such_item"),
            Err(BagError::UnknownItem)
        );
        while add_to_bag(&mut loadout, key).is_ok() {}
        assert_eq!(add_to_bag(&mut loadout, key), Err(BagError::Full));
        assert_eq!(loadout.inventory.len(), INVENTORY_SLOTS);
        // The legal sheet keeps every one of them.
        let sheet = sheet_of(Hero::Warrior, &loadout);
        assert_eq!(sheet.inventory().count(), INVENTORY_SLOTS);
    }

    #[test]
    fn mail_is_taken_into_the_first_free_cell_and_waits_when_the_bag_is_full() {
        let key = engine_game::character::loot_pool()[0].key;
        let mut loadout = Loadout::default();
        let first = add_to_mail(&mut loadout, key, MAIL_BAG_FULL, 100).expect("a real item");
        let second = add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 101).expect("a real item");
        assert_ne!(first, second);
        assert_eq!(
            add_to_mail(&mut loadout, "nope", MAIL_UNCLAIMED, 0),
            Err(BagError::UnknownItem)
        );
        assert!(mark_mail_read(&mut loadout, first));
        assert!(!mark_mail_read(&mut loadout, 99));
        assert_eq!(take_mail(&mut loadout, first), Ok(0));
        assert_eq!(take_mail(&mut loadout, first), Err(TakeMailError::NotFound));
        while add_to_bag(&mut loadout, key).is_ok() {}
        assert_eq!(take_mail(&mut loadout, second), Err(TakeMailError::BagFull));
        assert_eq!(loadout.mail.len(), 1, "full bag leaves the mail");
        // Old rows without a mailbox still load.
        let old: Loadout =
            serde_json::from_str(r#"{"equipment":[],"inventory":[]}"#).expect("loads");
        assert!(old.mail.is_empty());
    }

    #[test]
    fn mail_survives_the_trip_through_a_sheet() {
        let hero = Hero::Warrior;
        let mut loadout = starting_loadout(hero);
        let key = engine_game::character::loot_pool()[0].key;
        let first = add_to_mail(&mut loadout, key, MAIL_BAG_FULL, 7).expect("a real item");
        let second = add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 8).expect("a real item");
        let mut sheet = sheet_of(hero, &loadout);
        assert_eq!(sheet.mailbox().len(), 2);
        let back = loadout_of(&sheet);
        assert_eq!(back.mail.len(), 2);
        assert_eq!(back.mail[0].id, first);
        assert_eq!(back.mail[1].reason, MAIL_UNCLAIMED);
        // New letters never reuse an id.
        let fresh = sheet.mailbox_mut().send(
            engine_game::character::item_by_key(key).expect("real").id,
            MailSender::DungeonChest,
            MailReason::BagFull,
            9.0,
        );
        assert!(fresh > second);
    }

    #[test]
    fn moving_gear_keeps_the_mail() {
        let hero = Hero::Warrior;
        let mut loadout = kit_loadout();
        let key = engine_game::character::loot_pool()[0].key;
        add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 5).expect("a real item");
        let from = SlotRef::Equipment {
            slot: loadout.equipment[0].slot.clone(),
        };
        let moved = move_item(hero, &loadout, &from, &SlotRef::Inventory { index: 31 })
            .expect("fits the bag");
        assert_eq!(moved.mail, loadout.mail);
    }

    #[test]
    fn moves_are_validated() {
        let hero = Hero::FireMage;
        let loadout = kit_loadout();
        let head = SlotRef::Equipment {
            slot: "head".into(),
        };
        let cell = SlotRef::Inventory { index: 31 };
        // Something worn moves to an empty bag cell, and back.
        let worn = loadout.equipment[0].slot.clone();
        let from = SlotRef::Equipment { slot: worn };
        let moved = move_item(hero, &loadout, &from, &cell).expect("a worn item fits the bag");
        assert_eq!(moved.equipment.len(), loadout.equipment.len() - 1);
        // Nothing to move from an empty cell; nonsense addresses are refused.
        assert!(move_item(hero, &loadout, &SlotRef::Inventory { index: 30 }, &head).is_err());
        assert!(
            move_item(
                hero,
                &loadout,
                &SlotRef::Equipment {
                    slot: "tail".into()
                },
                &cell
            )
            .is_err()
        );
        assert!(move_item(hero, &loadout, &SlotRef::Inventory { index: 999 }, &cell).is_err());
    }
}
