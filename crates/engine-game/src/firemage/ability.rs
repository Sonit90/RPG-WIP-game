//! FireMage's abilities, as data, plus the runtime that cools them down.
//!
//! ```text
//! FireAbility (which)  ->  AbilityDef (numbers)  ->  AbilityBook (cooldowns and charges)
//! ```
//!
//! [`ABILITIES`] is a table: cast times, cooldowns, charges, ranges, radii, Heat, Energy and
//! the damage numbers all live in one place a designer edits. Nothing in the executor, the
//! UI or the tooltips restates any of them, and the tooltip text is *generated* from the same
//! rows so a description can never drift from the ability it describes.
//!
//! Charges and cooldowns are one system: an ability with one charge is the ordinary case, and
//! an ability with two is the same rules with a bigger bucket. That is why Flame Lance needed
//! no special handling to get its second charge, and why Meteor Shower is a talent that edits
//! two numbers rather than a code path.

use super::talent::TalentEffects;

/// Abilities on the regular bar.
pub const ABILITY_SLOTS: usize = 10;

/// Every ability, including the Ultimate.
pub const ABILITY_COUNT: usize = 11;

/// Base chance any fire spell critically strikes.
pub const BASE_CRIT_CHANCE: f32 = 0.15;

/// Damage multiplier of a critical strike.
pub const CRIT_MULTIPLIER: f32 = 2.0;

/// Which ability. The first ten are the bar, in bar order; the eleventh is the Ultimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FireAbility {
    /// The filler: a short cast that keeps Ignite going.
    Firebolt,
    /// A hard single-target hit that rewards an existing burn.
    FlameLance,
    /// A cone that spreads Ignite.
    FlameWave,
    /// A ground-targeted rock.
    Meteor,
    /// A ground-targeted fire patch.
    Firestorm,
    /// The big cast, paid off by a full burn.
    Pyroblast,
    /// A ring of fire around the caster.
    Inferno,
    /// The payoff: consumes a full burn and explodes.
    FireDetonation,
    /// A defensive cooldown that burns whoever hits you.
    FlameShield,
    /// A short teleport.
    Blink,
    /// The Ultimate. Not on the bar.
    Cataclysm,
}

impl FireAbility {
    /// Every ability, in table order.
    pub const ALL: [Self; ABILITY_COUNT] = [
        Self::Firebolt,
        Self::FlameLance,
        Self::FlameWave,
        Self::Meteor,
        Self::Firestorm,
        Self::Pyroblast,
        Self::Inferno,
        Self::FireDetonation,
        Self::FlameShield,
        Self::Blink,
        Self::Cataclysm,
    ];

    /// The ten abilities on the regular bar, in bar order.
    pub const BAR: [Self; ABILITY_SLOTS] = [
        Self::Firebolt,
        Self::FlameLance,
        Self::FlameWave,
        Self::Meteor,
        Self::Firestorm,
        Self::Pyroblast,
        Self::Inferno,
        Self::FireDetonation,
        Self::FlameShield,
        Self::Blink,
    ];

    /// Dense index into the ability arrays.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The ability in bar slot `slot`, if there is one.
    #[must_use]
    pub fn from_slot(slot: usize) -> Option<Self> {
        Self::BAR.get(slot).copied()
    }

    /// Which bar slot this ability sits in, or `None` for the Ultimate.
    #[must_use]
    pub fn bar_slot(self) -> Option<usize> {
        Self::BAR.iter().position(|ability| *ability == self)
    }

    /// Whether this is the Ultimate, which lives outside the bar.
    #[must_use]
    pub fn is_ultimate(self) -> bool {
        matches!(self, Self::Cataclysm)
    }

    /// This ability's definition.
    #[must_use]
    pub fn definition(self) -> &'static AbilityDef {
        &ABILITIES[self.index()]
    }

    /// The key that casts it: 1-0 across the bar, R for the Ultimate.
    #[must_use]
    pub fn key_hint(self) -> &'static str {
        match self.bar_slot() {
            Some(slot) => ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"][slot],
            None => "R",
        }
    }
}

