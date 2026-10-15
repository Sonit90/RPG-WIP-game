//! Talents, seen through the simulation.
//!
//! Each hero's grid and derived effects live beside that hero's class state - the FireMage's
//! in [`crate::firemage::FireMage`], the Druid's in [`crate::druid::Druid`], the Warrior's in
//! the simulation - and so are swapped with the rest of a player's state in a party
//! (`players.rs`). This file is the one front door: the active hero's grid, however it is
//! stored.

use crate::hero::Hero;
use crate::talent::{TalentError, TalentGrid};

use super::Simulation;

impl Simulation {
    /// The talent grid of the active player's hero.
    #[must_use]
    pub fn talent_grid(&self) -> TalentGrid {
        self.talent_grid_of(self.hero)
    }

    /// The talent grid `hero` has in this simulation, active or not.
    #[must_use]
    pub fn talent_grid_of(&self, hero: Hero) -> TalentGrid {
        match hero {
            Hero::FireMage => self.mage.talents,
            Hero::Druid => self.druid.talents,
            Hero::Warrior => self.warrior_talents,
        }
    }

    /// Replaces the talents of `grid`'s hero for the active player: a respec, the server
    /// building a fight, the simulator setting a build.
    pub fn set_talents(&mut self, grid: TalentGrid) {
        match grid.hero() {
            Hero::FireMage => self.mage.set_talents(grid),
            Hero::Druid => self.druid.set_talents(grid),
            Hero::Warrior => {
                self.warrior_talents = grid;
                self.warrior_effects =
                    crate::talent::warrior::WarriorTalentEffects::from_grid(&grid);
            }
        }
    }

    /// Buys table row `index` of the active hero's tree.
    ///
    /// # Errors
    ///
    /// Returns [`TalentError`] if it is owned already, unknown or cannot be afforded.
    pub fn purchase_talent_index(&mut self, index: usize) -> Result<(), TalentError> {
        let mut grid = self.talent_grid();
        grid.purchase_index(index)?;
        self.apply_grid(grid);
        Ok(())
    }

    /// Refunds every talent of the active hero.
    pub fn reset_talents(&mut self) {
        let mut grid = self.talent_grid();
        grid.reset();
        self.apply_grid(grid);
    }

    /// A menu change in the middle of a fight: the same as [`Self::set_talents`], except a
    /// FireMage's cooldowns are not refilled by it.
    fn apply_grid(&mut self, grid: TalentGrid) {
        if grid.hero() == Hero::FireMage {
            self.mage.talents = grid;
            self.mage.refresh_talents();
        } else {
            self.set_talents(grid);
        }
    }
}
