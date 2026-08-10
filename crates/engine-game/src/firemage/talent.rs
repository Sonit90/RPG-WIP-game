//! The talent grid: four tiers of individually purchasable talents.
//!
//! ```text
//! TalentDef (data)  ->  TalentGrid (what is bought)  ->  TalentEffects (what it means)
//! ```
//!
//! Three layers on purpose. [`TALENTS`] is a table a designer edits. [`TalentGrid`] is the
//! player's state: points, purchases, and the rules about spending. [`TalentEffects`] is the
//! *derived* answer gameplay asks - "what is the Ignite maximum", "how much extra does a
//! detonation do" - recomputed whenever a talent is bought and never scattered as `if
//! has_talent(..)` checks through the combat code.
//!
//! That third layer is what makes Infernal Capacity work everywhere at once: the Ignite
//! maximum is a field on `TalentEffects`, and every ability, tooltip and bar reads it.
//!
//! There is no forced path. A talent in tier 4 is bought the same way as one in tier 1; the
//! tiers are a way of grouping power, not a prerequisite chain.

/// Every talent in the grid.
///
/// A dense enum rather than a string id: the compiler then checks that the effects table
/// covers all of them, and a typo cannot invent a talent that silently does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TalentId {
    // Tier 1
    /// Firebolt is likelier to add a stack to an already-burning target.
    HotHands,
    /// Each Ignite stack makes a detonation hit harder.
    BurningEmbers,
    /// Firebolt casts faster.
    QuickCasting,
    /// Ignite lasts longer.
    ExtendedFlame,
    /// Boiling over empowers the next spell.
    VolatileHeat,
    /// A critical Firebolt gives extra Ultimate Energy.
    SparkOfPower,
    // Tier 2
    /// A detonation leaves one stack behind.
    LingeringEmbers,
    /// A detonation applies an extra stack to everything it splashes.
    SpreadingFire,
    /// Pyroblast hits harder into a full burn.
    MoltenCore,
    /// Inferno comes back sooner.
    AcceleratedInferno,
    /// Meteor gains a charge and a longer cooldown.
    MeteorShower,
    /// Meteor leaves fire behind.
    BurningGround,
    // Tier 3
    /// Critical fire spells can add a stack.
    CriticalIgnition,
    /// A detonation at the cap hits harder.
    PerfectCombustion,
    /// Pyroblast guarantees the next spell's critical strike.
    Pyroclasm,
    /// Firestorm generates far more Heat.
    EndlessFurnace,
    /// Held Ultimate Energy is fire damage.
    ArcanePressure,
    /// A detonation kill brings Meteor back sooner.
    RapidCombustion,
    // Tier 4
    /// Boiling over grants haste.
    LivingInferno,
    /// Meteor into a full burn brings Pyroblast back sooner.
    MeteoricCollapse,
    /// A lethal hit is survived, followed by two seconds of invulnerability.
    PhoenixProtocol,
    /// Critical strikes extend Inferno.
    InfernoCascade,
    /// High Heat is fire damage.
    FurnaceOverdrive,
    /// Detonations feed the Ultimate, and Cataclysm makes them splash harder.
    CataclysmicIgnition,
    /// One more Ignite stack, everywhere.
    InfernalCapacity,
}

impl TalentId {
    /// Every talent, in table order.
    pub const ALL: [Self; TALENT_COUNT] = [
        Self::HotHands,
        Self::BurningEmbers,
        Self::QuickCasting,
        Self::ExtendedFlame,
        Self::VolatileHeat,
        Self::SparkOfPower,
        Self::LingeringEmbers,
        Self::SpreadingFire,
        Self::MoltenCore,
        Self::AcceleratedInferno,
        Self::MeteorShower,
        Self::BurningGround,
        Self::CriticalIgnition,
        Self::PerfectCombustion,
        Self::Pyroclasm,
        Self::EndlessFurnace,
        Self::ArcanePressure,
        Self::RapidCombustion,
        Self::LivingInferno,
        Self::MeteoricCollapse,
        Self::PhoenixProtocol,
        Self::InfernoCascade,
        Self::FurnaceOverdrive,
        Self::CataclysmicIgnition,
        Self::InfernalCapacity,
    ];

