//! The FireMage's talent tree: twelve talents, bought individually against a point budget.
//!
//! ```text
//! table (talents.json)  ->  TalentGrid (what is bought)  ->  TalentEffects (what it means)
//! ```
//!
//! The three layers are the hero-generic ones in [`crate::talent`]: the table is data (the
//! built-in [`DEFS`] here are the defaults the unit tests run on and the numbers the shipped
//! JSON must equal), the grid is the player's purchases, and [`TalentEffects`] is the
//! *derived* answer gameplay asks - "what is the Ignite maximum", "how much extra does a
//! detonation do" - recomputed whenever a talent is bought and never scattered as
//! `if has_talent(..)` checks through the combat code.
//!
//! That third layer is what makes Infernal Capacity work everywhere at once: the Ignite
//! maximum is a field on [`TalentEffects`], and every ability, tooltip, bar and simulator
//! run reads it.

pub use crate::talent::{TALENT_TIERS, TalentError, TalentGrid};
use crate::talent::{TalentDef, TalentKey, hero_talents};

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
    // Hidden book nodes (tier 0), one per enhancement-book option, in book order.
    /// Pyroblast Book: a second Pyroblast charge.
    TwinPyre,
    /// Pyroblast Book: Pyroblast bounces a few times.
    RicochetPyre,
    /// Pyroblast Book: a critical Pyroblast cuts Flame Lance further.
    SearingPyre,
    /// Meteor Book: a critical Meteor hits harder and hurries Pyroblast.
    ImpactCrater,
    /// Meteor Book: Meteor leaves burning ground, hitting a little softer.
    ScorchedEarth,
    /// Meteor Book: critical Ignite ticks often hurry Meteor.
    FallingStar,
    /// Ignite Book: Ignite lasts longer and ticks a little softer.
    SlowBurn,
    /// Ignite Book: Detonate hits harder per Ignite stack.
    HotCore,
    /// Ignite Book: Detonate often hurries the area spells.
    Flashpoint,
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
}

impl TalentId {
    /// The hidden book nodes, in table order after the visible talents.
    pub const NODES: [Self; 9] = [
        Self::TwinPyre,
        Self::RicochetPyre,
        Self::SearingPyre,
        Self::ImpactCrater,
        Self::ScorchedEarth,
        Self::FallingStar,
        Self::SlowBurn,
        Self::HotCore,
        Self::Flashpoint,
    ];
}

impl TalentKey for TalentId {
    const HERO: crate::hero::Hero = crate::hero::Hero::FireMage;
    const ALL: &'static [Self] = &Self::ALL;

    fn index(self) -> usize {
        self as usize
    }
}

/// How many talents the player can buy. Grows with the table; nothing restates it.
pub const TALENT_COUNT: usize = 12;

/// Talent points the simulator's combination search spends by default.
///
/// Enough for four or five talents out of twelve, which is what makes the tree a set of
/// decisions rather than a checklist.
pub const DEFAULT_TALENT_BUDGET: u32 = 20;

