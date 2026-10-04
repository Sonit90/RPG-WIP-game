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

use engine_game::character::{
    CharacterSheet, EquipSlot, Inventory, ItemSlot, ItemStack, MoveError, item_by_key,
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

/// Everything a character wears and carries, in stored form.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loadout {
    /// What is worn.
    pub equipment: Vec<EquippedItem>,
    /// What is carried.
    pub inventory: Vec<BagItem>,
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

/// What a new character starts with: the hero's starting set.
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
    loadout
}

/// A legal character sheet from a stored loadout. Entries that cannot be honoured are
/// dropped and logged, never trusted.
#[must_use]
pub fn sheet_of(hero: Hero, loadout: &Loadout) -> CharacterSheet {
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
    sheet
}

/// Gear score: ten times the summed item level of everything worn.
///
/// Summed rather than averaged over fifteen slots, so an empty slot costs what the item in it
/// would have been worth and a starting character (four level-1 pieces) is 40 rather than a
/// fraction. Ten times so a single item level is a visible step in a three-digit number.
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
    let mut sheet = sheet_of(hero, loadout);
    sheet.move_item(from, to).map_err(|error| {
        RealmError::persistence(match error {
            MoveError::Empty => "There is nothing to move.",
            MoveError::SameSlot => "The item is already there.",
            MoveError::NoSuchSlot => "That slot does not exist.",
            MoveError::Incompatible => "That item does not go there.",
        })
    })?;
    Ok(loadout_of(&sheet))
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

    #[test]
    fn starting_loadout_round_trips() {
        for hero in Hero::ALL {
            let loadout = starting_loadout(hero);
            assert!(!loadout.equipment.is_empty());
            assert_eq!(loadout_of(&sheet_of(hero, &loadout)), loadout);
        }
    }

    #[test]
    fn starting_gear_score_is_forty() {
        // Four white pieces at item level 1.
        for hero in Hero::ALL {
            assert_eq!(gear_score(hero, &starting_loadout(hero)), 40);
        }
    }

    #[test]
    fn forged_rows_cannot_raise_the_gear_score() {
        let hero = Hero::Warrior;
        let mut loadout = starting_loadout(hero);
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
    fn moves_are_validated() {
        let hero = Hero::FireMage;
        let loadout = starting_loadout(hero);
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