    /// Dense index, for the purchased bitset.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// This talent's definition.
    #[must_use]
    pub fn definition(self) -> &'static TalentDef {
        &TALENTS[self.index()]
    }
}

/// How many talents exist. Grows with the table; nothing restates it.
pub const TALENT_COUNT: usize = 25;

/// How many tiers the grid has.
pub const TALENT_TIERS: u8 = 4;

/// One talent, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentDef {
    /// Which talent this describes.
    pub id: TalentId,
    /// Uppercase name; the UI font has no lowercase glyphs.
    pub name: &'static str,
    /// Which tier it sits in, 1 to [`TALENT_TIERS`].
    pub tier: u8,
    /// Talent points it costs.
    pub cost: u32,
    /// What it does, in words the tooltip prints.
    pub description: &'static str,
}

/// The whole grid, in tier order.
pub static TALENTS: [TalentDef; TALENT_COUNT] = [
    TalentDef {
        id: TalentId::HotHands,
        name: "HOT HANDS",
        tier: 1,
        cost: 1,
        description: "FIREBOLT: +10% CHANCE TO APPLY IGNITE TO AN ALREADY BURNING TARGET",
    },
    TalentDef {
        id: TalentId::BurningEmbers,
        name: "BURNING EMBERS",
        tier: 1,
        cost: 1,
        description: "EACH IGNITE STACK INCREASES FIRE DETONATION DAMAGE BY 3%",
    },
    TalentDef {
        id: TalentId::QuickCasting,
        name: "QUICK CASTING",
        tier: 1,
        cost: 2,
        description: "FIREBOLT CAST TIME -20%",
    },
    TalentDef {
        id: TalentId::ExtendedFlame,
        name: "EXTENDED FLAME",
        tier: 1,
        cost: 2,
        description: "IGNITE DURATION +2 SECONDS",
    },
    TalentDef {
        id: TalentId::VolatileHeat,
        name: "VOLATILE HEAT",
        tier: 1,
        cost: 2,
        description: "AFTER REACHING 100 HEAT THE NEXT FIRE SPELL DEALS +20% DAMAGE",
    },
    TalentDef {
        id: TalentId::SparkOfPower,
        name: "SPARK OF POWER",
        tier: 1,
        cost: 2,
        description: "A CRITICAL FIREBOLT GENERATES +2 ULTIMATE ENERGY",
    },
    TalentDef {
        id: TalentId::LingeringEmbers,
        name: "LINGERING EMBERS",
        tier: 2,
        cost: 3,
        description: "AFTER FIRE DETONATION THE PRIMARY TARGET KEEPS 1 IGNITE STACK",
    },
    TalentDef {
        id: TalentId::SpreadingFire,
        name: "SPREADING FIRE",
        tier: 2,
        cost: 3,
        description: "FIRE DETONATION APPLIES +1 IGNITE STACK TO SECONDARY TARGETS",
    },
    TalentDef {
        id: TalentId::MoltenCore,
        name: "MOLTEN CORE",
        tier: 2,
        cost: 3,
        description: "PYROBLAST DEALS +15% DAMAGE AT MAXIMUM IGNITE STACKS",
    },
    TalentDef {
        id: TalentId::AcceleratedInferno,
        name: "ACCELERATED INFERNO",
        tier: 2,
        cost: 3,
        description: "INFERNO COOLDOWN -5 SECONDS",
    },
    TalentDef {
        id: TalentId::MeteorShower,
        name: "METEOR SHOWER",
        tier: 2,
        cost: 4,
        description: "METEOR GAINS 2 CHARGES. COOLDOWN BECOMES 22 SECONDS",
    },
    TalentDef {
        id: TalentId::BurningGround,
        name: "BURNING GROUND",
        tier: 2,
        cost: 3,
        description: "METEOR LEAVES A BURNING AREA FOR 4 SECONDS, RADIUS 5 M",
    },
    TalentDef {
        id: TalentId::CriticalIgnition,
        name: "CRITICAL IGNITION",
        tier: 3,
        cost: 4,
        description: "CRITICAL FIRE SPELLS HAVE A 30% CHANCE TO APPLY +1 IGNITE STACK",
    },
    TalentDef {
        id: TalentId::PerfectCombustion,
        name: "PERFECT COMBUSTION",
        tier: 3,
        cost: 5,
        description: "FIRE DETONATION AT MAXIMUM IGNITE STACKS DEALS +25% DAMAGE",
    },
    TalentDef {
        id: TalentId::Pyroclasm,
        name: "PYROCLASM",
        tier: 3,
        cost: 4,
        description: "AFTER PYROBLAST THE NEXT FIRE SPELL ALWAYS CRITS. 10 S INTERNAL CD",
    },
    TalentDef {
        id: TalentId::EndlessFurnace,
        name: "ENDLESS FURNACE",
        tier: 3,
        cost: 5,
        description: "FIRESTORM GENERATES +50% MORE HEAT",
    },
    TalentDef {
        id: TalentId::ArcanePressure,
        name: "ARCANE PRESSURE",
        tier: 3,
        cost: 4,
        description: "EVERY 20 ULTIMATE ENERGY GRANTS +2% FIRE DAMAGE, UP TO +10%",
    },
    TalentDef {
        id: TalentId::RapidCombustion,
        name: "RAPID COMBUSTION",
        tier: 3,
        cost: 5,
        description: "A FIRE DETONATION KILL REDUCES METEOR COOLDOWN BY 3 SECONDS",
    },
    TalentDef {
        id: TalentId::LivingInferno,
        name: "LIVING INFERNO",
        tier: 4,
        cost: 6,
        description: "REACHING 100 HEAT GRANTS +15% HASTE FOR 5 SECONDS",
    },
    TalentDef {
        id: TalentId::MeteoricCollapse,
        name: "METEORIC COLLAPSE",
        tier: 4,
        cost: 6,
        description: "METEOR HITTING A MAX STACK TARGET REDUCES PYROBLAST COOLDOWN BY 5 S",
    },
    TalentDef {
        id: TalentId::PhoenixProtocol,
        name: "PHOENIX PROTOCOL",
        tier: 4,
        cost: 7,
        description: "SURVIVE LETHAL DAMAGE AT 1 HP AND BECOME INVULNERABLE 2 S. 180 S CD",
    },
    TalentDef {
        id: TalentId::InfernoCascade,
        name: "INFERNO CASCADE",
        tier: 4,
        cost: 6,
        description: "CRITS DURING INFERNO EXTEND IT BY 0.5 S, UP TO 3 S",
    },
    TalentDef {
        id: TalentId::FurnaceOverdrive,
        name: "FURNACE OVERDRIVE",
        tier: 4,
        cost: 7,
        description: "+10% FIRE DAMAGE AT 80 HEAT, +20% AT 100 HEAT",
    },
    TalentDef {
        id: TalentId::CataclysmicIgnition,
        name: "CATACLYSMIC IGNITION",
        tier: 4,
        cost: 8,
        description: "DETONATIONS GIVE +2 ENERGY. DURING CATACLYSM THEY SPLASH +25%",
    },
    TalentDef {
        id: TalentId::InfernalCapacity,
        name: "INFERNAL CAPACITY",
        tier: 4,
        cost: 6,
        description: "MAXIMUM IGNITE STACKS +1. EVERY ABILITY AND TOOLTIP FOLLOWS",
    },
];

