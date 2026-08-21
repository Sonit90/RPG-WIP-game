//! The character sheet: what a hero is worth, what they are wearing, and what they carry.
//!
//! ```text
//!  Hero::base_stats()  ─┐
//!  equipment stats     ─┼─►  CharacterSheet::stats()  ─┬─► max health (Stamina)
//!  Spirit buff         ─┘         (recomputed, never    ├─► spell power (main stat)
//!                                  stored)              └─► the character panel
//! ```
//!
//! # Base + gear + modifiers, computed on demand
//!
//! [`CharacterSheet::stats`] adds a hero's base block, every worn item's block and the
//! Spirit buff, every time it is called. **Nothing caches the total.** The rule is the one
//! [`crate::firemage::stats`] already follows for the same reason: a stored total is a total
//! that can be stale, and the moment gear can change mid-fight a stale total is a character
//! whose health bar and whose damage disagree about what they are wearing.
//!
//! The aggregation lives here and only here. No interface component adds up a stat — the
//! panel asks for the finished block and prints it, which is what lets combat, healing,
//! Ultimate generation and the character sheet all be looking at the same number.
//!
//! # Module map
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`stat`] | The seven stats, the block they live in, and health from Stamina |
//! | [`item`] | What an item is, where it fits, and the table of every one |
//! | [`equipment`] | The fifteen worn slots, the bag, and how a slot is addressed |
//! | [`spirit`] | The Spirit proc and its stacking main-stat buff |

/// The fifteen worn slots, the bag, and the one rule that moves an item between them.
pub mod equipment;
/// What an item is, where it can go, and the one table every item is a row of.
pub mod item;
/// Spirit: the chance to bank a point of Ultimate, and the stacking buff it grants.
pub mod spirit;
/// Every stat a character has, as one table rather than as fields.
pub mod stat;

use crate::hero::Hero;

pub use equipment::{
    Equipment, INVENTORY_COLUMNS, INVENTORY_SLOTS, Inventory, ItemSlot, MoveError,
};
pub use item::{
    EquipSlot, ITEMS, ItemDef, ItemFamily, ItemIcon, ItemId, ItemQuality, ItemStack, item,
    item_by_key, starting_item,
};
pub use spirit::{
    SPIRIT_BUFF_DURATION, SPIRIT_MAIN_STAT_PER_STACK, SPIRIT_MAX_STACKS, SpiritProc, SpiritSurge,
};
pub use stat::{HEALTH_PER_STAMINA, RATING_PER_PERCENT, Stat, StatBlock, StatGroup};

/// The main stat every hero starts with, before any gear.
///
/// A hundred, so a point of the main stat reads directly as a point of spell power: the
/// FireMage's [`crate::firemage::BASE_SPELL_POWER`] is a hundred as well, and the bridge
/// between the two is therefore "gear is the difference", with nothing to reconcile at the
/// base. A test in this module holds the two numbers together.
pub const BASE_MAIN_STAT: f32 = 100.0;

/// The Stamina every hero starts with, before any gear.
///
/// Fifty, which at [`stat::HEALTH_PER_STAMINA`] is exactly the five hundred health
/// [`engine_core::tuning::player::MAX_HEALTH`] already tunes the whole enemy roster against.
/// The health number did not move; it simply stopped being a constant and started being a
/// consequence of a stat.
pub const BASE_STAMINA: f32 = 50.0;

/// The slots a fresh character is dressed in.
///
/// Chest, Legs, Wrist and Feet, each with the white zero-stat piece for that family. Every
/// other slot starts bare.
pub const STARTING_SLOTS: [EquipSlot; 4] = [
    EquipSlot::Chest,
    EquipSlot::Legs,
    EquipSlot::Wrist,
    EquipSlot::Feet,
];

/// What a fresh character has in the bag.
///
/// The white pieces for the slots they are *not* wearing, so there is something to drag on
/// the first time the panel is opened, and so every equipment restriction can be exercised
/// by hand. All of them grant nothing, which is what keeps "no starting equipment modifies
/// a secondary stat" true whether a piece is worn or carried.
pub const STARTING_BAG: [ItemFamily; 5] = [
    ItemFamily::Head,
    ItemFamily::Shoulder,
    ItemFamily::Back,
    ItemFamily::Ring,
    ItemFamily::Trinket,
];

