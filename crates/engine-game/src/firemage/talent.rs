//! The talent tree: twelve talents, bought individually against a point budget.
//!
//! ```text
//! TalentDef (data)  ->  TalentGrid (what is bought)  ->  TalentEffects (what it means)
//! ```
//!
//! Three layers on purpose. [`TALENTS`] is a table a designer edits. [`TalentGrid`] is the
//! player's state: points, purchases, and the rules about spending. [`TalentEffects`] is the
//! *derived* answer gameplay asks - "what is the Ignite maximum", "how much extra does a
//! detonation do" - recomputed whenever a talent is bought and never scattered as
//! `if has_talent(..)` checks through the combat code.
//!
//! That third layer is what makes Infernal Capacity work everywhere at once: the Ignite
//! maximum is a field on [`TalentEffects`], and every ability, tooltip, bar and simulator
//! run reads it.
//!
//! The same table is what the combat simulator enumerates. There is exactly one definition
//! of a talent in the project.

/// Every talent in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TalentId {
    /// Each Ignite stack consumed makes Detonate hit harder.
    BurningEmbers,
    /// Ignite lasts ten seconds longer.
    ExtendedFlame,
    /// One more Ignite stack, everywhere.
    InfernalCapacity,
    /// Detonate can hurry the area cooldowns along.
    RapidCombustion,
    /// Meteor trades its impact for burning ground.
    MoltenMeteor,
    /// The Inferno State lasts two seconds longer.
    FurnaceMastery,
    /// Meteor crits for triple, and a critical Meteor hurries Pyroblast.
    MeteoricImpact,
    /// The critical strike loop between Flame Lance and Pyroblast, at its cap.
    SpellfireFeedback,
    /// A critical Ignite tick can hurry Meteor along.
    BurningPrecision,
    /// Weaker Ignite, a second Pyroblast.
    VolatileIgnition,
    /// Pyroblast bounces.
    ChainPyroblast,
    /// Fire Mastery grants both stats instead of one.
    DualFireMastery,
}