/// Whether the build hands out enough points to buy everything.
///
/// A testing convenience, not a progression change: with it off the class starts at zero
/// points and earns them. It is a `const` rather than a build feature so a test can assert
/// both paths without a second compilation.
pub const TESTING_MODE: bool = true;

/// Points needed to own every talent in the table.
///
/// Computed from the table, never written down: adding a talent raises it automatically,
/// which is the whole point of testing mode being derived rather than a magic number.
#[must_use]
pub fn total_talent_cost() -> u32 {
    TALENTS.iter().map(|talent| talent.cost).sum()
}

/// How many points a new FireMage starts with.
#[must_use]
pub fn starting_talent_points() -> u32 {
    if TESTING_MODE { total_talent_cost() } else { 0 }
}

/// Why a talent could not be bought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalentError {
    /// It is already owned.
    AlreadyOwned,
    /// There are not enough points.
    NotEnoughPoints,
}

impl TalentError {
    /// Short uppercase description, for the UI font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyOwned => "ALREADY LEARNED",
            Self::NotEnoughPoints => "NOT ENOUGH POINTS",
        }
    }
}

/// What the player has bought, and what they have left to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentGrid {
    /// One bit per talent. 25 talents fit in a `u32` with room to grow to 32.
    purchased: u32,
    available: u32,
    spent: u32,
}