/// One character: which hero, what they wear, and what they carry.
///
/// Per hero rather than per player: swapping to the Warrior swaps to the Warrior's gear and
/// the Warrior's bag, because they are different characters rather than one character
/// holding a different weapon. [`Roster`] is what holds all three.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterSheet {
    /// Which hero this sheet is for. Decides the main stat and the base block.
    hero: Hero,
    /// What is worn.
    equipment: Equipment,
    /// What is carried.
    inventory: Inventory,
}

impl CharacterSheet {
    /// A character wearing nothing and carrying nothing.
    #[must_use]
    pub fn bare(hero: Hero) -> Self {
        Self {
            hero,
            equipment: Equipment::empty(),
            inventory: Inventory::empty(),
        }
    }

    /// A fresh character: the four white pieces worn, the rest of the set in the bag.
    #[must_use]
    pub fn starting(hero: Hero) -> Self {
        let mut sheet = Self::bare(hero);
        for slot in STARTING_SLOTS {
            if let Some(def) = item::starting_item(slot.family()) {
                // Cannot fail: the piece was looked up by the slot's own family.
                let _ = sheet.equipment.equip(slot, ItemStack::one(def.id));
            }
        }
        for family in STARTING_BAG {
            if let Some(def) = item::starting_item(family) {
                sheet.inventory.add(ItemStack::one(def.id));
            }
        }
        sheet
    }

    /// Which hero this sheet is for.
    #[must_use]
    pub fn hero(&self) -> Hero {
        self.hero
    }

    /// Which stat is this character's main stat.
    #[must_use]
    pub fn main_stat(&self) -> Stat {
        self.hero.main_stat()
    }

    /// What is worn.
    #[must_use]
    pub fn equipment(&self) -> &Equipment {
        &self.equipment
    }

    /// What is carried.
    #[must_use]
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// What is worn, mutably. For a save being loaded; gameplay goes through [`Self::move_item`].
    pub fn equipment_mut(&mut self) -> &mut Equipment {
        &mut self.equipment
    }

    /// What is carried, mutably. For a save being loaded.
    pub fn inventory_mut(&mut self) -> &mut Inventory {
        &mut self.inventory
    }

    /// The hero's own stats, before anything is worn.
    #[must_use]
    pub fn base_stats(&self) -> StatBlock {
        self.hero.base_stats()
    }

    /// The sum of every worn item's stats. Nothing else contributes.
    #[must_use]
    pub fn gear_stats(&self) -> StatBlock {
        let mut total = StatBlock::zero();
        for (_, stack) in self.equipment.worn() {
            total.add_block(&stack.stats());
        }
        total
    }

    /// **The character's stats, right now.** Base plus gear.
    ///
    /// Derived on every call. See the module documentation for why nothing stores it.
    #[must_use]
    pub fn stats(&self) -> StatBlock {
        let mut stats = self.base_stats();
        stats.add_block(&self.gear_stats());
        stats
    }

    /// The same total with a modifier applied: today, the Spirit buff.
    ///
    /// A separate entry point rather than a field on the sheet, because a buff belongs to
    /// the fight and the sheet outlives fights. Everything that wants the number a
    /// calculation should use asks for this one.
    #[must_use]
    pub fn stats_with(&self, surge: &SpiritSurge) -> StatBlock {
        let mut stats = self.stats();
        surge.apply_to(&mut stats, self.main_stat());
        stats
    }

    /// What this character's maximum health is.
    #[must_use]
    pub fn max_health(&self) -> f32 {
        stat::health_from_stamina(self.stats().get(Stat::Stamina))
    }

    /// What is in `slot`.
    #[must_use]
    pub fn at(&self, slot: ItemSlot) -> Option<ItemStack> {
        match slot {
            ItemSlot::Equipment(worn) => self.equipment.get(worn),
            ItemSlot::Inventory(index) => self.inventory.get(index),
        }
    }

    /// Whether `stack` may be put in `slot`.
    ///
    /// A bag cell takes anything; an equipment slot takes its own family and nothing else.
    #[must_use]
    pub fn accepts(&self, slot: ItemSlot, stack: ItemStack) -> bool {
        match slot {
            ItemSlot::Equipment(worn) => stack.fits(worn),
            ItemSlot::Inventory(index) => index < self.inventory.len(),
        }
    }

