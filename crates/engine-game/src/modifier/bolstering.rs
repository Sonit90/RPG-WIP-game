//! Bolstering: what a slain monster's neighbours gain, and the bounded list that remembers it.
//!
//! ```text
//!   monster dies ──▶ every living, non-boss enemy within `radius` gains one stack
//!                      stack n: max health × (1 + n·h), damage × (1 + n·d)   (n ≤ max_stacks)
//! ```
//!
//! # Stacks, not compounding
//!
//! Each stack adds a fraction of the monster's **base** (`base_max` is remembered the first
//! time it is bolstered), so the tenth death in a room is worth as much as the first and the
//! cap is a plain number rather than a runaway product.
//!
//! # It changes no health by itself
//!
//! [`BolsterField`] only counts. The simulation applies the stacks to a body's health and to
//! its damage, and puts both back when the monster is restored (a reset, a wipe).

use crate::entity::EntityId;

/// One monster's bolstering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bolster {
    /// Whose it is.
    pub entity: EntityId,
    /// How many deaths have fed it.
    pub stacks: u8,
    /// Its maximum health before it was first bolstered.
    pub base_max: f32,
}

/// Every bolstered monster.
#[derive(Debug, Clone, Default)]
pub struct BolsterField {
    entries: Vec<Bolster>,
}

impl BolsterField {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every bolstered monster.
    #[must_use]
    pub fn entries(&self) -> &[Bolster] {
        &self.entries
    }

    /// Stacks `entity` carries. Zero for a monster that was never bolstered.
    #[must_use]
    pub fn stacks(&self, entity: EntityId) -> u8 {
        self.entries
            .iter()
            .find(|entry| entry.entity == entity)
            .map_or(0, |entry| entry.stacks)
    }

    /// Gives `entity` one more stack, up to `max_stacks`, remembering `current_max` as its base
    /// the first time. Returns the entry as it now stands, or `None` if it was already at the
    /// cap.
    pub fn add_stack(
        &mut self,
        entity: EntityId,
        current_max: f32,
        max_stacks: u8,
    ) -> Option<Bolster> {
        if max_stacks == 0 {
            return None;
        }
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.entity == entity) {
            if entry.stacks >= max_stacks {
                return None;
            }
            entry.stacks += 1;
            return Some(*entry);
        }
        let entry = Bolster {
            entity,
            stacks: 1,
            base_max: current_max,
        };
        self.entries.push(entry);
        Some(entry)
    }

    /// Takes `entity`'s bolstering away, returning it so the caller can undo it.
    pub fn remove(&mut self, entity: EntityId) -> Option<Bolster> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.entity == entity)?;
        Some(self.entries.remove(index))
    }

    /// Takes all of it away, returning what there was.
    pub fn drain(&mut self) -> Vec<Bolster> {
        std::mem::take(&mut self.entries)
    }
}

/// What a monster's maximum health or damage is multiplied by at `stacks`, for `pct` per
/// stack (after magnitude). One at no stacks.
#[must_use]
pub fn factor(stacks: u8, pct: f32) -> f32 {
    1.0 + f32::from(stacks) * pct.max(0.0) / 100.0
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 1)
    }

    #[test]
    fn stacks_accumulate_to_the_cap_and_remember_the_base() {
        let mut field = BolsterField::new();
        let first = field.add_stack(id(1), 100.0, 3).expect("a first stack");
        assert_eq!((first.stacks, first.base_max), (1, 100.0));
        // A later call carries a bigger current max, but the base is what it was.
        field.add_stack(id(1), 110.0, 3);
        let third = field.add_stack(id(1), 120.0, 3).expect("a third stack");
        assert_eq!((third.stacks, third.base_max), (3, 100.0));
        assert_eq!(field.add_stack(id(1), 130.0, 3), None, "capped");
        assert_eq!(field.stacks(id(1)), 3);
        assert_eq!(field.stacks(id(2)), 0);
    }

    #[test]
    fn a_zero_cap_bolsters_nobody() {
        let mut field = BolsterField::new();
        assert_eq!(field.add_stack(id(1), 100.0, 0), None);
        assert!(field.entries().is_empty());
    }

    #[test]
    fn removing_returns_the_entry_once() {
        let mut field = BolsterField::new();
        field.add_stack(id(1), 80.0, 5);
        assert_eq!(field.remove(id(1)).map(|entry| entry.base_max), Some(80.0));
        assert_eq!(field.remove(id(1)), None);
    }

    #[test]
    fn a_factor_adds_a_share_of_the_base_per_stack() {
        assert_eq!(factor(0, 10.0), 1.0);
        assert!((factor(3, 10.0) - 1.3).abs() < 1e-6);
        assert_eq!(factor(4, -10.0), 1.0, "a negative share is ignored");
    }
}