impl Default for TalentGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl TalentGrid {
    /// A grid with the starting points and nothing bought.
    #[must_use]
    pub fn new() -> Self {
        Self {
            purchased: 0,
            available: starting_talent_points(),
            spent: 0,
        }
    }

    /// A grid with an explicit number of points, for tests and for progression.
    #[must_use]
    pub fn with_points(points: u32) -> Self {
        Self {
            purchased: 0,
            available: points,
            spent: 0,
        }
    }

    /// Points left to spend.
    #[must_use]
    pub fn available_points(&self) -> u32 {
        self.available
    }

    /// Points already spent.
    #[must_use]
    pub fn spent_points(&self) -> u32 {
        self.spent
    }

    /// Grants points, as progression would.
    pub fn grant_points(&mut self, points: u32) {
        self.available = self.available.saturating_add(points);
    }

    /// Whether `talent` is owned.
    #[must_use]
    pub fn has(&self, talent: TalentId) -> bool {
        self.purchased & (1 << talent.index()) != 0
    }

    /// Whether `talent` could be bought right now.
    #[must_use]
    pub fn can_purchase(&self, talent: TalentId) -> bool {
        self.purchase_error(talent).is_none()
    }

    /// Why `talent` cannot be bought, or `None` if it can.
    #[must_use]
    pub fn purchase_error(&self, talent: TalentId) -> Option<TalentError> {
        if self.has(talent) {
            return Some(TalentError::AlreadyOwned);
        }
        if self.available < talent.definition().cost {
            return Some(TalentError::NotEnoughPoints);
        }
        None
    }

    /// Buys `talent`, spending its cost.
    ///
    /// # Errors
    ///
    /// Returns [`TalentError`] if it is owned already or cannot be afforded.
    pub fn purchase(&mut self, talent: TalentId) -> Result<(), TalentError> {
        if let Some(error) = self.purchase_error(talent) {
            return Err(error);
        }
        let cost = talent.definition().cost;
        self.available -= cost;
        self.spent += cost;
        self.purchased |= 1 << talent.index();
        Ok(())
    }

    /// How many talents are owned.
    #[must_use]
    pub fn purchased_count(&self) -> u32 {
        self.purchased.count_ones()
    }

    /// Every owned talent.
    pub fn purchased(&self) -> impl Iterator<Item = TalentId> + '_ {
        TalentId::ALL.into_iter().filter(|id| self.has(*id))
    }

    /// Refunds everything, returning the grid to a fresh build.
    pub fn reset(&mut self) {
        self.available += self.spent;
        self.spent = 0;
        self.purchased = 0;
    }

    /// What the owned talents mean for gameplay.
    #[must_use]
    pub fn effects(&self) -> TalentEffects {
        TalentEffects::from_grid(self)
    }
}

