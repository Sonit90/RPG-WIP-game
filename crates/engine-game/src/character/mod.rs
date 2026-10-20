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
//! | [`stat`] | The stats, the block they live in, rating conversion and health from Stamina |
//! | [`item`] | What an item is, where it fits, and the table of every one |
//! | [`equipment`] | The thirteen worn slots, the bag, and how a slot is addressed |
//! | [`spirit`] | The Spirit proc and its stacking main-stat buff |
//! | [`mail`] | The mailbox: items sent to a character instead of lost |

/// What an incoming attack meets: Dodge, Parry, Block and Armour, rolled once.
pub mod defense;
/// The thirteen worn slots, the bag, and the one rule that moves an item between them.
pub mod equipment;
/// What an item is, where it can go, and the one table every item is a row of.
pub mod item;
/// The mailbox: items that could not be handed over, kept for the player to collect.
pub mod mail;
/// Spirit: the chance to bank a point of Ultimate, and the stacking buff it grants.
pub mod spirit;
/// Every stat a character has, as one table rather than as fields.
pub mod stat;

use crate::hero::Hero;

pub use defense::{AVOIDANCE_CAP, BLOCK_REDUCTION, DefenseOutcome, DefenseProfile};
pub use equipment::{
    Equipment, INVENTORY_COLUMNS, INVENTORY_SLOTS, Inventory, ItemSlot, MoveError,
};
pub use item::{
    BROKEN_PENALTY, CATALOG_ID_BASE, CatalogItem, EquipSlot, ITEMS, ItemDef, ItemFamily, ItemIcon,
    ItemId, ItemProps, ItemQuality, ItemStack, QUALITY_DEFAULT, QUALITY_MAX, QUALITY_MIN,
    SHARPEN_HIGHLIGHT_DEFAULT, SHARPEN_STEP_PCT, SHARPENING_CEILING, catalog_items, item,
    item_by_key, loot_pool, register_catalog_items, set_sharpen_highlight_from,
    sharpen_highlight_from, starting_item,
};
pub use mail::{MAIL_SOFT_CAP, MailItem, MailReason, MailSender, Mailbox, TakeMailError};
pub use spirit::{
    SPIRIT_BUFF_DURATION, SPIRIT_MAIN_STAT_PER_STACK, SPIRIT_MAX_STACKS, SpiritProc, SpiritSurge,
};
pub use stat::{
    ARMOR_HALVING_POINT, ARMOR_MAX_REDUCTION, HEALTH_PER_STAMINA, RATING_PER_PERCENT, Stat,
    StatBlock, StatGroup, armor_reduction, secondary_fraction,
};

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

/// The armour the Warrior wears before any gear at all.
///
/// The one place a hero's base block is *not* the same as every other hero's, and it is
/// deliberate: a tank with no armour is a cloth caster with a shield, and War Stomp's
/// armour bonus — a percentage *of* armour — would be a percentage of nothing. Five hundred, which
/// through [`stat::armor_reduction`] is a third off a physical hit before a single item is
/// equipped.
pub const WARRIOR_BASE_ARMOR: f32 = 500.0;

/// The Block, Parry and Dodge the Warrior has before any gear, as ratings.
///
/// Three per cent each, which is nine per cent of attacks answered by a defensive event at
/// base gear. It has to be non-zero for the same reason the armour does: Battle Trance and
/// Spell Reflect are fed by defensive events, and a tank whose defences cannot fire without gear
/// cannot be played in the gear it starts in.
pub const WARRIOR_BASE_AVOIDANCE_RATING: f32 = 3.0 * RATING_PER_PERCENT;

/// The slots a fresh character is dressed in: none. A new hero has no items at all.
pub const STARTING_SLOTS: [EquipSlot; 0] = [];

/// What a fresh character has in the bag: nothing.
pub const STARTING_BAG: [ItemFamily; 0] = [];

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
    /// Items sent to this character instead of being lost.
    mailbox: Mailbox,
}

impl CharacterSheet {
    /// A character wearing nothing and carrying nothing.
    #[must_use]
    pub fn bare(hero: Hero) -> Self {
        Self {
            hero,
            equipment: Equipment::empty(),
            inventory: Inventory::empty(),
            mailbox: Mailbox::new(),
        }
    }

    /// A fresh character: no items worn and none carried (see [`STARTING_SLOTS`]).
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

    /// **Finds a home for an item whose equipment slot no longer exists.**
    ///
    /// The first free bag cell if there is one (answers its index); otherwise a letter in the
    /// mailbox ([`MailReason::SlotRemoved`]) and `None`. Either way the item is kept.
    pub fn rehome_removed_slot_item(
        &mut self,
        stack: ItemStack,
        received_at: f64,
    ) -> Option<usize> {
        if let Some(index) = self.inventory.add(stack) {
            return Some(index);
        }
        self.mailbox.send_stack(
            stack,
            MailSender::System,
            MailReason::SlotRemoved,
            received_at,
        );
        None
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

    /// This character's letters.
    #[must_use]
    pub fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }

    /// This character's letters, mutably. For delivering mail and for a save being loaded.
    pub fn mailbox_mut(&mut self) -> &mut Mailbox {
        &mut self.mailbox
    }

    /// How many letters have not been read.
    #[must_use]
    pub fn unread_mail_count(&self) -> usize {
        self.mailbox.unread_count()
    }

    /// Marks a letter read. Returns whether it exists.
    pub fn mark_read(&mut self, id: u64) -> bool {
        self.mailbox.mark_read(id)
    }