/// How an ability picks what it hits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetMode {
    /// The current target, within `range`.
    Single,
    /// A cone of `range` metres in front of the caster.
    Cone,
    /// A circle the player places with the mouse.
    Ground,
    /// A circle centred on the caster.
    PointBlank,
    /// The caster only.
    SelfCast,
    /// A movement, not an attack.
    Movement,
}

impl TargetMode {
    /// Whether the player has to have something selected.
    #[must_use]
    pub fn needs_target(self) -> bool {
        matches!(self, Self::Single)
    }

    /// Whether the player places this ability with the mouse.
    #[must_use]
    pub fn is_ground_targeted(self) -> bool {
        matches!(self, Self::Ground)
    }
}

/// One ability, entirely as data.
///
/// Every field the UI, the tooltip and the executor need. A row here is the whole definition
/// of an ability's numbers; the executor supplies only the behaviour that cannot be a number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityDef {
    /// Which ability this describes.
    pub id: FireAbility,
    /// Uppercase name; the UI font has no lowercase glyphs.
    pub name: &'static str,
    /// One line of what it does, for the tooltip.
    pub description: &'static str,
    /// How it picks what it hits.
    pub target: TargetMode,
    /// Seconds of casting before it lands. Zero is instant.
    pub cast_time: f32,
    /// Seconds before a charge comes back.
    pub cooldown: f32,
    /// How many charges it holds. One is the ordinary case.
    pub charges: u8,
    /// Metres it reaches. For a cone, how far the cone extends.
    pub range: f32,
    /// Metres of effect radius, or zero if it hits one thing.
    pub radius: f32,
    /// Seconds the effect lasts after it lands, or zero if it is instant.
    pub duration: f32,
    /// Seconds between the cast landing and the effect, for Meteor and Cataclysm.
    pub impact_delay: f32,
    /// Damage on impact, before every multiplier.
    pub damage: f32,
    /// Damage each periodic tick deals, for the zones.
    pub tick_damage: f32,
    /// Seconds between periodic ticks.
    pub tick_interval: f32,
    /// Ignite stacks applied to everything hit.
    pub ignite_stacks: u8,
    /// Heat generated by one cast, or per target for the cone.
    pub heat: f32,
    /// Most Heat one cast can generate, or zero for no cap.
    pub heat_cap: f32,
    /// Ultimate Energy generated by one cast.
    pub energy: f32,
    /// Ultimate Energy it costs. Only the Ultimate costs any.
    pub energy_cost: f32,
}

impl AbilityDef {
    /// Whether it can hit more than one thing.
    #[must_use]
    pub fn is_area(&self) -> bool {
        self.radius > 0.0 || matches!(self.target, TargetMode::Cone)
    }

    /// Whether it holds more than one charge.
    #[must_use]
    pub fn has_charges(&self) -> bool {
        self.charges > 1
    }

    /// Its cast time with talents and haste applied.
    ///
    /// Haste shortens a cast; Quick Casting shortens Firebolt specifically. Both are here so
    /// nothing else has to remember that a cast time is not the number in the table.
    #[must_use]
    pub fn cast_time_with(&self, effects: &TalentEffects, haste: f32) -> f32 {
        let scale = if self.id == FireAbility::Firebolt {
            effects.firebolt_cast_scale
        } else {
            1.0
        };
        (self.cast_time * scale / (1.0 + haste)).max(0.0)
    }

    /// Its cooldown with talents applied.
    #[must_use]
    pub fn cooldown_with(&self, effects: &TalentEffects) -> f32 {
        match self.id {
            FireAbility::Meteor if effects.meteor_cooldown_override > 0.0 => {
                effects.meteor_cooldown_override
            }
            FireAbility::Inferno => (self.cooldown - effects.inferno_cooldown_reduction).max(0.0),
            _ => self.cooldown,
        }
    }

    /// How many charges it holds with talents applied.
    #[must_use]
    pub fn charges_with(&self, effects: &TalentEffects) -> u8 {
        match self.id {
            FireAbility::Meteor => self.charges + effects.meteor_extra_charges,
            _ => self.charges,
        }
    }
}