/// Every talent's effect, resolved into the numbers gameplay actually uses.
///
/// Derived state, rebuilt whenever the grid changes. Combat code reads a field here rather
/// than asking "do I have talent X" - so a new talent that changes an existing number is a
/// line in [`TalentEffects::from_grid`] and no change at all where the number is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentEffects {
    /// Maximum Ignite stacks. The number the whole game asks for.
    pub ignite_max_stacks: u8,
    /// Seconds added to Ignite's duration.
    pub ignite_duration_bonus: f32,
    /// Extra chance for Firebolt to stack Ignite on a burning target.
    pub firebolt_extra_ignite_chance: f32,
    /// Multiplier on Firebolt's cast time.
    pub firebolt_cast_scale: f32,
    /// Extra detonation damage per Ignite stack consumed.
    pub detonation_damage_per_stack: f32,
    /// Extra detonation damage when it was cast at the maximum.
    pub detonation_max_stack_bonus: f32,
    /// Extra Ignite stacks a detonation applies to secondary targets.
    pub detonation_extra_secondary_stacks: u8,
    /// Ignite stacks the primary target keeps after a detonation.
    pub detonation_retained_stacks: u8,
    /// Extra Ultimate Energy per detonation.
    pub detonation_bonus_energy: f32,
    /// Extra splash damage a detonation deals while Cataclysm is running.
    pub detonation_cataclysm_splash: f32,
    /// Seconds Meteor's cooldown drops by when a detonation kills.
    pub detonation_kill_meteor_refund: f32,
    /// Extra Pyroblast damage against a target at the maximum.
    pub pyroblast_max_stack_bonus: f32,
    /// Whether Pyroblast guarantees the next spell's critical strike.
    pub pyroclasm: bool,
    /// Seconds off Inferno's cooldown.
    pub inferno_cooldown_reduction: f32,
    /// Seconds a critical strike adds to Inferno, and the most it can add.
    pub inferno_crit_extension: f32,
    /// The most Inferno's duration can be extended in one cast.
    pub inferno_max_extension: f32,
    /// Extra Meteor charges.
    pub meteor_extra_charges: u8,
    /// Meteor's cooldown when it has the extra charge, or zero for the default.
    pub meteor_cooldown_override: f32,
    /// Seconds of burning ground Meteor leaves, or zero for none.
    pub meteor_ground_duration: f32,
    /// Seconds off Pyroblast when Meteor hits a target at the maximum.
    pub meteor_max_stack_pyroblast_refund: f32,
    /// Multiplier on Firestorm's Heat generation.
    pub firestorm_heat_scale: f32,
    /// Chance a critical fire spell applies an extra Ignite stack.
    pub critical_ignition_chance: f32,
    /// Extra Ultimate Energy from a critical Firebolt.
    pub crit_firebolt_bonus_energy: f32,
    /// Damage bonus the next spell gets after boiling over.
    pub volatile_heat_bonus: f32,
    /// Haste granted for five seconds on boiling over.
    pub living_inferno_haste: f32,
    /// Fire damage per 20 Ultimate Energy held, and its cap.
    pub arcane_pressure_per_20: f32,
    /// The most Arcane Pressure can grant.
    pub arcane_pressure_cap: f32,
    /// Fire damage bonus at high Heat, and at the cap.
    pub furnace_overdrive_high: f32,
    /// Fire damage bonus at maximum Heat.
    pub furnace_overdrive_max: f32,
    /// Whether a lethal hit is survived.
    pub phoenix_protocol: bool,
}

impl Default for TalentEffects {
    fn default() -> Self {
        Self::from_grid(&TalentGrid::with_points(0))
    }
}