/// The built-in FireMage rows: the same numbers `assets/catalog/talents.json` ships.
pub const DEFS: [TalentDef; TALENT_COUNT + 9] = [
    TalentDef {
        key: "burning_embers",
        name: "BURNING EMBERS",
        tier: 1,
        cost: 3,
        description: "EACH IGNITE STACK CONSUMED INCREASES DETONATE DAMAGE BY {per_stack%}%",
        values: &[("per_stack", 0.03)],
    },
    TalentDef {
        key: "extended_flame",
        name: "EXTENDED FLAME",
        tier: 1,
        cost: 2,
        description: "IGNITE LASTS {bonus_seconds} SECONDS LONGER",
        values: &[("bonus_seconds", 10.0)],
    },
    TalentDef {
        key: "infernal_capacity",
        name: "INFERNAL CAPACITY",
        tier: 3,
        cost: 5,
        description: "MAXIMUM IGNITE +{extra_stacks}. DETONATE THEN REQUIRES THE FULL STACK",
        values: &[("extra_stacks", 1.0)],
    },
    TalentDef {
        key: "rapid_combustion",
        name: "RAPID COMBUSTION",
        tier: 2,
        cost: 4,
        description: "DETONATE: {chance%}% CHANCE TO CUT FLAME CONE, FIRESTORM AND METEOR BY {reduction} S",
        values: &[("chance", 0.20), ("reduction", 2.0)],
    },
    TalentDef {
        key: "molten_meteor",
        name: "MOLTEN METEOR",
        tier: 2,
        cost: 4,
        description: "METEOR IMPACT REDUCED TO {damage_scale%}% AND CANNOT CRIT. LEAVES BURNING GROUND FOR {ground_duration} S",
        values: &[
            ("damage_scale", 0.75),
            ("ground_duration", 15.0),
            ("ground_interval", 3.0),
            ("ground_tick_coefficient", 0.55),
            ("ground_vulnerability", 0.03),
        ],
    },
    TalentDef {
        key: "furnace_mastery",
        name: "FURNACE MASTERY",
        tier: 3,
        cost: 5,
        description: "THE INFERNO STATE LASTS {bonus_seconds} SECONDS LONGER",
        values: &[("bonus_seconds", 2.0)],
    },
    TalentDef {
        key: "meteoric_impact",
        name: "METEORIC IMPACT",
        tier: 2,
        cost: 4,
        description: "METEOR +{crit_damage_bonus%}% CRIT DAMAGE. A CRITICAL METEOR CUTS PYROBLAST BY {pyroblast_reduction} S",
        values: &[("crit_damage_bonus", 1.0), ("pyroblast_reduction", 4.0)],
    },
    TalentDef {
        key: "spellfire_feedback",
        name: "SPELLFIRE FEEDBACK",
        tier: 2,
        cost: 4,
        description: "CRIT FLAME LANCE AND CRIT PYROBLAST CUT EACH OTHER BY {reduction} S INSTEAD OF {base_reduction}",
        values: &[("reduction", 1.5), ("base_reduction", 1.0)],
    },
    TalentDef {
        key: "burning_precision",
        name: "BURNING PRECISION",
        tier: 1,
        cost: 3,
        description: "A CRITICAL IGNITE TICK HAS A {chance%}% CHANCE TO CUT METEOR BY {reduction} S",
        values: &[("chance", 0.10), ("reduction", 1.0)],
    },
    TalentDef {
        key: "volatile_ignition",
        name: "VOLATILE IGNITION",
        tier: 3,
        cost: 5,
        description: "IGNITE DAMAGE REDUCED TO {ignite_scale%}%. PYROBLAST GAINS {extra_charges} CHARGE",
        values: &[("ignite_scale", 0.80), ("extra_charges", 1.0)],
    },
    TalentDef {
        key: "chain_pyroblast",
        name: "CHAIN PYROBLAST",
        tier: 3,
        cost: 5,
        description: "PYROBLAST BOUNCES AT YOUR CRIT CHANCE. -{falloff%}% A BOUNCE, {max_bounces} MAXIMUM",
        values: &[("falloff", 0.20), ("max_bounces", 10.0)],
    },
    TalentDef {
        key: "dual_fire_mastery",
        name: "DUAL FIRE MASTERY",
        tier: 4,
        cost: 7,
        description: "FIRE MASTERY GRANTS +{dual_bonus%}% HASTE AND +{dual_bonus%}% CRIT INSTEAD OF ONE STAT",
        values: &[("dual_bonus", 0.10), ("single_bonus", 0.15)],
    },
    TalentDef {
        key: "bk_twin_pyre",
        name: "TWIN PYRE",
        tier: 0,
        cost: 0,
        description: "PYROBLAST GAINS {extra_charges} CHARGE",
        values: &[("extra_charges", 1.0)],
    },
    TalentDef {
        key: "bk_ricochet_pyre",
        name: "RICOCHET PYRE",
        tier: 0,
        cost: 0,
        description: "PYROBLAST BOUNCES AT YOUR CRIT CHANCE. -{falloff%}% A BOUNCE, {max_bounces} MAXIMUM",
        values: &[("falloff", 0.3), ("max_bounces", 3.0)],
    },
    TalentDef {
        key: "bk_searing_pyre",
        name: "SEARING PYRE",
        tier: 0,
        cost: 0,
        description: "A CRITICAL PYROBLAST CUTS FLAME LANCE BY A FURTHER {reduction} S",
        values: &[("reduction", 0.5)],
    },
    TalentDef {
        key: "bk_impact_crater",
        name: "IMPACT CRATER",
        tier: 0,
        cost: 0,
        description: "METEOR +{crit_damage_bonus%}% CRIT DAMAGE. A CRITICAL METEOR CUTS PYROBLAST BY {pyroblast_reduction} S",
        values: &[("crit_damage_bonus", 0.5), ("pyroblast_reduction", 2.0)],
    },
    TalentDef {
        key: "bk_scorched_earth",
        name: "SCORCHED EARTH",
        tier: 0,
        cost: 0,
        description: "METEOR IMPACT REDUCED TO {damage_scale%}%. LEAVES BURNING GROUND FOR {ground_duration} S",
        values: &[("damage_scale", 0.85), ("ground_duration", 10.0), ("ground_interval", 3.0), ("ground_tick_coefficient", 0.4), ("ground_vulnerability", 0.02)],
    },
    TalentDef {
        key: "bk_falling_star",
        name: "FALLING STAR",
        tier: 0,
        cost: 0,
        description: "A CRITICAL IGNITE TICK HAS A {chance%}% CHANCE TO CUT METEOR BY {reduction} S",
        values: &[("chance", 0.2), ("reduction", 1.5)],
    },
    TalentDef {
        key: "bk_slow_burn",
        name: "SLOW BURN",
        tier: 0,
        cost: 0,
        description: "IGNITE LASTS {bonus_seconds} SECONDS LONGER. ITS DAMAGE REDUCED TO {ignite_scale%}%",
        values: &[("bonus_seconds", 6.0), ("ignite_scale", 0.95)],
    },
    TalentDef {
        key: "bk_hot_core",
        name: "HOT CORE",
        tier: 0,
        cost: 0,
        description: "EACH IGNITE STACK CONSUMED INCREASES DETONATE DAMAGE BY A FURTHER {per_stack%}%",
        values: &[("per_stack", 0.02)],
    },
    TalentDef {
        key: "bk_flashpoint",
        name: "FLASHPOINT",
        tier: 0,
        cost: 0,
        description: "DETONATE: {chance%}% CHANCE TO CUT FLAME CONE, FIRESTORM AND METEOR BY {reduction} S",
        values: &[("chance", 0.15), ("reduction", 1.5)],
    },
];