/// The whole class, in bar order. The Ultimate is last.
pub static ABILITIES: [AbilityDef; ABILITY_COUNT] = [
    AbilityDef {
        id: FireAbility::Firebolt,
        name: "FIREBOLT",
        description: "FIRE DAMAGE. ALWAYS IGNITES AN UNBURNT TARGET, 50% CHANCE OTHERWISE",
        target: TargetMode::Single,
        cast_time: 1.0,
        cooldown: 0.0,
        charges: 1,
        range: 35.0,
        radius: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 120.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 1,
        heat: 5.0,
        heat_cap: 0.0,
        energy: 1.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::FlameLance,
        name: "FLAME LANCE",
        description: "STRONG FIRE DAMAGE. +40% AGAINST A BURNING TARGET",
        target: TargetMode::Single,
        cast_time: 1.5,
        cooldown: 8.0,
        charges: 2,
        range: 35.0,
        radius: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 260.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 0,
        heat: 10.0,
        heat_cap: 0.0,
        energy: 2.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::FlameWave,
        name: "FLAME WAVE",
        description: "A CONE OF FIRE. APPLIES 1 IGNITE STACK TO EVERY ENEMY HIT",
        target: TargetMode::Cone,
        cast_time: 1.2,
        cooldown: 10.0,
        charges: 1,
        range: 12.0,
        radius: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 180.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 1,
        heat: 3.0,
        heat_cap: 15.0,
        energy: 2.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Meteor,
        name: "METEOR",
        description: "A ROCK FALLS ON THE CHOSEN GROUND. APPLIES 2 IGNITE STACKS",
        target: TargetMode::Ground,
        cast_time: 1.0,
        cooldown: 18.0,
        charges: 1,
        range: 35.0,
        radius: 5.0,
        duration: 0.0,
        impact_delay: 1.25,
        damage: 420.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 2,
        heat: 15.0,
        heat_cap: 0.0,
        energy: 5.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Firestorm,
        name: "FIRESTORM",
        description: "A BURNING ZONE. EACH TICK HAS A 20% CHANCE TO APPLY IGNITE",
        target: TargetMode::Ground,
        cast_time: 0.8,
        cooldown: 24.0,
        charges: 1,
        range: 35.0,
        radius: 6.0,
        duration: 6.0,
        impact_delay: 0.0,
        damage: 0.0,
        tick_damage: 60.0,
        tick_interval: 0.5,
        ignite_stacks: 1,
        heat: 3.0,
        heat_cap: 30.0,
        energy: 8.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Pyroblast,
        name: "PYROBLAST",
        description: "VERY HIGH FIRE DAMAGE. +10% PER IGNITE STACK. ALWAYS CRITS AT MAXIMUM",
        target: TargetMode::Single,
        cast_time: 3.0,
        cooldown: 30.0,
        charges: 1,
        range: 35.0,
        radius: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 800.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 0,
        heat: 25.0,
        heat_cap: 0.0,
        energy: 10.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Inferno,
        name: "INFERNO",
        description: "FIRE SURROUNDS YOU. IGNITES EVERY SECOND. +20% CRIT NEXT TO A FULL BURN",
        target: TargetMode::PointBlank,
        cast_time: 0.0,
        cooldown: 45.0,
        charges: 1,
        range: 0.0,
        radius: 8.0,
        duration: 4.0,
        impact_delay: 0.0,
        damage: 0.0,
        tick_damage: 140.0,
        tick_interval: 1.0,
        ignite_stacks: 1,
        heat: 20.0,
        heat_cap: 0.0,
        energy: 10.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::FireDetonation,
        name: "FIRE DETONATION",
        description: "CONSUMES A FULL IGNITE FOR AN EXPLOSION. REQUIRES MAXIMUM STACKS",
        target: TargetMode::Single,
        cast_time: 0.0,
        cooldown: 8.0,
        charges: 1,
        range: 35.0,
        radius: 10.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 300.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 1,
        heat: 15.0,
        heat_cap: 0.0,
        energy: 3.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::FlameShield,
        name: "FLAME SHIELD",
        description: "TAKE 30% LESS DAMAGE. ATTACKERS ARE IGNITED",
        target: TargetMode::SelfCast,
        cast_time: 0.0,
        cooldown: 30.0,
        charges: 1,
        range: 0.0,
        radius: 0.0,
        duration: 5.0,
        impact_delay: 0.0,
        damage: 0.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 1,
        heat: 0.0,
        heat_cap: 0.0,
        energy: 0.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Blink,
        name: "BLINK",
        description: "TELEPORT 10 M FORWARD. STOPS AT WALLS RATHER THAN FAILING",
        target: TargetMode::Movement,
        cast_time: 0.0,
        cooldown: 15.0,
        charges: 1,
        range: 10.0,
        radius: 0.0,
        duration: 0.0,
        impact_delay: 0.0,
        damage: 0.0,
        tick_damage: 0.0,
        tick_interval: 0.0,
        ignite_stacks: 0,
        heat: 0.0,
        heat_cap: 0.0,
        energy: 0.0,
        energy_cost: 0.0,
    },
    AbilityDef {
        id: FireAbility::Cataclysm,
        name: "CATACLYSM",
        description: "A GIANT METEOR, THEN BURNING GROUND FOR 8 SECONDS. COSTS 100 ENERGY",
        target: TargetMode::Ground,
        cast_time: 1.0,
        cooldown: 0.0,
        charges: 1,
        range: 35.0,
        radius: 10.0,
        duration: 8.0,
        impact_delay: 0.5,
        damage: 1_200.0,
        tick_damage: 150.0,
        tick_interval: 1.0,
        ignite_stacks: 3,
        heat: 0.0,
        heat_cap: 0.0,
        energy: 0.0,
        energy_cost: 100.0,
    },
];

