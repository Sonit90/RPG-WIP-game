//! Active trinkets: a button a worn trinket gives, with a cooldown of its own.
//!
//! ```text
//! loot.json  "active": { effect, cooldown_s, radius_m, duration_s, magnitude }
//!      │  (read by engine-realm, handed over as plain data)
//!      ▼
//! ItemDef::active ─► TrinketActive ──► PlayerCommand::UseTrinket(slot)
//!                                          │  sim/trinket_exec.rs
//!                                          ▼
//!                 status (ward) · dispel · cooldown cut · absorb pool
//! ```
//!
//! # What scales and what does not
//!
//! An item's quality and sharpening scale its **passive stats** (`ItemDef::stats_for`) and
//! nothing about its active: the cooldown, the radius, the duration and the magnitude are the
//! row's numbers for every copy. A 60 % roll of a Group Absorb trinket still shields for 20 %
//! of maximum health; it just carries less Haste. That keeps what a party can plan around the
//! same whichever copy is worn.
//!
//! # Where the state lives
//!
//! A trinket's cooldown belongs to the *slot* (`Trinket1`, `Trinket2`), per player, and is
//! swapped in and out with the rest of a player's state (see `sim::players`). Putting a new
//! trinket into a slot starts a short cooldown on it ([`EQUIP_COOLDOWN`]), so swapping gear
//! mid-fight cannot be used to get a second use out of one.

use crate::character::{EquipSlot, ItemId};
use crate::warrior::Absorb;

/// How many trinket slots a character has.
pub const TRINKET_SLOTS: usize = 2;

/// The longest cooldown that merely equipping a trinket starts, in seconds.
///
/// A trinket whose own cooldown is shorter starts at that instead.
pub const EQUIP_COOLDOWN: f32 = 30.0;

/// The equipment slot behind trinket button `index` (`0` is `Trinket1`).
#[must_use]
pub const fn slot_of(index: usize) -> Option<EquipSlot> {
    match index {
        0 => Some(EquipSlot::Trinket1),
        1 => Some(EquipSlot::Trinket2),
        _ => None,
    }
}

/// What a trinket's button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrinketEffect {
    /// Party members in range take less damage for a while.
    GroupDamageReduction,
    /// Harmful effects come off party members in range; helpful ones come off enemies in range.
    MassDispel,
    /// Every ability cooldown of every party member in range runs down faster, once.
    GroupCooldownReduction,
    /// Party members in range are shielded for a share of their maximum health.
    GroupAbsorb,
}

impl TrinketEffect {
    /// Every effect, in wire order. **Never reorder**: the realm sends the index.
    pub const ALL: [Self; 4] = [
        Self::GroupDamageReduction,
        Self::MassDispel,
        Self::GroupCooldownReduction,
        Self::GroupAbsorb,
    ];

    /// Dense index, the wire form.
    #[must_use]
    pub fn index(self) -> u8 {
        match self {
            Self::GroupDamageReduction => 0,
            Self::MassDispel => 1,
            Self::GroupCooldownReduction => 2,
            Self::GroupAbsorb => 3,
        }
    }

    /// The effect at wire index `index`.
    #[must_use]
    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(usize::from(index)).copied()
    }

    /// The name the data catalog writes (`"group_damage_reduction"`).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::GroupDamageReduction => "group_damage_reduction",
            Self::MassDispel => "mass_dispel",
            Self::GroupCooldownReduction => "group_cooldown_reduction",
            Self::GroupAbsorb => "group_absorb",
        }
    }

    /// The effect a catalog name stands for.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|effect| effect.key() == key)
    }

    /// Title case name, for a tooltip.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::GroupDamageReduction => "Group Damage Reduction",
            Self::MassDispel => "Mass Dispel",
            Self::GroupCooldownReduction => "Group Cooldown Reduction",
            Self::GroupAbsorb => "Group Absorb",
        }
    }

    /// Whether the effect needs a duration (the others are instant).
    #[must_use]
    pub fn has_duration(self) -> bool {
        matches!(self, Self::GroupDamageReduction | Self::GroupAbsorb)
    }
}

/// The active ability a trinket gives, as plain data.
///
/// `Copy`, and `PartialEq` over floats: it rides inside every [`crate::character::ItemDef`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrinketActive {
    /// What it does.
    pub effect: TrinketEffect,
    /// Seconds before it can be used again.
    pub cooldown: f32,
    /// How far from the user it reaches, in metres.
    pub radius: f32,
    /// Seconds an effect it leaves behind lasts. Zero for an instant one.
    pub duration: f32,
    /// Its strength, as a fraction: the damage taken off, the share of cooldown removed, the
    /// share of maximum health shielded. Unused by Mass Dispel.
    pub magnitude: f32,
}

impl TrinketActive {
    /// Whether every number is usable: finite, a positive cooldown and radius, a magnitude in
    /// `0..=1`, a duration where the effect needs one.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let finite = [self.cooldown, self.radius, self.duration, self.magnitude]
            .iter()
            .all(|v| v.is_finite());
        finite
            && self.cooldown > 0.0
            && self.radius > 0.0
            && self.duration >= 0.0
            && (0.0..=1.0).contains(&self.magnitude)
            && (!self.effect.has_duration() || self.duration > 0.0)
            && (self.effect == TrinketEffect::MassDispel || self.magnitude > 0.0)
    }

    /// What it does, in a sentence, with its numbers.
    #[must_use]
    pub fn description(&self) -> String {
        let percent = (self.magnitude * 100.0).round();
        match self.effect {
            TrinketEffect::GroupDamageReduction => format!(
                "Party members within {:.0} m take {percent:.0}% less damage for {:.0} sec.",
                self.radius, self.duration
            ),
            TrinketEffect::MassDispel => format!(
                "Removes harmful magic, poison, disease and curse effects from party members \
                 within {:.0} m, and helpful effects from enemies within {:.0} m.",
                self.radius, self.radius
            ),
            TrinketEffect::GroupCooldownReduction => format!(
                "Reduces the remaining cooldown of every ability of party members within \
                 {:.0} m by {percent:.0}%.",
                self.radius
            ),
            TrinketEffect::GroupAbsorb => format!(
                "Shields party members within {:.0} m for {percent:.0}% of their maximum \
                 health for {:.0} sec.",
                self.radius, self.duration
            ),
        }
    }
}