    /// **Moves a letter's attachment into the first free bag cell and removes the letter.**
    ///
    /// Answers the cell it landed in.
    ///
    /// # Errors
    ///
    /// [`TakeMailError::NotFound`] for an unknown id, [`TakeMailError::BagFull`] when there is
    /// no free cell. In both cases **nothing is changed**: a full bag keeps the letter.
    pub fn take_mail(&mut self, id: u64) -> Result<ItemSlot, TakeMailError> {
        self.mailbox.take_into(id, &mut self.inventory)
    }

    /// The hero's own stats, before anything is worn.
    #[must_use]
    pub fn base_stats(&self) -> StatBlock {
        self.hero.base_stats()
    }

    /// The sum of every worn item's stats. Nothing else contributes.
    ///
    /// **Where [`Stat::MainStat`] is resolved.** Items are hero-agnostic, so a piece says
    /// "main stat" and this sheet - the one place that knows its [`Hero`] - turns it into
    /// Intellect or Strength. The placeholder is zeroed in the result, so nothing downstream
    /// (the panel, the combat bridge, the Spirit buff) ever sees it, and a sheet cannot grow a
    /// "Main stat" row. Folding here rather than in `stats()` also keeps `gear_stats()` a
    /// faithful "what gear gives me" for the tooltip comparison.
    #[must_use]
    pub fn gear_stats(&self) -> StatBlock {
        let mut total = StatBlock::zero();
        for (_, stack) in self.equipment.worn() {
            total.add_block(&stack.stats());
        }
        let main = total.get(Stat::MainStat);
        if main != 0.0 {
            total.add(self.main_stat(), main);
            total.set(Stat::MainStat, 0.0);
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
    /// Every hero as a new character: no items at all.
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

    /// The white kit new heroes used to start with: four pieces worn, five in the bag.
    fn kit(hero: Hero) -> CharacterSheet {
        let mut sheet = CharacterSheet::bare(hero);
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let def = item::starting_item(slot.family()).expect("a white piece");
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
            let def = item::starting_item(family).expect("a white piece");
            sheet.inventory_mut().add(ItemStack::one(def.id));
        }
        sheet
    }

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
    fn a_fresh_character_has_no_items_at_all() {
        for hero in Hero::ALL {
            let sheet = CharacterSheet::starting(hero);
            assert_eq!(sheet.equipment().count(), 0);
            assert_eq!(sheet.inventory().count(), 0);
            assert!(sheet.mailbox().is_empty());
            assert!(sheet.gear_stats().is_empty());
        }
        let roster = Roster::starting();
        for sheet in roster.sheets() {
            assert_eq!(sheet.equipment().count() + sheet.inventory().count(), 0);
        }
    }

    #[test]
    fn an_item_from_a_removed_slot_goes_to_the_first_free_bag_cell() {
        let mut sheet = CharacterSheet::bare(Hero::Warrior);
        let def = item::starting_item(ItemFamily::Trinket).expect("a trinket");
        sheet.inventory_mut().put(0, Some(ItemStack::one(def.id)));
        let home = sheet.rehome_removed_slot_item(ItemStack::one(def.id), 5.0);
        assert_eq!(home, Some(1));
        assert_eq!(sheet.inventory().get(1), Some(ItemStack::one(def.id)));
        assert!(sheet.mailbox().is_empty());
    }

    #[test]
    fn an_item_from_a_removed_slot_is_mailed_when_the_bag_is_full() {
        let mut sheet = CharacterSheet::bare(Hero::Warrior);
        let def = item::starting_item(ItemFamily::Trinket).expect("a trinket");
        while sheet.inventory_mut().add(ItemStack::one(def.id)).is_some() {}
        assert_eq!(
            sheet.rehome_removed_slot_item(ItemStack::one(def.id), 5.0),
            None
        );
        let letters = sheet.mailbox().letters();
        assert_eq!(letters.len(), 1);
        assert_eq!(letters[0].item, def.id);
        assert_eq!(letters[0].reason, MailReason::SlotRemoved);
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
    fn worn_main_stat_becomes_the_wearers_own_main_stat() {
        let helm = item_by_key("head_haste_main_stat").expect("a main stat helm");
        for hero in Hero::ALL {
            let mut sheet = CharacterSheet::starting(hero);
            sheet
                .equipment_mut()
                .equip(EquipSlot::Head, ItemStack::one(helm.id))
                .expect("fits");
            let stats = sheet.stats();
            assert_eq!(
                stats.get(hero.main_stat()),
                BASE_MAIN_STAT + 2.0,
                "{hero:?}"
            );
            assert_eq!(stats.get(Stat::Haste), 2.0);
            assert_eq!(stats.get(Stat::MainStat), 0.0, "the placeholder leaked");
            let other = if hero.main_stat() == Stat::Strength {
                Stat::Intellect
            } else {
                Stat::Strength
            };
            assert_eq!(stats.get(other), 0.0, "{hero:?} gained the wrong stat");
        }
    }

    #[test]
    fn dragging_from_the_bag_onto_a_slot_equips_it() {
        let mut sheet = kit(Hero::Druid);
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
        let mut sheet = kit(Hero::FireMage);
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
        let mut sheet = kit(Hero::FireMage);
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
        let mut sheet = kit(Hero::Warrior);
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
        let mut sheet = kit(Hero::Warrior);
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
        let mut sheet = kit(Hero::Druid);
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
        let mut sheet = kit(Hero::FireMage);
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
        let mut roster = Roster::bare();
        for hero in Hero::ALL {
            *roster.sheet_mut(hero) = kit(hero);
        }
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