    /// **Moves what is in `from` into `to`, exchanging with whatever was there.**
    ///
    /// The one operation. Equipping, unequipping, swapping two rings and rearranging the bag
    /// are all this call with different addresses, which is what makes "an item is never
    /// duplicated and never lost" a property of one function rather than a rule four of them
    /// have to keep.
    ///
    /// # Errors
    ///
    /// Returns a [`MoveError`] if the source is empty, the addresses are the same, an
    /// address does not exist, or either end would receive something it does not accept. In
    /// every case **nothing is changed**: a refused drag leaves the character exactly as it
    /// was, and the interface can put the icon back without having recorded anything.
    pub fn move_item(&mut self, from: ItemSlot, to: ItemSlot) -> Result<(), MoveError> {
        if from == to {
            return Err(MoveError::SameSlot);
        }
        if !self.exists(from) || !self.exists(to) {
            return Err(MoveError::NoSuchSlot);
        }
        let Some(moving) = self.at(from) else {
            return Err(MoveError::Empty);
        };
        if !self.accepts(to, moving) {
            return Err(MoveError::Incompatible);
        }
        // The exchange has to be legal in both directions before either end is touched: a
        // ring dropped on a worn cloak would otherwise send the cloak to a finger.
        let displaced = self.at(to);
        if let Some(displaced) = displaced
            && !self.accepts(from, displaced)
        {
            return Err(MoveError::Incompatible);
        }

        self.put(to, Some(moving));
        self.put(from, displaced);
        Ok(())
    }

    /// Whether `slot` is an address this character has.
    #[must_use]
    fn exists(&self, slot: ItemSlot) -> bool {
        match slot {
            ItemSlot::Equipment(_) => true,
            ItemSlot::Inventory(index) => index < self.inventory.len(),
        }
    }

    /// Writes `stack` into `slot`. Private: every caller has already checked it fits.
    fn put(&mut self, slot: ItemSlot, stack: Option<ItemStack>) {
        match slot {
            ItemSlot::Equipment(worn) => match stack {
                Some(stack) => {
                    let _ = self.equipment.equip(worn, stack);
                }
                None => {
                    self.equipment.unequip(worn);
                }
            },
            ItemSlot::Inventory(index) => {
                self.inventory.put(index, stack);
            }
        }
    }

    /// Takes what is in `slot` off and puts it in the first free bag cell.
    ///
    /// What a right click on a worn item does. Answers where it went; `None` means the bag
    /// was full and **nothing was taken off**.
    pub fn unequip_to_bag(&mut self, slot: EquipSlot) -> Option<usize> {
        let stack = self.equipment.get(slot)?;
        let index = self.inventory.add(stack)?;
        self.equipment.unequip(slot);
        Some(index)
    }
}

/// Every character the player owns: one sheet per hero.
///
/// An array indexed by [`Hero::index`] rather than three fields, so the character panel is
/// written once against "the sheet of the hero the player currently is" and adding a fourth
/// hero adds no branch anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct Roster {
    /// One sheet per hero, in [`Hero::ALL`] order.
    sheets: [CharacterSheet; Hero::ALL.len()],
}

impl Default for Roster {
    fn default() -> Self {
        Self::starting()
    }
}

impl Roster {
    /// Every hero, freshly dressed.
    #[must_use]
    pub fn starting() -> Self {
        Self {
            sheets: Hero::ALL.map(CharacterSheet::starting),
        }
    }

    /// Every hero, wearing and carrying nothing. For a save that is about to be loaded.
    #[must_use]
    pub fn bare() -> Self {
        Self {
            sheets: Hero::ALL.map(CharacterSheet::bare),
        }
    }

    /// The sheet for `hero`.
    #[must_use]
    pub fn sheet(&self, hero: Hero) -> &CharacterSheet {
        &self.sheets[hero.index()]
    }

    /// The sheet for `hero`, mutably.
    pub fn sheet_mut(&mut self, hero: Hero) -> &mut CharacterSheet {
        &mut self.sheets[hero.index()]
    }