/// Points needed to own every FireMage talent.
///
/// Computed from the table, never written down: adding a talent raises it automatically.
#[must_use]
pub fn total_talent_cost() -> u32 {
    hero_talents(crate::hero::Hero::FireMage).total_cost()
}

/// The most a critical Flame Lance or Pyroblast may ever cut the other by.
///
/// A hard ceiling, not a talent value. Nothing in the class is allowed to exceed it, which
/// is asserted by a test rather than trusted.
pub const CRIT_FEEDBACK_CAP: f32 = 1.5;

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
    /// Seconds between burning ground ticks.
    pub meteor_ground_interval: f32,
    /// Damage a burning ground tick deals, as a fraction of spell power.
    pub meteor_ground_coefficient: f32,
    /// Extra fire damage taken by whatever stands in the burning ground.
    pub meteor_ground_vulnerability: f32,
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

impl Default for TalentEffects {
    fn default() -> Self {
        Self::from_grid(&TalentGrid::with_points(crate::hero::Hero::FireMage, 0))
    }
}

impl TalentEffects {
    /// Resolves a grid into the numbers gameplay uses.
    #[must_use]
    pub fn from_grid(grid: &TalentGrid) -> Self {
        let table = hero_talents(crate::hero::Hero::FireMage);
        let has = |talent: TalentId| grid.has(talent);
        let v = |talent: TalentId, name: &str| table.talents[talent.index()].values.value(name);
        let if_has = |talent: TalentId, name: &str| {
            if has(talent) { v(talent, name) } else { 0.0 }
        };
        let feedback = if has(TalentId::SpellfireFeedback) {
            v(TalentId::SpellfireFeedback, "reduction")
        } else {
            v(TalentId::SpellfireFeedback, "base_reduction")
        }
        .min(CRIT_FEEDBACK_CAP);

        let mut fx = Self {
            ignite_max_stacks: super::ignite::IGNITE_BASE_MAX_STACKS
                + if_has(TalentId::InfernalCapacity, "extra_stacks") as u8,
            ignite_duration_bonus: if_has(TalentId::ExtendedFlame, "bonus_seconds"),
            ignite_damage_scale: if has(TalentId::VolatileIgnition) {
                v(TalentId::VolatileIgnition, "ignite_scale")
            } else {
                1.0
            },
            detonate_damage_per_stack: if_has(TalentId::BurningEmbers, "per_stack"),
            detonate_cooldown_chance: if_has(TalentId::RapidCombustion, "chance"),
            detonate_cooldown_reduction: if_has(TalentId::RapidCombustion, "reduction"),
            inferno_duration_bonus: if_has(TalentId::FurnaceMastery, "bonus_seconds"),
            meteor_crit_damage_bonus: if_has(TalentId::MeteoricImpact, "crit_damage_bonus"),
            meteor_crit_pyroblast_reduction: if_has(
                TalentId::MeteoricImpact,
                "pyroblast_reduction",
            ),
            meteor_damage_scale: if has(TalentId::MoltenMeteor) {
                v(TalentId::MoltenMeteor, "damage_scale")
            } else {
                1.0
            },
            meteor_cannot_crit: has(TalentId::MoltenMeteor),
            meteor_ground_duration: if_has(TalentId::MoltenMeteor, "ground_duration"),
            meteor_ground_interval: v(TalentId::MoltenMeteor, "ground_interval"),
            meteor_ground_coefficient: v(TalentId::MoltenMeteor, "ground_tick_coefficient"),
            meteor_ground_vulnerability: v(TalentId::MoltenMeteor, "ground_vulnerability"),
            flame_lance_crit_reduction: feedback,
            pyroblast_crit_reduction: feedback,
            ignite_crit_meteor_chance: if_has(TalentId::BurningPrecision, "chance"),
            ignite_crit_meteor_reduction: if_has(TalentId::BurningPrecision, "reduction"),
            pyroblast_extra_charges: if_has(TalentId::VolatileIgnition, "extra_charges") as u8,
            pyroblast_bounces: has(TalentId::ChainPyroblast),
            pyroblast_bounce_falloff: if_has(TalentId::ChainPyroblast, "falloff"),
            pyroblast_max_bounces: if_has(TalentId::ChainPyroblast, "max_bounces") as u8,
            dual_mastery: has(TalentId::DualFireMastery),
            mastery_active_bonus: if has(TalentId::DualFireMastery) {
                v(TalentId::DualFireMastery, "dual_bonus")
            } else {
                v(TalentId::DualFireMastery, "single_bonus")
            },
        };
        // Book options come last and compose with the talents above.
        use TalentId::{
            Flashpoint, HotCore, ImpactCrater, FallingStar, RicochetPyre, ScorchedEarth,
            SearingPyre, SlowBurn, TwinPyre,
        };
        if has(TwinPyre) {
            fx.pyroblast_extra_charges += v(TwinPyre, "extra_charges") as u8;
        }
        if has(RicochetPyre) {
            fx.pyroblast_bounce_falloff = if fx.pyroblast_bounces {
                fx.pyroblast_bounce_falloff.min(v(RicochetPyre, "falloff"))
            } else {
                v(RicochetPyre, "falloff")
            };
            fx.pyroblast_max_bounces = fx
                .pyroblast_max_bounces
                .max(v(RicochetPyre, "max_bounces") as u8);
            fx.pyroblast_bounces = true;
        }
        if has(SearingPyre) {
            fx.pyroblast_crit_reduction =
                (fx.pyroblast_crit_reduction + v(SearingPyre, "reduction")).min(CRIT_FEEDBACK_CAP);
        }
        if has(ImpactCrater) {
            fx.meteor_crit_damage_bonus += v(ImpactCrater, "crit_damage_bonus");
            fx.meteor_crit_pyroblast_reduction += v(ImpactCrater, "pyroblast_reduction");
        }
        if has(ScorchedEarth) {
            fx.meteor_damage_scale *= v(ScorchedEarth, "damage_scale");
            if fx.meteor_ground_duration <= 0.0 {
                fx.meteor_ground_duration = v(ScorchedEarth, "ground_duration");
                fx.meteor_ground_interval = v(ScorchedEarth, "ground_interval");
                fx.meteor_ground_coefficient = v(ScorchedEarth, "ground_tick_coefficient");
                fx.meteor_ground_vulnerability = v(ScorchedEarth, "ground_vulnerability");
            }
        }
        if has(FallingStar) {
            fx.ignite_crit_meteor_chance =
                fx.ignite_crit_meteor_chance.max(v(FallingStar, "chance"));
            fx.ignite_crit_meteor_reduction =
                fx.ignite_crit_meteor_reduction.max(v(FallingStar, "reduction"));
        }
        if has(SlowBurn) {
            fx.ignite_duration_bonus += v(SlowBurn, "bonus_seconds");
            fx.ignite_damage_scale *= v(SlowBurn, "ignite_scale");
        }
        if has(HotCore) {
            fx.detonate_damage_per_stack += v(HotCore, "per_stack");
        }
        if has(Flashpoint) {
            fx.detonate_cooldown_chance = fx.detonate_cooldown_chance.max(v(Flashpoint, "chance"));
            fx.detonate_cooldown_reduction =
                fx.detonate_cooldown_reduction.max(v(Flashpoint, "reduction"));
        }
        fx
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

    /// How long the burning ground's fire vulnerability lasts once applied.
    ///
    /// Twice the ground's tick interval, so standing in it keeps the debuff up and stepping out
    /// of it lets the debuff lapse rather than ending it on the frame the foot leaves.
    #[must_use]
    pub fn vulnerability_duration(&self) -> f32 {
        self.meteor_ground_interval * 2.0
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::hero::Hero;

    #[test]
    fn the_tree_is_the_twelve_talents_the_design_names() {
        assert_eq!(hero_talents(Hero::FireMage).visible().len(), TALENT_COUNT);
        assert_eq!(TALENT_COUNT, 12);
        for (index, talent) in TalentId::ALL.into_iter().enumerate() {
            assert_eq!(talent.index(), index, "{talent:?} is out of order");
            assert_eq!(TalentKey::spec(talent).key, DEFS[index].key);
        }
    }

    #[test]
    fn every_talent_is_described_and_priced_as_designed() {
        let costs: Vec<u32> = TalentId::ALL
            .into_iter()
            .map(|talent| TalentKey::spec(talent).cost)
            .collect();
        assert_eq!(costs, vec![3, 2, 5, 4, 4, 5, 4, 4, 3, 5, 5, 7]);
        assert_eq!(total_talent_cost(), 51);

        for talent in hero_talents(Hero::FireMage).visible() {
            assert!(!talent.name.is_empty());
            assert!(!talent.description.is_empty());
            assert!(!talent.description.contains('{'), "{}", talent.description);
            assert!((1..=TALENT_TIERS).contains(&talent.tier));
        }
    }

    #[test]
    fn a_budget_is_what_makes_the_tree_a_decision() {
        let mut grid = TalentGrid::with_points(Hero::FireMage, DEFAULT_TALENT_BUDGET);
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
            let effects = TalentEffects::from_grid(&grid);
            assert_ne!(effects, none, "{talent:?} changed nothing");

            let mut refunded = grid;
            refunded.reset();
            assert_eq!(
                TalentEffects::from_grid(&refunded),
                none,
                "{talent:?} did not come back off"
            );
        }
    }

    #[test]
    fn every_book_node_changes_its_ability_through_the_talent_path() {
        let none = TalentEffects::default();
        let fx = |node: TalentId| TalentEffects::from_grid(&TalentGrid::from_nodes(&[node]));
        for node in TalentId::NODES {
            assert!(TalentKey::spec(node).is_hidden(), "{node:?}");
            assert_ne!(fx(node), none, "{node:?} changed nothing");
        }
        assert_eq!(fx(TalentId::TwinPyre).pyroblast_extra_charges, 1);
        let ricochet = fx(TalentId::RicochetPyre);
        assert!(ricochet.pyroblast_bounces && ricochet.pyroblast_max_bounces == 3);
        assert!(fx(TalentId::SearingPyre).pyroblast_crit_reduction > 1.0);
        assert!(fx(TalentId::ImpactCrater).meteor_crit_damage_bonus > 0.0);
        let earth = fx(TalentId::ScorchedEarth);
        assert!(earth.meteor_ground_duration > 0.0 && !earth.meteor_cannot_crit);
        assert!(fx(TalentId::FallingStar).ignite_crit_meteor_chance > 0.1);
        assert!(fx(TalentId::SlowBurn).ignite_duration() > 30.0);
        assert!(fx(TalentId::HotCore).detonate_damage_per_stack > 0.0);
        assert!(fx(TalentId::Flashpoint).detonate_cooldown_chance > 0.0);
    }

    #[test]
    fn infernal_capacity_moves_the_maximum_and_nothing_else_does() {
        assert_eq!(TalentEffects::default().ignite_max_stacks, 5);
        let effects =
            TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::InfernalCapacity]));
        assert_eq!(effects.ignite_max_stacks, 6);
    }

    #[test]
    fn the_critical_strike_feedback_never_exceeds_its_cap() {
        let base = TalentEffects::default();
        assert!((base.flame_lance_crit_reduction - 1.0).abs() < 1e-6);

        let talented =
            TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::SpellfireFeedback]));
        assert!((talented.flame_lance_crit_reduction - 1.5).abs() < 1e-6);
        assert!((talented.pyroblast_crit_reduction - 1.5).abs() < 1e-6);

        for effects in [base, talented] {
            assert!(effects.flame_lance_crit_reduction <= CRIT_FEEDBACK_CAP);
            assert!(effects.pyroblast_crit_reduction <= CRIT_FEEDBACK_CAP);
        }
    }

    #[test]
    fn molten_meteor_and_meteoric_impact_pull_in_opposite_directions() {
        let molten = TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::MoltenMeteor]));
        assert!(molten.meteor_cannot_crit);
        assert!((molten.meteor_damage_scale - 0.75).abs() < 1e-6);
        assert!(molten.meteor_ground_duration > 0.0);

        let impact =
            TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::MeteoricImpact]));
        assert!(!impact.meteor_cannot_crit);
        assert!((impact.meteor_crit_damage_bonus - 1.0).abs() < 1e-6);
    }

    #[test]
    fn extended_flame_and_furnace_mastery_read_through_their_derived_values() {
        assert!((TalentEffects::default().ignite_duration() - 30.0).abs() < 1e-6);
        let extended =
            TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::ExtendedFlame]));
        assert!((extended.ignite_duration() - 40.0).abs() < 1e-6);

        assert!((TalentEffects::default().inferno_duration() - 10.0).abs() < 1e-6);
        let furnace =
            TalentEffects::from_grid(&TalentGrid::from_talents(&[TalentId::FurnaceMastery]));
        assert!((furnace.inferno_duration() - 12.0).abs() < 1e-6);
    }

    #[test]
    fn the_keys_are_the_ones_characters_already_store() {
        let keys: Vec<&str> = hero_talents(Hero::FireMage)
            .visible()
            .iter()
            .map(|talent| talent.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "burning_embers",
                "extended_flame",
                "infernal_capacity",
                "rapid_combustion",
                "molten_meteor",
                "furnace_mastery",
                "meteoric_impact",
                "spellfire_feedback",
                "burning_precision",
                "volatile_ignition",
                "chain_pyroblast",
                "dual_fire_mastery",
            ]
        );
    }
}