impl TalentEffects {
    /// Resolves a grid into the numbers gameplay uses.
    #[must_use]
    pub fn from_grid(grid: &TalentGrid) -> Self {
        let has = |talent| grid.has(talent);
        Self {
            ignite_max_stacks: super::ignite::IGNITE_BASE_MAX_STACKS
                + u8::from(has(TalentId::InfernalCapacity)),
            ignite_duration_bonus: if has(TalentId::ExtendedFlame) {
                2.0
            } else {
                0.0
            },
            firebolt_extra_ignite_chance: if has(TalentId::HotHands) { 0.10 } else { 0.0 },
            firebolt_cast_scale: if has(TalentId::QuickCasting) {
                0.8
            } else {
                1.0
            },
            detonation_damage_per_stack: if has(TalentId::BurningEmbers) {
                0.03
            } else {
                0.0
            },
            detonation_max_stack_bonus: if has(TalentId::PerfectCombustion) {
                0.25
            } else {
                0.0
            },
            detonation_extra_secondary_stacks: u8::from(has(TalentId::SpreadingFire)),
            detonation_retained_stacks: u8::from(has(TalentId::LingeringEmbers)),
            detonation_bonus_energy: if has(TalentId::CataclysmicIgnition) {
                2.0
            } else {
                0.0
            },
            detonation_cataclysm_splash: if has(TalentId::CataclysmicIgnition) {
                0.25
            } else {
                0.0
            },
            detonation_kill_meteor_refund: if has(TalentId::RapidCombustion) {
                3.0
            } else {
                0.0
            },
            pyroblast_max_stack_bonus: if has(TalentId::MoltenCore) { 0.15 } else { 0.0 },
            pyroclasm: has(TalentId::Pyroclasm),
            inferno_cooldown_reduction: if has(TalentId::AcceleratedInferno) {
                5.0
            } else {
                0.0
            },
            inferno_crit_extension: if has(TalentId::InfernoCascade) {
                0.5
            } else {
                0.0
            },
            inferno_max_extension: if has(TalentId::InfernoCascade) {
                3.0
            } else {
                0.0
            },
            meteor_extra_charges: u8::from(has(TalentId::MeteorShower)),
            meteor_cooldown_override: if has(TalentId::MeteorShower) {
                22.0
            } else {
                0.0
            },
            meteor_ground_duration: if has(TalentId::BurningGround) {
                4.0
            } else {
                0.0
            },
            meteor_max_stack_pyroblast_refund: if has(TalentId::MeteoricCollapse) {
                5.0
            } else {
                0.0
            },
            firestorm_heat_scale: if has(TalentId::EndlessFurnace) {
                1.5
            } else {
                1.0
            },
            critical_ignition_chance: if has(TalentId::CriticalIgnition) {
                0.30
            } else {
                0.0
            },
            crit_firebolt_bonus_energy: if has(TalentId::SparkOfPower) {
                2.0
            } else {
                0.0
            },
            volatile_heat_bonus: if has(TalentId::VolatileHeat) {
                0.20
            } else {
                0.0
            },
            living_inferno_haste: if has(TalentId::LivingInferno) {
                0.15
            } else {
                0.0
            },
            arcane_pressure_per_20: if has(TalentId::ArcanePressure) {
                0.02
            } else {
                0.0
            },
            arcane_pressure_cap: if has(TalentId::ArcanePressure) {
                0.10
            } else {
                0.0
            },
            furnace_overdrive_high: if has(TalentId::FurnaceOverdrive) {
                0.10
            } else {
                0.0
            },
            furnace_overdrive_max: if has(TalentId::FurnaceOverdrive) {
                0.20
            } else {
                0.0
            },
            phoenix_protocol: has(TalentId::PhoenixProtocol),
        }
    }

    /// Ignite's duration with talents applied.
    #[must_use]
    pub fn ignite_duration(&self) -> f32 {
        super::ignite::IGNITE_BASE_DURATION + self.ignite_duration_bonus
    }

    /// The fire damage bonus Arcane Pressure grants at `energy` Ultimate Energy.
    #[must_use]
    pub fn arcane_pressure_bonus(&self, energy: f32) -> f32 {
        if self.arcane_pressure_per_20 <= 0.0 {
            return 0.0;
        }
        let steps = (energy / 20.0).floor();
        (steps * self.arcane_pressure_per_20).min(self.arcane_pressure_cap)
    }