/// One player's trinket state: two cooldowns and the absorb pool Group Absorb leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct TrinketState {
    cooldown: [f32; TRINKET_SLOTS],
    /// The Group Absorb shield. Consumed ahead of the Warrior's own (see
    /// `Simulation::absorb_of`).
    pub absorb: Absorb,
    /// What was worn last time anyone looked, to notice a trinket being put on.
    worn: Option<[ItemId; TRINKET_SLOTS]>,
}

impl Default for TrinketState {
    fn default() -> Self {
        Self::new()
    }
}

impl TrinketState {
    /// Nothing cooling, nothing absorbed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cooldown: [0.0; TRINKET_SLOTS],
            absorb: Absorb::none(),
            worn: None,
        }
    }

    /// Back to ready, as a reset of the fight does. Keeps what is worn.
    pub fn reset(&mut self) {
        self.cooldown = [0.0; TRINKET_SLOTS];
        self.absorb.clear();
    }

    /// Seconds left on button `index`'s cooldown. Zero for an unknown button.
    #[must_use]
    pub fn remaining(&self, index: usize) -> f32 {
        self.cooldown.get(index).copied().unwrap_or(0.0).max(0.0)
    }

    /// Starts button `index`'s cooldown, keeping the longer of the two if it is running.
    pub fn start(&mut self, index: usize, seconds: f32) {
        if let Some(cooldown) = self.cooldown.get_mut(index)
            && seconds.is_finite()
            && seconds > *cooldown
        {
            *cooldown = seconds;
        }
    }

    /// Sets button `index`'s remaining cooldown outright: how a client is told what the
    /// authority has.
    pub fn set_remaining(&mut self, index: usize, seconds: f32) {
        if let Some(cooldown) = self.cooldown.get_mut(index) {
            *cooldown = if seconds.is_finite() {
                seconds.max(0.0)
            } else {
                0.0
            };
        }
    }

    /// Advances both cooldowns and the absorb's clock.
    pub fn tick(&mut self, seconds: f32) {
        for cooldown in &mut self.cooldown {
            if *cooldown > 0.0 {
                *cooldown -= seconds;
                if *cooldown < crate::TIME_EPSILON {
                    *cooldown = 0.0;
                }
            }
        }
        self.absorb.tick(seconds);
    }

    /// Records what is worn now and returns the buttons whose item changed since the last
    /// look. The first look only records.
    pub fn note_worn(&mut self, worn: [ItemId; TRINKET_SLOTS]) -> [bool; TRINKET_SLOTS] {
        let changed = match self.worn {
            Some(before) => [before[0] != worn[0], before[1] != worn[1]],
            None => [false; TRINKET_SLOTS],
        };
        self.worn = Some(worn);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_round_trip_through_their_index_and_catalog_name() {
        for effect in TrinketEffect::ALL {
            assert_eq!(TrinketEffect::from_index(effect.index()), Some(effect));
            assert_eq!(TrinketEffect::from_key(effect.key()), Some(effect));
        }
        assert_eq!(TrinketEffect::from_index(9), None);
        assert_eq!(TrinketEffect::from_key("nope"), None);
    }

    #[test]
    fn a_cooldown_runs_down_and_the_longer_start_wins() {
        let mut state = TrinketState::new();
        state.start(0, 10.0);
        state.start(0, 5.0);
        assert_eq!(state.remaining(0), 10.0);
        for _ in 0..60 {
            state.tick(1.0 / 60.0);
        }
        assert!((state.remaining(0) - 9.0).abs() < 1e-3);
        assert_eq!(state.remaining(1), 0.0);
        assert_eq!(state.remaining(7), 0.0);
    }

    #[test]
    fn the_first_look_at_what_is_worn_changes_nothing() {
        let mut state = TrinketState::new();
        assert_eq!(state.note_worn([ItemId(2000), ItemId(0)]), [false, false]);
        assert_eq!(state.note_worn([ItemId(2000), ItemId(0)]), [false, false]);
        assert_eq!(state.note_worn([ItemId(2001), ItemId(0)]), [true, false]);
    }

    #[test]
    fn validity_asks_for_the_numbers_each_effect_needs() {
        let ward = TrinketActive {
            effect: TrinketEffect::GroupDamageReduction,
            cooldown: 120.0,
            radius: 30.0,
            duration: 8.0,
            magnitude: 0.15,
        };
        assert!(ward.is_valid());
        assert!(
            !TrinketActive {
                duration: 0.0,
                ..ward
            }
            .is_valid()
        );
        assert!(
            !TrinketActive {
                magnitude: 1.5,
                ..ward
            }
            .is_valid()
        );
        assert!(
            TrinketActive {
                effect: TrinketEffect::MassDispel,
                duration: 0.0,
                magnitude: 0.0,
                ..ward
            }
            .is_valid()
        );
    }
}