impl TalentId {
    /// Every talent, in table order.
    pub const ALL: [Self; TALENT_COUNT] = [
        Self::BurningEmbers,
        Self::ExtendedFlame,
        Self::InfernalCapacity,
        Self::RapidCombustion,
        Self::MoltenMeteor,
        Self::FurnaceMastery,
        Self::MeteoricImpact,
        Self::SpellfireFeedback,
        Self::BurningPrecision,
        Self::VolatileIgnition,
        Self::ChainPyroblast,
        Self::DualFireMastery,
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
pub const TALENT_COUNT: usize = 12;

/// How many tiers the menu groups them into.
pub const TALENT_TIERS: u8 = 4;

/// Talent points a build is given by default.
///
/// Enough for four or five talents out of twelve, which is what makes the tree a set of
/// decisions rather than a checklist. The simulator's combination search uses the same
/// number unless it is told otherwise.
pub const DEFAULT_TALENT_BUDGET: u32 = 20;

/// Whether the build hands out enough points to buy everything.
///
/// A testing convenience, not a progression change. It is a `const` rather than a build
/// feature so a test can assert both paths without a second compilation.
pub const TESTING_MODE: bool = true;

/// One talent, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentDef {
    /// Which talent this describes.
    pub id: TalentId,
    /// Uppercase name; the UI font has no lowercase glyphs.
    pub name: &'static str,
    /// Which tier it sits in, 1 to [`TALENT_TIERS`]. Grouping for the menu, not a gate.
    pub tier: u8,
    /// Talent points it costs.
    pub cost: u32,
    /// What it does, in words the tooltip prints.
    pub description: &'static str,
}

/// The whole tree, grouped by cost into tiers.
pub static TALENTS: [TalentDef; TALENT_COUNT] = [
    TalentDef {
        id: TalentId::BurningEmbers,
        name: "BURNING EMBERS",
        tier: 1,
        cost: 3,
        description: "EACH IGNITE STACK CONSUMED INCREASES DETONATE DAMAGE BY 3%",
    },
    TalentDef {
        id: TalentId::ExtendedFlame,
        name: "EXTENDED FLAME",
        tier: 1,
        cost: 2,
        description: "IGNITE DURATION 30 -> 40 SECONDS",
    },
    TalentDef {
        id: TalentId::InfernalCapacity,
        name: "INFERNAL CAPACITY",
        tier: 3,
        cost: 5,
        description: "MAXIMUM IGNITE 5 -> 6. DETONATE THEN REQUIRES 6",
    },
    TalentDef {
        id: TalentId::RapidCombustion,
        name: "RAPID COMBUSTION",
        tier: 2,
        cost: 4,
        description: "DETONATE: 20% CHANCE TO CUT FLAME CONE, FIRESTORM AND METEOR BY 2 S",
    },
    TalentDef {
        id: TalentId::MoltenMeteor,
        name: "MOLTEN METEOR",
        tier: 2,
        cost: 4,
        description: "METEOR -25% IMPACT AND CANNOT CRIT. LEAVES BURNING GROUND FOR 15 S",
    },
    TalentDef {
        id: TalentId::FurnaceMastery,
        name: "FURNACE MASTERY",
        tier: 3,
        cost: 5,
        description: "THE INFERNO STATE LASTS 12 SECONDS INSTEAD OF 10",
    },
    TalentDef {
        id: TalentId::MeteoricImpact,
        name: "METEORIC IMPACT",
        tier: 2,
        cost: 4,
        description: "METEOR +100% CRIT DAMAGE. A CRITICAL METEOR CUTS PYROBLAST BY 4 S",
    },
    TalentDef {
        id: TalentId::SpellfireFeedback,
        name: "SPELLFIRE FEEDBACK",
        tier: 2,
        cost: 4,
        description: "CRIT FLAME LANCE AND CRIT PYROBLAST CUT EACH OTHER BY 1.5 S",
    },
    TalentDef {
        id: TalentId::BurningPrecision,
        name: "BURNING PRECISION",
        tier: 1,
        cost: 3,
        description: "A CRITICAL IGNITE TICK HAS A 10% CHANCE TO CUT METEOR BY 1 S",
    },
    TalentDef {
        id: TalentId::VolatileIgnition,
        name: "VOLATILE IGNITION",
        tier: 3,
        cost: 5,
        description: "IGNITE DAMAGE -20%. PYROBLAST GAINS A SECOND CHARGE",
    },
    TalentDef {
        id: TalentId::ChainPyroblast,
        name: "CHAIN PYROBLAST",
        tier: 3,
        cost: 5,
        description: "PYROBLAST BOUNCES AT YOUR CRIT CHANCE. -20% A BOUNCE, 10 MAXIMUM",
    },
    TalentDef {
        id: TalentId::DualFireMastery,
        name: "DUAL FIRE MASTERY",
        tier: 4,
        cost: 7,
        description: "FIRE MASTERY GRANTS +10% HASTE AND +10% CRIT INSTEAD OF ONE STAT",
    },
];

/// Points needed to own every talent in the table.
///
/// Computed from the table, never written down: adding a talent raises it automatically.
#[must_use]
pub fn total_talent_cost() -> u32 {
    TALENTS.iter().map(|talent| talent.cost).sum()
}

/// How many points a new FireMage starts with.
#[must_use]
pub fn starting_talent_points() -> u32 {
    if TESTING_MODE {
        total_talent_cost()
    } else {
        DEFAULT_TALENT_BUDGET
    }
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
    /// One bit per talent.
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
        Self::with_points(starting_talent_points())
    }

    /// A grid with an explicit number of points, for tests, builds and the simulator.
    #[must_use]
    pub fn with_points(points: u32) -> Self {
        Self {
            purchased: 0,
            available: points,
            spent: 0,
        }
    }

