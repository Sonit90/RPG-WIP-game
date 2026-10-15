//! The Warrior's talents: none yet.
//!
//! The framework is hero-generic, so the Warrior needs only rows in `talents.json`, variants
//! in [`WarriorTalent`] (and the matching rows in [`DEFS`]), and fields on
//! [`WarriorTalentEffects`] that the Warrior's executor reads.

use super::{TalentDef, TalentGrid, TalentKey};
use crate::hero::Hero;

/// Every Warrior talent. Empty until the Warrior's tree exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarriorTalent {}

impl TalentKey for WarriorTalent {
    const HERO: Hero = Hero::Warrior;
    const ALL: &'static [Self] = &[];

    fn index(self) -> usize {
        match self {}
    }
}

/// The built-in Warrior rows.
pub const DEFS: [TalentDef; 0] = [];

/// What the Warrior's bought talents mean. A placeholder: nothing reads it yet.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct WarriorTalentEffects {}

impl WarriorTalentEffects {
    /// Resolves a grid into the numbers gameplay uses.
    #[must_use]
    pub fn from_grid(_grid: &TalentGrid) -> Self {
        Self {}
    }
}