    /// The fire damage bonus Furnace Overdrive grants at `heat` Heat.
    #[must_use]
    pub fn furnace_overdrive_bonus(&self, heat: f32) -> f32 {
        if self.furnace_overdrive_high <= 0.0 {
            return 0.0;
        }
        if heat >= super::resource::HEAT_OVERHEAT_THRESHOLD {
            self.furnace_overdrive_max
        } else if heat >= super::resource::HEAT_HIGH_THRESHOLD {
            self.furnace_overdrive_high
        } else {
            0.0
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn every_talent_has_a_definition_in_its_own_slot() {
        // The table is indexed by the enum; a reordering that broke that would silently
        // give every talent its neighbour's cost.
        for (index, talent) in TalentId::ALL.into_iter().enumerate() {
            assert_eq!(talent.index(), index, "{talent:?} is out of order");
            assert_eq!(
                talent.definition().id,
                talent,
                "{talent:?} has the wrong def"
            );
        }
        assert_eq!(TALENTS.len(), TALENT_COUNT);
    }

    #[test]
    fn every_talent_is_described_and_priced() {
        for talent in &TALENTS {
            assert!(!talent.name.is_empty(), "{:?} has no name", talent.id);
            assert!(
                !talent.description.is_empty(),
                "{:?} has no description",
                talent.id
            );
            assert!(talent.cost > 0, "{:?} is free", talent.id);
            assert!(
                (1..=TALENT_TIERS).contains(&talent.tier),
                "{:?} is in tier {}",
                talent.id,
                talent.tier
            );
        }
    }

    #[test]
    fn the_grid_covers_all_four_tiers() {
        for tier in 1..=TALENT_TIERS {
            assert!(
                TALENTS.iter().any(|talent| talent.tier == tier),
                "tier {tier} is empty"
            );
        }
    }

    #[test]
    fn testing_mode_grants_exactly_what_every_talent_costs() {
        // Derived, never written down: a new talent raises this automatically.
        let expected: u32 = TALENTS.iter().map(|talent| talent.cost).sum();
        assert_eq!(total_talent_cost(), expected);

        if TESTING_MODE {
            assert_eq!(starting_talent_points(), expected);
        } else {
            assert_eq!(starting_talent_points(), 0);
        }
    }

    #[test]
    fn a_testing_grid_can_buy_the_entire_tree_and_ends_at_zero() {
        let mut grid = TalentGrid::with_points(total_talent_cost());
        for talent in TalentId::ALL {
            grid.purchase(talent)
                .unwrap_or_else(|error| panic!("{talent:?}: {error:?}"));
        }
        assert_eq!(grid.purchased_count(), TALENT_COUNT as u32);
        assert_eq!(grid.available_points(), 0, "the budget was wrong");
        assert_eq!(grid.spent_points(), total_talent_cost());
    }

    #[test]
    fn a_fresh_progression_grid_can_buy_nothing() {
        let grid = TalentGrid::with_points(0);
        for talent in TalentId::ALL {
            assert!(!grid.can_purchase(talent), "{talent:?} was free");
            assert_eq!(
                grid.purchase_error(talent),
                Some(TalentError::NotEnoughPoints)
            );
        }
    }

    #[test]
    fn talents_can_be_bought_in_any_order_rather_than_along_a_path() {
        // No prerequisites: a tier 4 talent is reachable first if it is paid for.
        let mut grid = TalentGrid::with_points(20);
        assert!(grid.purchase(TalentId::PhoenixProtocol).is_ok());
        assert!(grid.has(TalentId::PhoenixProtocol));
        assert!(grid.purchase(TalentId::HotHands).is_ok());
        assert_eq!(grid.purchased_count(), 2);
    }

    #[test]
    fn a_talent_cannot_be_bought_twice_or_without_the_points() {
        let mut grid = TalentGrid::with_points(1);
        assert!(grid.purchase(TalentId::HotHands).is_ok());
        assert_eq!(
            grid.purchase(TalentId::HotHands),
            Err(TalentError::AlreadyOwned)
        );
        assert_eq!(
            grid.purchase(TalentId::QuickCasting),
            Err(TalentError::NotEnoughPoints)
        );
        assert_eq!(grid.available_points(), 0);
    }

    #[test]
    fn resetting_gives_every_point_back() {
        let mut grid = TalentGrid::with_points(total_talent_cost());
        for talent in TalentId::ALL {
            let _ = grid.purchase(talent);
        }
        grid.reset();
        assert_eq!(grid.purchased_count(), 0);
        assert_eq!(grid.available_points(), total_talent_cost());
    }

    #[test]
    fn the_default_ignite_maximum_is_five_and_infernal_capacity_makes_it_six() {
        // The property section 18 of the brief is about, at the source.
        let plain = TalentGrid::with_points(0).effects();
        assert_eq!(plain.ignite_max_stacks, 5);

        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::InfernalCapacity)
            .expect("the points are there");
        assert_eq!(grid.effects().ignite_max_stacks, 6);
    }

    #[test]
    fn effects_are_derived_from_what_is_owned() {
        let mut grid = TalentGrid::with_points(100);
        let before = grid.effects();
        assert_eq!(before.firebolt_cast_scale, 1.0);
        assert_eq!(before.firebolt_extra_ignite_chance, 0.0);

        grid.purchase(TalentId::QuickCasting).expect("affordable");
        grid.purchase(TalentId::HotHands).expect("affordable");
        let after = grid.effects();
        assert!((after.firebolt_cast_scale - 0.8).abs() < 1e-6);
        assert!((after.firebolt_extra_ignite_chance - 0.10).abs() < 1e-6);
        assert_eq!(after.ignite_max_stacks, 5, "an unrelated talent changed it");
    }

    #[test]
    fn extended_flame_lengthens_the_burn() {
        let mut grid = TalentGrid::with_points(100);
        assert_eq!(
            grid.effects().ignite_duration(),
            super::super::ignite::IGNITE_BASE_DURATION
        );
        grid.purchase(TalentId::ExtendedFlame).expect("affordable");
        assert_eq!(
            grid.effects().ignite_duration(),
            super::super::ignite::IGNITE_BASE_DURATION + 2.0
        );
    }

    #[test]
    fn arcane_pressure_steps_every_twenty_energy_and_caps() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::ArcanePressure).expect("affordable");
        let effects = grid.effects();