    /// A grid holding exactly `talents`, ignoring the budget.
    ///
    /// What the simulator's combination search builds: it has already decided the set fits.
    #[must_use]
    pub fn from_talents(talents: &[TalentId]) -> Self {
        let mut grid = Self::with_points(total_talent_cost());
        for talent in talents {
            let _ = grid.purchase(*talent);
        }
        grid
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

/// Damage a Molten Meteor's burning ground deals per tick, as a fraction of spell power.
///
/// Balanced by the simulator: it is the number that decides whether trading Meteor's impact
/// for fifteen seconds of ground is worth four points.
pub const MOLTEN_GROUND_TICK_COEFFICIENT: f32 = 0.55;

/// Seconds of burning ground Molten Meteor leaves.
pub const MOLTEN_GROUND_DURATION: f32 = 15.0;

/// Seconds between burning ground ticks.
pub const MOLTEN_GROUND_INTERVAL: f32 = 3.0;

/// Extra fire damage taken by whatever stands in a Molten Meteor's ground.
pub const MOLTEN_GROUND_VULNERABILITY: f32 = 0.03;

/// Every talent's effect, resolved into the numbers gameplay actually uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentEffects {
    /// Maximum Ignite stacks. The number the whole game asks for.
    pub ignite_max_stacks: u8,
    /// Seconds added to Ignite's duration.
    pub ignite_duration_bonus: f32,
    /// Multiplier on Ignite's damage. Below one for Volatile Ignition.
    pub ignite_damage_scale: f32,
    /// Extra Detonate damage per Ignite stack consumed.
    pub detonate_damage_per_stack: f32,
    /// Chance a Detonate cuts each area cooldown, and by how much.
    pub detonate_cooldown_chance: f32,
    /// Seconds a Detonate cuts an area cooldown by when the chance lands.
    pub detonate_cooldown_reduction: f32,
    /// Seconds added to the Inferno State.
    pub inferno_duration_bonus: f32,
    /// Extra critical strike damage on Meteor. `1.0` turns x2 into x3.
    pub meteor_crit_damage_bonus: f32,
    /// Seconds a critical Meteor cuts Pyroblast by.
    pub meteor_crit_pyroblast_reduction: f32,
    /// Multiplier on Meteor's impact damage.
    pub meteor_damage_scale: f32,
    /// Whether Meteor is forbidden from critically striking.
    pub meteor_cannot_crit: bool,
    /// Seconds of burning ground Meteor leaves, or zero for none.
    pub meteor_ground_duration: f32,
    /// Seconds a critical Flame Lance cuts Pyroblast by.
    pub flame_lance_crit_reduction: f32,
    /// Seconds a critical Pyroblast cuts Flame Lance by.
    pub pyroblast_crit_reduction: f32,
    /// Chance a critical Ignite tick cuts Meteor's cooldown.
    pub ignite_crit_meteor_chance: f32,
    /// Seconds it cuts Meteor by when it does.
    pub ignite_crit_meteor_reduction: f32,
    /// Extra Pyroblast charges.
    pub pyroblast_extra_charges: u8,
    /// Whether Pyroblast bounces.
    pub pyroblast_bounces: bool,
    /// Damage lost per bounce.
    pub pyroblast_bounce_falloff: f32,
    /// The most times one Pyroblast may bounce.
    pub pyroblast_max_bounces: u8,
    /// Whether Fire Mastery grants both stats.
    pub dual_mastery: bool,
    /// What Fire Mastery's active grants, per stat.
    pub mastery_active_bonus: f32,
}

/// What Fire Mastery's active grants when it specialises into one stat.
pub const MASTERY_ACTIVE_SINGLE: f32 = 0.15;

/// What it grants per stat when Dual Fire Mastery splits it across both.
pub const MASTERY_ACTIVE_DUAL: f32 = 0.10;

/// Seconds a critical Flame Lance or Pyroblast cuts the other by, before talents.
pub const CRIT_FEEDBACK_BASE: f32 = 1.0;

/// The most a critical Flame Lance or Pyroblast may ever cut the other by.
///
/// A hard ceiling, not a talent value. Nothing in the class is allowed to exceed it, which
/// is asserted by a test rather than trusted.
pub const CRIT_FEEDBACK_CAP: f32 = 1.5;

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
        let feedback = if has(TalentId::SpellfireFeedback) {
            CRIT_FEEDBACK_CAP
        } else {
            CRIT_FEEDBACK_BASE
        }
        .min(CRIT_FEEDBACK_CAP);

