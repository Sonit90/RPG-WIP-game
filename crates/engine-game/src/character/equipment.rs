//! What a character is wearing, what they are carrying, and the one rule that moves an item
//! between the two.
//!
//! ```text
//! ItemSlot::Equipment(EquipSlot)  ─┐
//!                                  ├─►  CharacterSheet::move_item(from, to)
//! ItemSlot::Inventory(index)     ─┘            |
//!                                              +--► Ok: the two slots swapped
//!                                              +--► Err(MoveError): nothing moved at all
//! ```
//!
//! # One operation, not four
//!
//! Equipping, unequipping, swapping two rings and rearranging the bag are the same
//! operation: *exchange the contents of two slots, if both ends accept what they would
//! receive*. Written once, as [`Equipment`] and [`Inventory`] behind one addressing type,
//! there is no equip path that can duplicate an item and no unequip path that can lose one —
//! a swap moves what is already there, and the count of items is conserved by construction.
//!
//! The one asymmetry is failure. A move that is refused changes **nothing**: the item stays
//! where it was, which is what lets the interface drag a cloak onto a ring finger, be told
//! no, and put the icon back without holding any state of its own.

use super::item::{EquipSlot, ItemStack};

/// How many cells the inventory has.
///
/// Thirty-two, which is four rows of the eight-across grid the panel draws. A fixed size
/// rather than a growing bag: the grid is part of the layout, and a bag that could grow
/// would be a scrollbar and a page of new interface for no gameplay yet.
pub const INVENTORY_SLOTS: usize = 32;

/// How many cells the inventory grid is wide.
pub const INVENTORY_COLUMNS: usize = 8;

/// Where an item is: a slot on the body, or a cell in the bag.
///
/// The addressing type the whole system is written against. A drag is a pair of these, a
/// save is a list of them, and neither the interface nor the simulation needs a second way
/// to say where something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemSlot {
    /// Worn.
    Equipment(EquipSlot),
    /// Carried, in the cell at this index.
    Inventory(usize),
}

/// Why a move was refused.
///
/// Every variant means the same thing about the world: nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveError {
    /// The slot being dragged from is empty.
    Empty,
    /// The source and the destination are the same slot.
    SameSlot,
    /// An address that does not exist — an inventory cell past the end of the bag.
    NoSuchSlot,
    /// The item cannot be worn where it was dropped.
    ///
    /// Also what a *swap* answers when the item that would come back is not something the
    /// source slot accepts: dragging a ring onto a worn cloak would send the cloak to a
    /// finger, so the whole exchange is refused rather than half-applied.
    Incompatible,
}

impl MoveError {
    /// Uppercase line the interface shows when a drop is refused.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "NOTHING TO MOVE",
            Self::SameSlot => "ALREADY THERE",
            Self::NoSuchSlot => "NO SUCH SLOT",
            Self::Incompatible => "THAT DOES NOT GO THERE",
        }
    }
}

/// The thirteen slots on the body.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Equipment {
    /// One optional stack per [`EquipSlot`], indexed by [`EquipSlot::index`].
    slots: [Option<ItemStack>; EquipSlot::COUNT],
}

impl Equipment {
    /// Nothing worn.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            slots: [None; EquipSlot::COUNT],
        }
    }

    /// What is worn in `slot`.
    #[must_use]
    pub fn get(&self, slot: EquipSlot) -> Option<ItemStack> {
        self.slots[slot.index()]
    }

    /// Puts `stack` in `slot` without asking whether it fits, and answers what was there.
    ///
    /// Private, because "without asking whether it fits" is exactly the door the rest of
    /// this module exists to keep shut. Every public path checks first.
    fn put(&mut self, slot: EquipSlot, stack: Option<ItemStack>) -> Option<ItemStack> {
        core::mem::replace(&mut self.slots[slot.index()], stack)
    }

    /// Wears `stack`, if it fits, and answers what came off.
    ///
    /// # Errors
    ///
    /// [`MoveError::Incompatible`] if the item does not belong in that slot. Nothing is
    /// changed in that case, including the slot's current occupant.
    pub fn equip(
        &mut self,
        slot: EquipSlot,
        stack: ItemStack,
    ) -> Result<Option<ItemStack>, MoveError> {
        if !stack.fits(slot) {
            return Err(MoveError::Incompatible);
        }
        Ok(self.put(slot, Some(stack)))
    }

    /// Takes whatever is in `slot` off.
    pub fn unequip(&mut self, slot: EquipSlot) -> Option<ItemStack> {
        self.put(slot, None)
    }

    /// Every slot with something in it.
    pub fn worn(&self) -> impl Iterator<Item = (EquipSlot, ItemStack)> + '_ {
        EquipSlot::ALL
            .into_iter()
            .filter_map(|slot| self.get(slot).map(|stack| (slot, stack)))
    }

    /// How many slots are filled.
    #[must_use]
    pub fn count(&self) -> usize {
        self.worn().count()
    }
}

/// The bag: a fixed grid of cells, each holding a stack or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    /// One optional stack per cell, in grid order: left to right, then down.
    slots: [Option<ItemStack>; INVENTORY_SLOTS],
}

impl Default for Inventory {
    fn default() -> Self {
        Self::empty()
    }
}

