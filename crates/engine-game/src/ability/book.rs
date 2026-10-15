//! The runtime half of the ability table: cooldowns, charges, and why a cast was refused.

use crate::firemage::talent::TalentEffects;

use super::table::{ABILITY_COUNT, FireAbility};

/// Why an ability could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityError {
    /// The caster is dead.
    CasterDead,
    /// Every charge is spent.
    OnCooldown,
    /// Another ability was started less than a global cooldown ago.
    GlobalCooldown,
    /// Not enough Ultimate Energy.
    NotEnoughEnergy,
    /// It needs a target and none is selected.
    NoTarget,
    /// The selected target is dead or gone.
    TargetInvalid,
    /// The selected target is not something to attack — the player themselves, or an ally.
    ///
    /// Selecting yourself is legal and useful; setting yourself on fire is not, and the
    /// refusal has to say which of the two happened.
    TargetFriendly,
    /// The target is further away than it reaches.
    OutOfRange,
    /// The target is behind the caster.
    FacingWrongWay,
    /// Something in the level stands between the caster and the target.
    NoLineOfSight,
    /// Detonate without a full burn on the target.
    NotEnoughIgnite,
    /// A ground-targeted ability was cast without a placement.
    NeedsGroundTarget,
    /// The chosen ground is out of range.
    GroundOutOfRange,
    /// The Ultimate is switched off ([`super::ULTIMATE_ENABLED`]).
    UltimateDisabled,
    /// A movement was pressed while the Moving Fire Beam is being channelled.
    ///
    /// The mage may walk, slowly, but may not jump or Blink out of the beam.
    Channelling,
    /// Not enough Mana.
    NotEnoughMana,
    /// The Astral Power button was pressed without a full bar, or while already armed.
    NotEnoughAstralPower,
    /// Swiftmend was cast on a target with none of the Druid's heals over time on it.
    NoHealOverTime,
    /// A friendly ability was cast with nothing friendly to point it at.
    NoFriendlyTarget,
}

impl AbilityError {
    /// The refusal as a sentence, for the interface's proportional face.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::CasterDead => "You are dead.",
            Self::OnCooldown => "That ability is not ready yet.",
            Self::GlobalCooldown => "Not ready yet.",
            Self::NotEnoughEnergy => "Not enough Ultimate Energy.",
            Self::NoTarget => "You have no target.",
            Self::TargetInvalid => "Invalid target.",
            Self::TargetFriendly => "That is not an enemy.",
            Self::OutOfRange => "Out of range.",
            Self::FacingWrongWay => "You are facing the wrong way.",
            Self::NoLineOfSight => "Target not in line of sight.",
            Self::NotEnoughIgnite => "Requires maximum Ignite on the target.",
            Self::NeedsGroundTarget => "Choose a location.",
            Self::GroundOutOfRange => "That location is too far away.",
            Self::UltimateDisabled => "Not available yet.",
            Self::Channelling => "You cannot do that while channelling.",
            Self::NotEnoughMana => "Not enough Mana.",
            Self::NotEnoughAstralPower => "Not enough Astral Power.",
            Self::NoHealOverTime => "No heal over time on the target.",
            Self::NoFriendlyTarget => "No friendly target.",
        }
    }

    /// Short uppercase description, for the UI font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CasterDead => "YOU ARE DEAD",
            Self::OnCooldown => "NOT READY",
            Self::GlobalCooldown => "TOO SOON",
            Self::NotEnoughEnergy => "NOT ENOUGH ENERGY",
            Self::NoTarget => "NO TARGET",
            Self::TargetInvalid => "TARGET IS GONE",
            Self::TargetFriendly => "THAT IS NOT AN ENEMY",
            Self::OutOfRange => "OUT OF RANGE",
            Self::FacingWrongWay => "YOU ARE FACING THE WRONG DIRECTION.",
            Self::NoLineOfSight => "TARGET IS NOT IN LINE OF SIGHT",
            Self::NotEnoughIgnite => "NEEDS MAXIMUM IGNITE",
            Self::NeedsGroundTarget => "CHOOSE A LOCATION",
            Self::GroundOutOfRange => "TOO FAR AWAY",
            Self::UltimateDisabled => "NOT AVAILABLE YET",
            Self::Channelling => "CHANNELLING",
            Self::NotEnoughMana => "NOT ENOUGH MANA",
            Self::NotEnoughAstralPower => "NOT ENOUGH ASTRAL POWER",
            Self::NoHealOverTime => "NO HEAL OVER TIME ON THE TARGET",
            Self::NoFriendlyTarget => "NO FRIENDLY TARGET",
        }
    }
}

