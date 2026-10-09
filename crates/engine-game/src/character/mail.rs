//! The mailbox: where loot goes when it has nowhere else to go.
//!
//! ```text
//! chest claim, bag full       ──► Mailbox::send(BagFull)    ──┐
//! chest claim, never opened   ──► Mailbox::send(Unclaimed)  ──┼─► letters, oldest first
//!                                                             │
//! CharacterSheet::take_mail(id) ◄─────────────────────────────┘
//!      attachment ──► first free bag cell, letter removed (or refused, letter kept)
//! ```
//!
//! # Mail belongs to a character
//!
//! One [`Mailbox`] per [`super::CharacterSheet`], so the Warrior's mail is the Warrior's. Online a
//! character is exactly one hero, which makes this the same thing as "the player's mail".
//!
//! # Nothing is ever lost
//!
//! A mailbox has no hard cap. [`MAIL_SOFT_CAP`] is the size past which sending logs a warning;
//! the letter is stored regardless, because the whole point of mail is that an item which could
//! not be kept elsewhere is kept here. A refused letter would be a deleted item.
//!
//! # Time
//!
//! [`MailItem::received_at`] is seconds on whatever clock the sender stamps with. The
//! simulation stamps `epoch + simulation time`, where the epoch is supplied by the host
//! (`Simulation::set_mail_epoch`, unix seconds in the offline game, zero by default), which
//! keeps a replay deterministic and still lets a real date be shown.

use super::{Inventory, ItemId, ItemSlot, ItemStack};

/// The number of letters past which sending logs a warning. Not a limit: see the module docs.
pub const MAIL_SOFT_CAP: usize = 200;

/// Who a letter is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailSender {
    /// The boss's chest at the end of a dungeon.
    DungeonChest,
    /// The game itself, for an item that lost its place (a slot that no longer exists).
    System,
}

impl MailSender {
    /// The name the interface prints.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::DungeonChest => "Dungeon Chest",
            Self::System => "System",
        }
    }
}

/// Why an item was mailed rather than handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MailReason {
    /// The bag was full when the player tried to take it.
    BagFull,
    /// The player never took it from the chest.
    Unclaimed,
    /// It was worn in an equipment slot that has since been removed, and the bag was full.
    SlotRemoved,
}

impl MailReason {
    /// The line the interface prints under the item.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::BagFull => "Your bag was full.",
            Self::Unclaimed => "You did not take this from the chest.",
            Self::SlotRemoved => "Its equipment slot no longer exists.",
        }
    }
}

/// One letter: an item and where it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MailItem {
    /// Unique within its mailbox; never reused.
    pub id: u64,
    /// The attachment.
    pub item: ItemId,
    /// Who sent it.
    pub sender: MailSender,
    /// Why it was sent.
    pub reason: MailReason,
    /// When it arrived, in seconds. See the module docs for the clock.
    pub received_at: f64,
    /// Whether the player has opened it.
    pub read: bool,
}

/// Why [`super::CharacterSheet::take_mail`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeMailError {
    /// No letter has that id.
    NotFound,
    /// The bag has no free cell. The letter is kept.
    BagFull,
}

/// A character's letters, oldest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mailbox {
    letters: Vec<MailItem>,
    /// The id the next letter gets. Monotonic, so an id is never reused after a take.
    next_id: u64,
}

impl Mailbox {
    /// An empty mailbox.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every letter, oldest first.
    #[must_use]
    pub fn letters(&self) -> &[MailItem] {
        &self.letters
    }