impl Inventory {
    /// An empty bag.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            slots: [None; INVENTORY_SLOTS],
        }
    }

    /// How many cells there are.
    #[must_use]
    pub fn len(&self) -> usize {
        INVENTORY_SLOTS
    }

    /// Whether every cell is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// What is in the cell at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    /// Puts `stack` in the cell at `index` and answers what was there.
    ///
    /// An index past the end of the bag changes nothing and answers `None`, which is what
    /// makes a corrupt save a dropped item rather than a panic.
    pub fn put(&mut self, index: usize, stack: Option<ItemStack>) -> Option<ItemStack> {
        match self.slots.get_mut(index) {
            Some(cell) => core::mem::replace(cell, stack),
            None => None,
        }
    }

    /// Puts `stack` in the first empty cell and answers where it went.
    ///
    /// A stack joins an existing one first when the item allows more than one per cell and the
    /// two are the same item with equal props ([`ItemStack::can_merge`]); a copy with different
    /// quality or enhancement never merges. Equipment stacks to one, so it never does.
    ///
    /// `None` means the bag is full and **nothing was taken**: a caller that unequipped
    /// something to make this call has to put it back.
    pub fn add(&mut self, stack: ItemStack) -> Option<usize> {
        let cap = stack.def().map_or(1, |def| def.max_stack.max(1));
        if let Some(index) = self.slots.iter().position(|cell| {
            cell.is_some_and(|held| {
                held.can_merge(stack)
                    && u32::from(held.quantity) + u32::from(stack.quantity) <= u32::from(cap)
            })
        }) && let Some(held) = self.slots[index].as_mut()
        {
            held.quantity += stack.quantity;
            return Some(index);
        }
        let index = self.slots.iter().position(Option::is_none)?;
        self.slots[index] = Some(stack);
        Some(index)
    }

    /// Empties the cell at `index` and answers what was in it.
    pub fn take(&mut self, index: usize) -> Option<ItemStack> {
        self.put(index, None)
    }

    /// Every cell with something in it, with its index.
    pub fn contents(&self) -> impl Iterator<Item = (usize, ItemStack)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| cell.map(|stack| (index, stack)))
    }

    /// Every cell in grid order, empty ones included. What the panel draws.
    #[must_use]
    pub fn cells(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// How many cells are filled.
    #[must_use]
    pub fn count(&self) -> usize {
        self.contents().count()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::character::item::{ItemFamily, starting_item};

    fn stack(family: ItemFamily) -> ItemStack {
        ItemStack::one(starting_item(family).expect("a white piece").id)
    }

    #[test]
    fn the_bag_is_a_grid_of_whole_rows() {
        assert_eq!(INVENTORY_SLOTS % INVENTORY_COLUMNS, 0);
        assert_eq!(Inventory::empty().len(), INVENTORY_SLOTS);
    }

    #[test]
    fn equipping_answers_what_came_off() {
        let mut equipment = Equipment::empty();
        assert_eq!(equipment.get(EquipSlot::Ring1), None);

        let first = stack(ItemFamily::Ring);
        assert_eq!(equipment.equip(EquipSlot::Ring1, first), Ok(None));
        assert_eq!(equipment.get(EquipSlot::Ring1), Some(first));
        assert_eq!(equipment.count(), 1);

        assert_eq!(equipment.equip(EquipSlot::Ring1, first), Ok(Some(first)));
        assert_eq!(equipment.unequip(EquipSlot::Ring1), Some(first));
        assert_eq!(equipment.count(), 0);
    }

    #[test]
    fn an_item_never_reaches_a_slot_of_another_family() {
        let mut equipment = Equipment::empty();
        let cloak = stack(ItemFamily::Back);
        assert_eq!(
            equipment.equip(EquipSlot::Ring1, cloak),
            Err(MoveError::Incompatible)
        );
        assert_eq!(equipment.get(EquipSlot::Ring1), None);

        // And a refused equip leaves what was already worn alone.
        let ring = stack(ItemFamily::Ring);
        assert_eq!(equipment.equip(EquipSlot::Ring1, ring), Ok(None));
        assert_eq!(
            equipment.equip(EquipSlot::Ring1, cloak),
            Err(MoveError::Incompatible)
        );
        assert_eq!(equipment.get(EquipSlot::Ring1), Some(ring));
    }

    #[test]
    fn a_ring_fits_either_finger_and_a_trinket_either_of_two() {
        let mut equipment = Equipment::empty();
        for slot in ItemFamily::Ring.slots() {
            assert!(equipment.equip(slot, stack(ItemFamily::Ring)).is_ok());
        }
        for slot in ItemFamily::Trinket.slots() {
            assert!(equipment.equip(slot, stack(ItemFamily::Trinket)).is_ok());
        }
        assert_eq!(equipment.count(), 4);
    }

    #[test]
    fn the_bag_fills_from_the_first_empty_cell_and_stops_when_it_is_full() {
        let mut inventory = Inventory::empty();
        assert!(inventory.is_empty());
        for index in 0..INVENTORY_SLOTS {
            assert_eq!(inventory.add(stack(ItemFamily::Ring)), Some(index));
        }
        assert_eq!(inventory.add(stack(ItemFamily::Ring)), None);
        assert_eq!(inventory.count(), INVENTORY_SLOTS);

        inventory.take(4);
        assert_eq!(inventory.add(stack(ItemFamily::Ring)), Some(4));
    }

    #[test]
    fn a_cell_past_the_end_of_the_bag_is_not_a_panic() {
        let mut inventory = Inventory::empty();
        assert_eq!(inventory.get(INVENTORY_SLOTS), None);
        assert_eq!(
            inventory.put(INVENTORY_SLOTS, Some(stack(ItemFamily::Ring))),
            None
        );
        assert!(inventory.is_empty());
    }
}