/// One ability's cooldown and charges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityState {
    /// Charges ready to spend.
    pub charges: u8,
    /// Seconds until the next charge returns. Zero when the bucket is full.
    pub recharge: f32,
}

impl AbilityState {
    /// A full bucket.
    #[must_use]
    pub fn full(charges: u8) -> Self {
        Self {
            charges: charges.max(1),
            recharge: 0.0,
        }
    }
}

/// Every ability's runtime state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityBook {
    states: [AbilityState; ABILITY_COUNT],
    /// Charges a hero talent adds to an ability, beyond the FireMage's [`TalentEffects`].
    extra_charges: [u8; ABILITY_COUNT],
    /// Seconds a hero talent adds to an ability's cooldown.
    extra_cooldown: [f32; ABILITY_COUNT],
}

impl Default for AbilityBook {
    fn default() -> Self {
        Self::new(&TalentEffects::default())
    }
}

impl AbilityBook {
    /// Every ability ready, with the charges the talents allow.
    #[must_use]
    pub fn new(effects: &TalentEffects) -> Self {
        let mut states = [AbilityState::full(1); ABILITY_COUNT];
        for ability in FireAbility::ALL {
            states[ability.index()] =
                AbilityState::full(ability.definition().charges_with(effects));
        }
        Self {
            states,
            extra_charges: [0; ABILITY_COUNT],
            extra_cooldown: [0.0; ABILITY_COUNT],
        }
    }

    /// Charges `ability` holds: its row, the FireMage's talents and any other hero's
    /// adjustment (see [`Self::adjust`]).
    #[must_use]
    pub fn max_charges(&self, ability: FireAbility, effects: &TalentEffects) -> u8 {
        ability
            .definition()
            .charges_with(effects)
            .saturating_add(self.extra_charges[ability.index()])
    }

    /// `ability`'s cooldown in seconds, with every adjustment.
    #[must_use]
    pub fn cooldown(&self, ability: FireAbility, effects: &TalentEffects) -> f32 {
        let base = ability.definition().cooldown_with(effects);
        if base <= 0.0 {
            return base;
        }
        base + self.extra_cooldown[ability.index()]
    }

    /// Sets what a hero talent adds to `ability`: extra charges and extra cooldown seconds.
    ///
    /// A bucket that was full when it gains a charge gains it ready, so buying the talent is
    /// not a wait for the first use.
    pub fn adjust(
        &mut self,
        ability: FireAbility,
        extra_charges: u8,
        extra_cooldown: f32,
        effects: &TalentEffects,
    ) {
        let was = self.max_charges(ability, effects);
        self.extra_charges[ability.index()] = extra_charges;
        self.extra_cooldown[ability.index()] = extra_cooldown.max(0.0);
        let now = self.max_charges(ability, effects);
        let state = &mut self.states[ability.index()];
        if now > was && state.charges == was {
            state.charges = now;
            state.recharge = 0.0;
        }
        state.charges = state.charges.min(now);
    }

    /// Everything about `ability`'s cooldown: charges ready and time to the next one.
    #[must_use]
    pub fn state(&self, ability: FireAbility) -> AbilityState {
        self.states[ability.index()]
    }

    /// Replaces `ability`'s cooldown state: how a networked client is brought back in line
    /// with the authoritative simulation.
    pub fn set_state(&mut self, ability: FireAbility, state: AbilityState) {
        self.states[ability.index()] = AbilityState {
            charges: state.charges,
            recharge: if state.recharge.is_finite() {
                state.recharge.max(0.0)
            } else {
                0.0
            },
        };
    }

    /// Charges ready on `ability`.
    #[must_use]
    pub fn charges(&self, ability: FireAbility) -> u8 {
        self.states[ability.index()].charges
    }