        assert_eq!(effects.arcane_pressure_bonus(0.0), 0.0);
        assert_eq!(effects.arcane_pressure_bonus(19.0), 0.0);
        assert!((effects.arcane_pressure_bonus(20.0) - 0.02).abs() < 1e-6);
        assert!((effects.arcane_pressure_bonus(80.0) - 0.08).abs() < 1e-6);
        assert!((effects.arcane_pressure_bonus(100.0) - 0.10).abs() < 1e-6);
        assert!(
            (effects.arcane_pressure_bonus(1_000.0) - 0.10).abs() < 1e-6,
            "the cap leaked"
        );

        // Without the talent it is worth nothing at any amount.
        let none = TalentGrid::with_points(0).effects();
        assert_eq!(none.arcane_pressure_bonus(100.0), 0.0);
    }

    #[test]
    fn furnace_overdrive_has_two_steps() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::FurnaceOverdrive)
            .expect("affordable");
        let effects = grid.effects();

        assert_eq!(effects.furnace_overdrive_bonus(50.0), 0.0);
        assert!((effects.furnace_overdrive_bonus(80.0) - 0.10).abs() < 1e-6);
        assert!((effects.furnace_overdrive_bonus(99.0) - 0.10).abs() < 1e-6);
        assert!((effects.furnace_overdrive_bonus(100.0) - 0.20).abs() < 1e-6);
    }

    #[test]
    fn meteor_shower_replaces_the_cooldown_as_well_as_adding_a_charge() {
        let mut grid = TalentGrid::with_points(100);
        grid.purchase(TalentId::MeteorShower).expect("affordable");
        let effects = grid.effects();
        assert_eq!(effects.meteor_extra_charges, 1);
        assert!((effects.meteor_cooldown_override - 22.0).abs() < 1e-6);
    }
}