/// Why an ability could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityError {
    /// The caster is dead.
    CasterDead,
    /// Every charge is spent.
    OnCooldown,
    /// Not enough Ultimate Energy.
    NotEnoughEnergy,
    /// It needs a target and none is selected.
    NoTarget,
    /// The selected target is dead or gone.
    TargetInvalid,
    /// The target is further away than it reaches.
    OutOfRange,
    /// Fire Detonation without a full burn on the target.
    NotEnoughIgnite,
    /// A ground-targeted ability was cast without a placement.
    NeedsGroundTarget,
    /// The chosen ground is out of range.
    GroundOutOfRange,
}

impl AbilityError {
    /// Short uppercase description, for the UI font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CasterDead => "YOU ARE DEAD",
            Self::OnCooldown => "NOT READY",
            Self::NotEnoughEnergy => "NOT ENOUGH ENERGY",
            Self::NoTarget => "NO TARGET",
            Self::TargetInvalid => "TARGET IS GONE",
            Self::OutOfRange => "OUT OF RANGE",
            Self::NotEnoughIgnite => "NEEDS MAXIMUM IGNITE",
            Self::NeedsGroundTarget => "CHOOSE A LOCATION",
            Self::GroundOutOfRange => "TOO FAR AWAY",
        }
    }
}