        Self {
            ignite_max_stacks: super::ignite::IGNITE_BASE_MAX_STACKS
                + u8::from(has(TalentId::InfernalCapacity)),
            ignite_duration_bonus: if has(TalentId::ExtendedFlame) {
                10.0
            } else {
                0.0
            },
            ignite_damage_scale: if has(TalentId::VolatileIgnition) {
                0.80
            } else {
                1.0
            },
            detonate_damage_per_stack: if has(TalentId::BurningEmbers) {
                0.03
            } else {
                0.0
            },
            detonate_cooldown_chance: if has(TalentId::RapidCombustion) {
                0.20
            } else {
                0.0
            },
            detonate_cooldown_reduction: if has(TalentId::RapidCombustion) {
                2.0
            } else {
                0.0
            },
            inferno_duration_bonus: if has(TalentId::FurnaceMastery) {
                2.0
            } else {
                0.0
            },
            meteor_crit_damage_bonus: if has(TalentId::MeteoricImpact) {
                1.0
            } else {
                0.0
            },
            meteor_crit_pyroblast_reduction: if has(TalentId::MeteoricImpact) {
                4.0
            } else {
                0.0
            },
            meteor_damage_scale: if has(TalentId::MoltenMeteor) {
                0.75
            } else {
                1.0
            },
            meteor_cannot_crit: has(TalentId::MoltenMeteor),
            meteor_ground_duration: if has(TalentId::MoltenMeteor) {
                MOLTEN_GROUND_DURATION
            } else {
                0.0
            },
            flame_lance_crit_reduction: feedback,
            pyroblast_crit_reduction: feedback,
            ignite_crit_meteor_chance: if has(TalentId::BurningPrecision) {
                0.10
            } else {
                0.0
            },
            ignite_crit_meteor_reduction: if has(TalentId::BurningPrecision) {
                1.0
            } else {
                0.0
            },
            pyroblast_extra_charges: u8::from(has(TalentId::VolatileIgnition)),
            pyroblast_bounces: has(TalentId::ChainPyroblast),
            pyroblast_bounce_falloff: if has(TalentId::ChainPyroblast) {
                0.20
            } else {
                0.0
            },
            pyroblast_max_bounces: if has(TalentId::ChainPyroblast) { 10 } else { 0 },
            dual_mastery: has(TalentId::DualFireMastery),
            mastery_active_bonus: if has(TalentId::DualFireMastery) {
                MASTERY_ACTIVE_DUAL
            } else {
                MASTERY_ACTIVE_SINGLE
            },
        }
    }

    /// Ignite's duration with talents applied.
    #[must_use]
    pub fn ignite_duration(&self) -> f32 {
        super::ignite::IGNITE_BASE_DURATION + self.ignite_duration_bonus
    }

    /// The Inferno State's duration with talents applied.
    #[must_use]
    pub fn inferno_duration(&self) -> f32 {
        super::resource::INFERNO_DURATION + self.inferno_duration_bonus
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
    fn the_tree_is_the_twelve_talents_the_design_names() {
        assert_eq!(TALENTS.len(), TALENT_COUNT);
        assert_eq!(TALENT_COUNT, 12);
        for (index, talent) in TalentId::ALL.into_iter().enumerate() {
            assert_eq!(talent.index(), index, "{talent:?} is out of order");
            assert_eq!(talent.definition().id, talent);
        }
    }

    #[test]
    fn every_talent_is_described_and_priced_as_designed() {
        let costs: Vec<u32> = TalentId::ALL
            .into_iter()
            .map(|talent| talent.definition().cost)
            .collect();
        assert_eq!(costs, vec![3, 2, 5, 4, 4, 5, 4, 4, 3, 5, 5, 7]);
        assert_eq!(total_talent_cost(), 51);

        for talent in &TALENTS {
            assert!(!talent.name.is_empty());
            assert!(!talent.description.is_empty());
            assert!((1..=TALENT_TIERS).contains(&talent.tier));
        }
    }

    #[test]
    fn a_budget_is_what_makes_the_tree_a_decision() {
        let mut grid = TalentGrid::with_points(DEFAULT_TALENT_BUDGET);
        assert!(grid.purchase(TalentId::DualFireMastery).is_ok());
        assert!(grid.purchase(TalentId::InfernalCapacity).is_ok());
        assert!(grid.purchase(TalentId::VolatileIgnition).is_ok());
        assert_eq!(grid.available_points(), 3);
        assert_eq!(
            grid.purchase(TalentId::ChainPyroblast),
            Err(TalentError::NotEnoughPoints)
        );
        assert_eq!(
            grid.purchase(TalentId::InfernalCapacity),
            Err(TalentError::AlreadyOwned)
        );
    }

    #[test]
    fn every_talent_can_be_enabled_and_disabled_cleanly() {
        let none = TalentEffects::default();
        for talent in TalentId::ALL {
            let grid = TalentGrid::from_talents(&[talent]);
            let effects = grid.effects();
            assert_ne!(effects, none, "{talent:?} changed nothing");

            let mut refunded = grid;
            refunded.reset();
            assert_eq!(refunded.effects(), none, "{talent:?} did not come back off");
        }
    }

    #[test]
    fn infernal_capacity_moves_the_maximum_and_nothing_else_does() {
        assert_eq!(TalentEffects::default().ignite_max_stacks, 5);
        let effects = TalentGrid::from_talents(&[TalentId::InfernalCapacity]).effects();
        assert_eq!(effects.ignite_max_stacks, 6);
    }

    #[test]
    fn the_critical_strike_feedback_never_exceeds_its_cap() {
        // The brief's hard limit, asserted at the source rather than at the call site.
        let base = TalentEffects::default();
        assert!((base.flame_lance_crit_reduction - CRIT_FEEDBACK_BASE).abs() < 1e-6);

        let talented = TalentGrid::from_talents(&[TalentId::SpellfireFeedback]).effects();
        assert!((talented.flame_lance_crit_reduction - 1.5).abs() < 1e-6);
        assert!((talented.pyroblast_crit_reduction - 1.5).abs() < 1e-6);

        for effects in [base, talented] {
            assert!(effects.flame_lance_crit_reduction <= CRIT_FEEDBACK_CAP);
            assert!(effects.pyroblast_crit_reduction <= CRIT_FEEDBACK_CAP);
        }
    }

    #[test]
    fn molten_meteor_and_meteoric_impact_pull_in_opposite_directions() {
        let molten = TalentGrid::from_talents(&[TalentId::MoltenMeteor]).effects();
        assert!(molten.meteor_cannot_crit);
        assert!((molten.meteor_damage_scale - 0.75).abs() < 1e-6);
        assert!(molten.meteor_ground_duration > 0.0);

        let impact = TalentGrid::from_talents(&[TalentId::MeteoricImpact]).effects();
        assert!(!impact.meteor_cannot_crit);
        assert!((impact.meteor_crit_damage_bonus - 1.0).abs() < 1e-6);
    }

    #[test]
    fn extended_flame_and_furnace_mastery_read_through_their_derived_values() {
        assert!((TalentEffects::default().ignite_duration() - 30.0).abs() < 1e-6);
        let extended = TalentGrid::from_talents(&[TalentId::ExtendedFlame]).effects();
        assert!((extended.ignite_duration() - 40.0).abs() < 1e-6);

        assert!((TalentEffects::default().inferno_duration() - 10.0).abs() < 1e-6);
        let furnace = TalentGrid::from_talents(&[TalentId::FurnaceMastery]).effects();
        assert!((furnace.inferno_duration() - 12.0).abs() < 1e-6);
    }
}