    /// Every sheet, in hero order.
    pub fn sheets(&self) -> impl Iterator<Item = &CharacterSheet> {
        self.sheets.iter()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::character::stat::StatGroup;

    #[test]
    fn the_base_stamina_is_the_health_the_game_is_already_tuned_against() {
        assert_eq!(
            stat::health_from_stamina(BASE_STAMINA),
            engine_core::tuning::player::MAX_HEALTH
        );
    }

    #[test]
    fn the_base_main_stat_is_the_base_spell_power() {
        assert_eq!(BASE_MAIN_STAT, crate::firemage::BASE_SPELL_POWER);
    }

    #[test]
    fn each_hero_has_the_main_stat_it_was_specified_with() {
        assert_eq!(Hero::FireMage.main_stat(), Stat::Intellect);
        assert_eq!(Hero::Warrior.main_stat(), Stat::Strength);
        assert_eq!(Hero::Druid.main_stat(), Stat::Intellect);
    }

    #[test]
    fn every_hero_starts_with_a_main_stat_stamina_and_no_secondary_stats() {
        for hero in Hero::ALL {
            let sheet = CharacterSheet::starting(hero);
            let stats = sheet.stats();
            assert_eq!(stats.get(sheet.main_stat()), BASE_MAIN_STAT);
            assert_eq!(stats.get(Stat::Stamina), BASE_STAMINA);
            assert!(stats.get(Stat::Stamina) > 0.0);
            for stat in Stat::ALL {
                if stat.group() == StatGroup::Secondary {
                    assert_eq!(stats.get(stat), 0.0, "{hero:?} starts with {}", stat.name());
                }
            }
        }
    }

    #[test]
    fn stamina_is_what_maximum_health_is() {
        let sheet = CharacterSheet::starting(Hero::FireMage);
        assert_eq!(sheet.max_health(), engine_core::tuning::player::MAX_HEALTH);
    }

    #[test]
    fn a_fresh_character_wears_the_four_white_pieces_and_carries_the_rest() {
        for hero in Hero::ALL {
            let sheet = CharacterSheet::starting(hero);
            for slot in STARTING_SLOTS {
                let stack = sheet
                    .equipment()
                    .get(slot)
                    .unwrap_or_else(|| panic!("{hero:?} is missing its {}", slot.name()));
                let def = stack.def().expect("a real item");
                assert_eq!(def.quality, ItemQuality::Common);
                assert!(def.grants_nothing());
            }
            for slot in EquipSlot::ALL {
                if !STARTING_SLOTS.contains(&slot) {
                    assert_eq!(sheet.equipment().get(slot), None, "{}", slot.name());
                }
            }
            assert_eq!(sheet.inventory().count(), STARTING_BAG.len());
            assert!(
                sheet.gear_stats().is_empty(),
                "starting gear grants nothing"
            );
        }
    }

    #[test]
    fn equipping_stamina_raises_the_maximum_health() {
        // The table has no item with stats, so the effect is asserted through the sheet's
        // own aggregation rather than by inventing one: a block added to the gear total is
        // exactly what a worn item contributes.
        let sheet = CharacterSheet::starting(Hero::Warrior);
        let before = sheet.max_health();
        let mut with_gear = sheet.stats();
        with_gear.add_block(&StatBlock::of(Stat::Stamina, 10.0));
        let after = stat::health_from_stamina(with_gear.get(Stat::Stamina));
        assert!((after - before - 10.0 * HEALTH_PER_STAMINA).abs() < 1e-3);
    }

    #[test]
    fn dragging_from_the_bag_onto_a_slot_equips_it() {
        let mut sheet = CharacterSheet::starting(Hero::Druid);
        let (index, ring) = sheet
            .inventory()
            .contents()
            .find(|(_, stack)| stack.fits(EquipSlot::Ring1))
            .expect("a ring in the bag");

        assert_eq!(
            sheet.move_item(
                ItemSlot::Inventory(index),
                ItemSlot::Equipment(EquipSlot::Ring1)
            ),
            Ok(())
        );
        assert_eq!(sheet.equipment().get(EquipSlot::Ring1), Some(ring));
        assert_eq!(sheet.inventory().get(index), None);
    }

    #[test]
    fn dragging_from_a_slot_into_the_bag_takes_it_off() {
        let mut sheet = CharacterSheet::starting(Hero::FireMage);
        let chest = sheet.equipment().get(EquipSlot::Chest).expect("a chest");
        let free = sheet
            .inventory()
            .cells()
            .iter()
            .position(Option::is_none)
            .expect("a free cell");

        assert_eq!(
            sheet.move_item(
                ItemSlot::Equipment(EquipSlot::Chest),
                ItemSlot::Inventory(free)
            ),
            Ok(())
        );
        assert_eq!(sheet.equipment().get(EquipSlot::Chest), None);
        assert_eq!(sheet.inventory().get(free), Some(chest));
    }

    #[test]
    fn two_compatible_slots_swap_rather_than_duplicating() {
        let mut sheet = CharacterSheet::starting(Hero::FireMage);
        let (index, ring) = sheet
            .inventory()
            .contents()
            .find(|(_, stack)| stack.fits(EquipSlot::Ring1))
            .expect("a ring");
        sheet
            .move_item(
                ItemSlot::Inventory(index),
                ItemSlot::Equipment(EquipSlot::Ring1),
            )
            .expect("the ring goes on");

        assert_eq!(
            sheet.move_item(
                ItemSlot::Equipment(EquipSlot::Ring1),
                ItemSlot::Equipment(EquipSlot::Ring2)
            ),
            Ok(())
        );
        assert_eq!(sheet.equipment().get(EquipSlot::Ring1), None);
        assert_eq!(sheet.equipment().get(EquipSlot::Ring2), Some(ring));
    }

    #[test]
    fn an_incompatible_drop_changes_nothing_at_all() {
        let mut sheet = CharacterSheet::starting(Hero::Warrior);
        let before = sheet.clone();
        let (cloak, _) = sheet
            .inventory()
            .contents()
            .find(|(_, stack)| stack.fits(EquipSlot::Back))
            .expect("a cloak");

        assert_eq!(
            sheet.move_item(
                ItemSlot::Inventory(cloak),
                ItemSlot::Equipment(EquipSlot::Ring1)
            ),
            Err(MoveError::Incompatible)
        );
        assert_eq!(sheet, before, "a refused move changed the character");
    }

    #[test]
    fn a_swap_that_would_send_the_wrong_item_back_is_refused_whole() {
        let mut sheet = CharacterSheet::starting(Hero::Warrior);
        let before = sheet.clone();
        // The bag's ring onto the worn chest: the ring does not fit the chest slot.
        let (ring, _) = sheet
            .inventory()
            .contents()
            .find(|(_, stack)| stack.fits(EquipSlot::Ring1))
            .expect("a ring");
        assert_eq!(
            sheet.move_item(
                ItemSlot::Inventory(ring),
                ItemSlot::Equipment(EquipSlot::Chest)
            ),
            Err(MoveError::Incompatible)
        );
        assert_eq!(sheet, before);
    }

    #[test]
    fn an_empty_source_and_a_slot_onto_itself_are_both_refused() {
        let mut sheet = CharacterSheet::starting(Hero::Druid);
        assert_eq!(
            sheet.move_item(
                ItemSlot::Equipment(EquipSlot::Head),
                ItemSlot::Inventory(INVENTORY_SLOTS - 1)
            ),
            Err(MoveError::Empty)
        );
        assert_eq!(
            sheet.move_item(
                ItemSlot::Equipment(EquipSlot::Chest),
                ItemSlot::Equipment(EquipSlot::Chest)
            ),
            Err(MoveError::SameSlot)
        );
        assert_eq!(
            sheet.move_item(
                ItemSlot::Equipment(EquipSlot::Chest),
                ItemSlot::Inventory(INVENTORY_SLOTS + 4)
            ),
            Err(MoveError::NoSuchSlot)
        );
    }

    #[test]
    fn no_move_ever_changes_how_many_items_the_character_has() {
        let mut sheet = CharacterSheet::starting(Hero::FireMage);
        let total = |sheet: &CharacterSheet| sheet.equipment().count() + sheet.inventory().count();
        let expected = total(&sheet);

        for from in EquipSlot::ALL.map(ItemSlot::Equipment) {
            for to in (0..INVENTORY_SLOTS).map(ItemSlot::Inventory) {
                let _ = sheet.move_item(from, to);
                let _ = sheet.move_item(to, from);
                assert_eq!(total(&sheet), expected, "an item was lost or duplicated");
            }
        }
    }

    #[test]
    fn every_hero_has_its_own_gear_and_its_own_bag() {
        let mut roster = Roster::starting();
        roster
            .sheet_mut(Hero::Warrior)
            .move_item(
                ItemSlot::Equipment(EquipSlot::Chest),
                ItemSlot::Inventory(INVENTORY_SLOTS - 1),
            )
            .expect("the chest comes off");

        assert_eq!(
            roster
                .sheet(Hero::Warrior)
                .equipment()
                .get(EquipSlot::Chest),
            None
        );
        assert!(
            roster
                .sheet(Hero::FireMage)
                .equipment()
                .get(EquipSlot::Chest)
                .is_some(),
            "undressing the Warrior undressed the FireMage"
        );
        assert_eq!(roster.sheets().count(), Hero::ALL.len());
    }

    #[test]
    fn the_spirit_buff_scales_the_main_stat_and_nothing_else() {
        let sheet = CharacterSheet::starting(Hero::FireMage);
        let mut surge = SpiritSurge::new();
        surge.apply();
        surge.apply();

        let stats = sheet.stats_with(&surge);
        assert!((stats.get(Stat::Intellect) - BASE_MAIN_STAT * 1.04).abs() < 1e-3);
        assert_eq!(stats.get(Stat::Stamina), BASE_STAMINA);
    }
}