/// One ability's cooldown and charges.
///
/// A charge bucket with one timer: the timer refills one charge, then restarts while any
/// charge is missing. That is the whole of the charge system, and it behaves correctly for
/// one charge as well as for three.
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
        Self { states }
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
    ///
    /// One means ready. This is what the radial cooldown overlay draws, and it is computed
    /// here rather than in the UI so both agree about what "half way back" means.
    #[must_use]
    pub fn recharge_fraction(&self, ability: FireAbility, effects: &TalentEffects) -> f32 {
        let state = self.states[ability.index()];
        if state.recharge <= 0.0 {
            return 1.0;
        }
        let cooldown = ability.definition().cooldown_with(effects);
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
    ///
    /// An ability with no cooldown - Firebolt - has nothing to spend: it is always ready, and
    /// taking its charge away would park it at zero forever with no timer to bring it back.
    pub fn spend(&mut self, ability: FireAbility, effects: &TalentEffects) -> bool {
        let cooldown = ability.definition().cooldown_with(effects);
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
    /// What Rapid Combustion and Meteoric Collapse do. Reducing a cooldown that is not
    /// running does nothing rather than banking the reduction.
    pub fn reduce_cooldown(&mut self, ability: FireAbility, seconds: f32, effects: &TalentEffects) {
        if seconds <= 0.0 {
            return;
        }
        let maximum = ability.definition().charges_with(effects);
        let cooldown = ability.definition().cooldown_with(effects);
        let state = &mut self.states[ability.index()];
        if state.recharge <= 0.0 {
            return;
        }
        state.recharge -= seconds;
        while state.recharge <= 0.0 && state.charges < maximum {
            state.charges += 1;
            if state.charges < maximum && cooldown > 0.0 {
                state.recharge += cooldown;
            } else {
                state.recharge = 0.0;
            }
        }
    }

    /// Hands `ability` one charge, up to its maximum.
    ///
    /// What a talent that widens the bucket does: the new charge is available at once.
    pub fn grant_charge(&mut self, ability: FireAbility, effects: &TalentEffects) {
        let maximum = ability.definition().charges_with(effects);
        let state = &mut self.states[ability.index()];
        if state.charges < maximum {
            state.charges += 1;
        }
        if state.charges >= maximum {
            state.recharge = 0.0;
        }
    }

    /// Advances every recharge by `seconds`, scaled by haste.
    pub fn tick(&mut self, seconds: f32, haste: f32, effects: &TalentEffects) {
        let scaled = seconds * (1.0 + haste);
        for ability in FireAbility::ALL {
            let maximum = ability.definition().charges_with(effects);
            let cooldown = ability.definition().cooldown_with(effects);
            let state = &mut self.states[ability.index()];
            // A talent that removed a charge could leave more banked than the bucket holds.
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
        *self = Self::new(effects);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::firemage::talent::{TalentGrid, TalentId};

    fn effects() -> TalentEffects {
        TalentEffects::default()
    }

    #[test]
    fn the_bar_holds_the_ten_abilities_in_the_designed_order() {
        let names: Vec<&str> = FireAbility::BAR
            .iter()
            .map(|ability| ability.definition().name)
            .collect();
        assert_eq!(
            names,
            [
                "FIREBOLT",
                "FLAME LANCE",
                "FLAME WAVE",
                "METEOR",
                "FIRESTORM",
                "PYROBLAST",
                "INFERNO",
                "FIRE DETONATION",
                "FLAME SHIELD",
                "BLINK",
            ]
        );
    }

    #[test]
    fn the_ultimate_is_not_on_the_bar() {
        assert!(FireAbility::Cataclysm.is_ultimate());
        assert_eq!(FireAbility::Cataclysm.bar_slot(), None);
        assert!(!FireAbility::BAR.contains(&FireAbility::Cataclysm));
        assert_eq!(FireAbility::BAR.len(), ABILITY_SLOTS);
        for ability in FireAbility::BAR {
            assert!(!ability.is_ultimate(), "{ability:?} is on the bar");
        }
    }

    #[test]
    fn every_ability_has_a_definition_in_its_own_slot() {
        for (index, ability) in FireAbility::ALL.into_iter().enumerate() {
            assert_eq!(ability.index(), index);
            assert_eq!(ability.definition().id, ability);
            assert!(!ability.definition().name.is_empty());
            assert!(!ability.definition().description.is_empty());
        }
    }

    #[test]
    fn the_table_matches_the_design_document() {
        // Spot-checks of the numbers the brief states outright, so a mistyped table is a
        // failing test rather than a wrong game.
        let firebolt = FireAbility::Firebolt.definition();
        assert_eq!(firebolt.cast_time, 1.0);
        assert_eq!(firebolt.cooldown, 0.0);
        assert_eq!(firebolt.range, 35.0);
        assert_eq!(firebolt.heat, 5.0);
        assert_eq!(firebolt.energy, 1.0);

        let lance = FireAbility::FlameLance.definition();
        assert_eq!(
            (lance.cast_time, lance.cooldown, lance.charges),
            (1.5, 8.0, 2)
        );

        let wave = FireAbility::FlameWave.definition();
        assert_eq!(
            (wave.cast_time, wave.cooldown, wave.range),
            (1.2, 10.0, 12.0)
        );
        assert_eq!((wave.heat, wave.heat_cap), (3.0, 15.0));

        let meteor = FireAbility::Meteor.definition();
        assert_eq!(
            (meteor.cooldown, meteor.radius, meteor.impact_delay),
            (18.0, 5.0, 1.25)
        );

        let firestorm = FireAbility::Firestorm.definition();
        assert_eq!((firestorm.duration, firestorm.tick_interval), (6.0, 0.5));
        assert_eq!((firestorm.heat, firestorm.heat_cap), (3.0, 30.0));

        let pyroblast = FireAbility::Pyroblast.definition();
        assert_eq!((pyroblast.cast_time, pyroblast.cooldown), (3.0, 30.0));

        let inferno = FireAbility::Inferno.definition();
        assert_eq!(
            (inferno.cooldown, inferno.radius, inferno.duration),
            (45.0, 8.0, 4.0)
        );

        let detonation = FireAbility::FireDetonation.definition();
        assert_eq!((detonation.cooldown, detonation.radius), (8.0, 10.0));
        assert_eq!(detonation.cast_time, 0.0, "detonation is instant");

        let shield = FireAbility::FlameShield.definition();
        assert_eq!((shield.cooldown, shield.duration), (30.0, 5.0));

        let blink = FireAbility::Blink.definition();
        assert_eq!((blink.cooldown, blink.range), (15.0, 10.0));

        let cataclysm = FireAbility::Cataclysm.definition();
        assert_eq!(cataclysm.energy_cost, 100.0);
        assert_eq!((cataclysm.radius, cataclysm.duration), (10.0, 8.0));
        assert_eq!((cataclysm.cast_time, cataclysm.impact_delay), (1.0, 0.5));
        assert_eq!(cataclysm.ignite_stacks, 3);
    }

    #[test]
    fn only_the_ultimate_costs_a_resource() {
        for ability in FireAbility::BAR {
            assert_eq!(
                ability.definition().energy_cost,
                0.0,
                "{ability:?} costs energy"
            );
        }
        assert_eq!(FireAbility::Cataclysm.definition().energy_cost, 100.0);
    }

    #[test]
    fn keys_run_across_the_bar_and_the_ultimate_has_its_own() {
        assert_eq!(FireAbility::Firebolt.key_hint(), "1");
        assert_eq!(FireAbility::Blink.key_hint(), "0");
        assert_eq!(FireAbility::Cataclysm.key_hint(), "R");
    }

    #[test]
    fn a_charge_is_spent_and_comes_back_after_the_cooldown() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        assert!(book.is_ready(FireAbility::Meteor));
        assert_eq!(book.charges(FireAbility::Meteor), 1);

        assert!(book.spend(FireAbility::Meteor, &effects));
        assert_eq!(book.charges(FireAbility::Meteor), 0);
        assert!(!book.is_ready(FireAbility::Meteor));
        assert!(!book.spend(FireAbility::Meteor, &effects), "spent twice");

        book.tick(17.0, 0.0, &effects);
        assert!(!book.is_ready(FireAbility::Meteor), "back early");
        book.tick(1.5, 0.0, &effects);
        assert!(book.is_ready(FireAbility::Meteor));
        assert_eq!(book.recharge_remaining(FireAbility::Meteor), 0.0);
    }

    #[test]
    fn two_charges_are_spent_one_at_a_time_and_return_one_at_a_time() {
        // Flame Lance: the reason charges are a bucket with one timer rather than a flag.
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 2);

        book.spend(FireAbility::FlameLance, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 1);
        assert!(
            book.is_ready(FireAbility::FlameLance),
            "one charge is still ready"
        );
        assert!(book.recharge_remaining(FireAbility::FlameLance) > 0.0);

        book.spend(FireAbility::FlameLance, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 0);

        book.tick(8.0, 0.0, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 1, "one came back");
        assert!(
            book.recharge_remaining(FireAbility::FlameLance) > 0.0,
            "the second is not recharging"
        );

        book.tick(8.0, 0.0, &effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 2);
        assert_eq!(book.recharge_remaining(FireAbility::FlameLance), 0.0);
    }

    #[test]
    fn the_recharge_fraction_runs_from_zero_to_one() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        assert_eq!(book.recharge_fraction(FireAbility::Meteor, &effects), 1.0);

        book.spend(FireAbility::Meteor, &effects);
        assert!(book.recharge_fraction(FireAbility::Meteor, &effects) < 0.01);

        book.tick(9.0, 0.0, &effects);
        let half = book.recharge_fraction(FireAbility::Meteor, &effects);
        assert!((half - 0.5).abs() < 0.05, "{half}");

        book.tick(9.0, 0.0, &effects);
        assert_eq!(book.recharge_fraction(FireAbility::Meteor, &effects), 1.0);
    }

    #[test]
    fn haste_shortens_cooldowns_and_casts() {
        let effects = effects();
        let mut fast = AbilityBook::new(&effects);
        fast.spend(FireAbility::Meteor, &effects);
        fast.tick(9.0, 1.0, &effects);
        assert!(
            fast.is_ready(FireAbility::Meteor),
            "double speed did not halve it"
        );

        let firebolt = FireAbility::Firebolt.definition();
        assert!((firebolt.cast_time_with(&effects, 0.0) - 1.0).abs() < 1e-6);
        assert!((firebolt.cast_time_with(&effects, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn quick_casting_shortens_firebolt_and_nothing_else() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::QuickCasting).expect("affordable");
        let effects = grid.effects();

        assert!(
            (FireAbility::Firebolt
                .definition()
                .cast_time_with(&effects, 0.0)
                - 0.8)
                .abs()
                < 1e-6
        );
        assert!(
            (FireAbility::Pyroblast
                .definition()
                .cast_time_with(&effects, 0.0)
                - 3.0)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn meteor_shower_gives_a_second_charge_and_a_longer_cooldown() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::MeteorShower).expect("affordable");
        let effects = grid.effects();

        let mut book = AbilityBook::new(&effects);
        assert_eq!(book.charges(FireAbility::Meteor), 2);
        assert!((FireAbility::Meteor.definition().cooldown_with(&effects) - 22.0).abs() < 1e-6);

        book.spend(FireAbility::Meteor, &effects);
        book.spend(FireAbility::Meteor, &effects);
        assert_eq!(book.charges(FireAbility::Meteor), 0);
        book.tick(22.0, 0.0, &effects);
        assert_eq!(book.charges(FireAbility::Meteor), 1);
    }

    #[test]
    fn accelerated_inferno_shortens_infernos_cooldown() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::AcceleratedInferno)
            .expect("affordable");
        let effects = grid.effects();
        assert!((FireAbility::Inferno.definition().cooldown_with(&effects) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn a_cooldown_reduction_can_hand_a_charge_straight_back() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        book.spend(FireAbility::Meteor, &effects);
        assert_eq!(book.charges(FireAbility::Meteor), 0);

        book.reduce_cooldown(FireAbility::Meteor, 3.0, &effects);
        assert!(book.recharge_remaining(FireAbility::Meteor) < 18.0);
        assert_eq!(
            book.charges(FireAbility::Meteor),
            0,
            "three seconds was enough"
        );

        book.reduce_cooldown(FireAbility::Meteor, 100.0, &effects);
        assert_eq!(book.charges(FireAbility::Meteor), 1);
        assert_eq!(book.recharge_remaining(FireAbility::Meteor), 0.0);
    }

    #[test]
    fn reducing_a_cooldown_that_is_not_running_banks_nothing() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        book.reduce_cooldown(FireAbility::Meteor, 100.0, &effects);
        book.spend(FireAbility::Meteor, &effects);
        assert!(
            book.recharge_remaining(FireAbility::Meteor) > 17.0,
            "the reduction was banked"
        );
    }

    #[test]
    fn a_zero_cooldown_ability_never_becomes_unready() {
        // Firebolt has no cooldown at all; spending it must not park it at zero charges with
        // no timer to bring it back.
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        for _ in 0..10 {
            assert!(book.spend(FireAbility::Firebolt, &effects));
            book.tick(0.016, 0.0, &effects);
            assert!(book.is_ready(FireAbility::Firebolt));
        }
        assert_eq!(book.recharge_remaining(FireAbility::Firebolt), 0.0);
    }

    #[test]
    fn refilling_returns_every_charge() {
        let effects = effects();
        let mut book = AbilityBook::new(&effects);
        book.spend(FireAbility::FlameLance, &effects);
        book.spend(FireAbility::Meteor, &effects);
        book.refill(&effects);
        assert_eq!(book.charges(FireAbility::FlameLance), 2);
        assert!(book.is_ready(FireAbility::Meteor));
    }
}