    /// The letter with `id`.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&MailItem> {
        self.letters.iter().find(|letter| letter.id == id)
    }

    /// How many letters there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.letters.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.letters.is_empty()
    }

    /// How many letters have not been read.
    #[must_use]
    pub fn unread_count(&self) -> usize {
        self.letters.iter().filter(|letter| !letter.read).count()
    }

    /// Delivers a new, unread letter and returns its id.
    ///
    /// Never refuses; past [`MAIL_SOFT_CAP`] it only warns.
    pub fn send(
        &mut self,
        item: ItemId,
        sender: MailSender,
        reason: MailReason,
        received_at: f64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.letters.push(MailItem {
            id,
            item,
            sender,
            reason,
            received_at,
            read: false,
        });
        if self.letters.len() > MAIL_SOFT_CAP {
            log::warn!(
                "mail: {} letters in one mailbox (soft cap {MAIL_SOFT_CAP}); keeping them all",
                self.letters.len()
            );
        }
        id
    }

    /// Marks a letter read. Returns whether it exists.
    pub fn mark_read(&mut self, id: u64) -> bool {
        match self.letters.iter_mut().find(|letter| letter.id == id) {
            Some(letter) => {
                letter.read = true;
                true
            }
            None => false,
        }
    }

    /// Restores a letter from a save, keeping its id.
    ///
    /// Advances the id counter past it, so a later [`Mailbox::send`] cannot collide. A letter
    /// whose id is already present is given a fresh one instead, so a hand-edited file cannot
    /// make two letters share an id.
    pub fn restore(&mut self, mut letter: MailItem) {
        if self.get(letter.id).is_some() {
            letter.id = self.next_id;
        }
        self.next_id = self.next_id.max(letter.id + 1);
        self.letters.push(letter);
    }

    /// The id the next letter will get. For a save.
    #[must_use]
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Raises the id counter to at least `next_id`. For a save, so ids stay unique across
    /// launches even after the newest letters were taken.
    pub fn reserve_ids_up_to(&mut self, next_id: u64) {
        self.next_id = self.next_id.max(next_id);
    }

    /// Removes a letter and returns it. Private to the character: taking goes through
    /// [`super::CharacterSheet::take_mail`], which owns the bag.
    pub(super) fn remove(&mut self, id: u64) -> Option<MailItem> {
        let index = self.letters.iter().position(|letter| letter.id == id)?;
        Some(self.letters.remove(index))
    }

    /// Moves the attachment of `id` into the first free cell of `bag`.
    pub(super) fn take_into(
        &mut self,
        id: u64,
        bag: &mut Inventory,
    ) -> Result<ItemSlot, TakeMailError> {
        let letter = *self.get(id).ok_or(TakeMailError::NotFound)?;
        let cell = bag
            .add(ItemStack::one(letter.item))
            .ok_or(TakeMailError::BagFull)?;
        self.remove(id);
        Ok(ItemSlot::Inventory(cell))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::character::{CharacterSheet, INVENTORY_SLOTS, loot_pool};
    use crate::hero::Hero;

    fn send(sheet: &mut CharacterSheet) -> u64 {
        sheet.mailbox_mut().send(
            loot_pool()[0].id,
            MailSender::DungeonChest,
            MailReason::Unclaimed,
            1.0,
        )
    }

    #[test]
    fn ids_are_unique_and_never_reused() {
        let mut sheet = CharacterSheet::starting(Hero::Warrior);
        let a = send(&mut sheet);
        let b = send(&mut sheet);
        assert_ne!(a, b);
        sheet.take_mail(b).expect("room");
        let c = send(&mut sheet);
        assert!(c > b, "a taken letter's id came back");
    }

    #[test]
    fn taking_mail_fills_the_first_free_cell_and_removes_the_letter() {
        let mut sheet = CharacterSheet::starting(Hero::Druid);
        let id = send(&mut sheet);
        let free = sheet
            .inventory()
            .cells()
            .iter()
            .position(Option::is_none)
            .expect("a free cell");
        assert_eq!(sheet.take_mail(id), Ok(ItemSlot::Inventory(free)));
        assert!(sheet.mailbox().is_empty());
        assert_eq!(
            sheet.inventory().get(free),
            Some(ItemStack::one(loot_pool()[0].id))
        );
    }

    #[test]
    fn a_full_bag_refuses_and_keeps_the_letter() {
        let mut sheet = CharacterSheet::bare(Hero::FireMage);
        let filler = ItemStack::one(loot_pool()[0].id);
        while sheet.inventory_mut().add(filler).is_some() {}
        assert_eq!(sheet.inventory().count(), INVENTORY_SLOTS);
        let id = send(&mut sheet);
        let before = sheet.clone();
        assert_eq!(sheet.take_mail(id), Err(TakeMailError::BagFull));
        assert_eq!(sheet, before, "a refused take changed the character");
    }

    #[test]
    fn an_unknown_letter_is_not_found() {
        let mut sheet = CharacterSheet::starting(Hero::Warrior);
        assert_eq!(sheet.take_mail(77), Err(TakeMailError::NotFound));
        assert!(!sheet.mark_read(77));
    }

    #[test]
    fn unread_count_follows_mark_read() {
        let mut sheet = CharacterSheet::starting(Hero::Warrior);
        let a = send(&mut sheet);
        send(&mut sheet);
        assert_eq!(sheet.unread_mail_count(), 2);
        assert!(sheet.mark_read(a));
        assert_eq!(sheet.unread_mail_count(), 1);
    }

    #[test]
    fn the_soft_cap_never_refuses_a_letter() {
        let mut sheet = CharacterSheet::bare(Hero::Warrior);
        for _ in 0..MAIL_SOFT_CAP + 5 {
            send(&mut sheet);
        }
        assert_eq!(sheet.mailbox().len(), MAIL_SOFT_CAP + 5);
    }

    #[test]
    fn restoring_a_duplicate_id_gives_it_a_fresh_one() {
        let mut mailbox = Mailbox::new();
        let letter = MailItem {
            id: 5,
            item: loot_pool()[0].id,
            sender: MailSender::DungeonChest,
            reason: MailReason::BagFull,
            received_at: 0.0,
            read: false,
        };
        mailbox.restore(letter);
        mailbox.restore(letter);
        let ids: Vec<_> = mailbox.letters().iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![5, 6]);
        assert_eq!(mailbox.next_id(), 7);
    }
}