    /// Seconds until the next charge of `ability` returns.
    #[must_use]
    pub fn recharge_remaining(&self, ability: FireAbility) -> f32 {
        self.states[ability.index()].recharge
    }

    /// How far through the current recharge `ability` is, in `0.0..=1.0`.
    #[must_use]
    pub fn recharge_fraction(&self, ability: FireAbility, effects: &TalentEffects) -> f32 {
        let state = self.states[ability.index()];
        if state.recharge <= 0.0 {
            return 1.0;
        }
        let cooldown = self.cooldown(ability, effects);
        if cooldown <= 0.0 {
            return 1.0;
        }
        (1.0 - state.recharge / cooldown).clamp(0.0, 1.0)
    }

    /// Whether `ability` has a charge to spend.
    #[must_use]
    pub fn is_ready(&self, ability: FireAbility) -> bool {
        self.states[ability.index()].charges > 0
    }

    /// Spends a charge and starts the recharge if it was not already running.
    pub fn spend(&mut self, ability: FireAbility, effects: &TalentEffects) -> bool {
        let cooldown = self.cooldown(ability, effects);
        if cooldown <= 0.0 {
            return true;
        }
        let state = &mut self.states[ability.index()];
        if state.charges == 0 {
            return false;
        }
        state.charges -= 1;
        if state.recharge <= 0.0 && cooldown > 0.0 {
            state.recharge = cooldown;
        }
        true
    }

    /// Takes `seconds` off the recharge of `ability`, possibly returning a charge.
    ///
    /// Returns the seconds that were *not* used, which is what a cooldown reduction wastes
    /// when the ability was already ready. The simulator reports that number; nothing in
    /// gameplay depends on it.
    pub fn reduce_cooldown(
        &mut self,
        ability: FireAbility,
        seconds: f32,
        effects: &TalentEffects,
    ) -> f32 {
        if seconds <= 0.0 {
            return 0.0;
        }
        let maximum = self.max_charges(ability, effects);
        let cooldown = self.cooldown(ability, effects);
        let state = &mut self.states[ability.index()];
        if state.recharge <= 0.0 {
            return seconds;
        }
        state.recharge -= seconds;
        while state.recharge <= 0.0 && state.charges < maximum {
            state.charges += 1;
            if state.charges < maximum && cooldown > 0.0 {
                state.recharge += cooldown;
            } else {
                let wasted = -state.recharge;
                state.recharge = 0.0;
                return wasted.max(0.0);
            }
        }
        0.0
    }

    /// Hands `ability` one charge, up to its maximum.
    pub fn grant_charge(&mut self, ability: FireAbility, effects: &TalentEffects) {
        let maximum = self.max_charges(ability, effects);
        let state = &mut self.states[ability.index()];
        if state.charges < maximum {
            state.charges += 1;
        }
        if state.charges >= maximum {
            state.recharge = 0.0;
        }
    }

    /// Advances every recharge by `seconds`, scaled by haste where the ability allows it.
    pub fn tick(&mut self, seconds: f32, haste: f32, effects: &TalentEffects) {
        for ability in FireAbility::ALL {
            let definition = ability.definition();
            let scaled = if definition.cooldown_hasted {
                seconds * (1.0 + haste)
            } else {
                seconds
            };
            let maximum = self.max_charges(ability, effects);
            let cooldown = self.cooldown(ability, effects);
            let state = &mut self.states[ability.index()];
            state.charges = state.charges.min(maximum);
            if state.recharge <= 0.0 {
                continue;
            }
            state.recharge -= scaled;
            while state.recharge <= 0.0 && state.charges < maximum {
                state.charges += 1;
                if state.charges < maximum && cooldown > 0.0 {
                    state.recharge += cooldown;
                } else {
                    state.recharge = 0.0;
                }
            }
            if state.charges >= maximum {
                state.recharge = 0.0;
            }
        }
    }

    /// Refills everything. For a reset.
    pub fn refill(&mut self, effects: &TalentEffects) {
        let (charges, cooldown) = (self.extra_charges, self.extra_cooldown);
        *self = Self::new(effects);
        self.extra_charges = charges;
        self.extra_cooldown = cooldown;
        for ability in FireAbility::ALL {
            self.states[ability.index()] = AbilityState::full(self.max_charges(ability, effects));
        }
    }
}
