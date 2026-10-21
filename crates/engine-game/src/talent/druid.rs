//! The Druid's eight talents and what they mean.
//!
//! ```text
//! DEFS (defaults) / talents.json  ->  TalentGrid  ->  DruidTalentEffects
//! ```
//!
//! [`DruidTalentEffects`] is the third layer: every field is a number the executor reads
//! (`druid_exec`, the status table), neutral when the talent is not bought. No call site asks
//! whether a talent is owned.

use super::{TalentDef, TalentGrid, TalentKey, hero_talents};
use crate::hero::Hero;

/// Every Druid talent, in table order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DruidTalent {
    /// Every fourth consecutive Wrath is instant and generates double Astral Power.
    SwiftWrath,
    /// Sunfire reaches further and burns softer.
    VerdantSunfire,
    /// Nature's Exposure stacks higher and lapses sooner.
    DeepExposure,
    /// Regrowth and Swiftmend leave a seed that pays out when the target is next hurt.
    LivingSeed,
    /// Swiftmend recharges faster and refunds Astral Power.
    Overgrowth,
    /// Empowerment is cheaper and weaker.
    AstralAttunement,
    /// Wild Rush shields nearby allies too.
    BarkskinInstinct,
    /// Tranquility recharges sooner and costs less Mana, and heals less.
    TranquilMind,
    // Hidden book nodes (tier 0), one per enhancement-book option, in book order.
    /// Sunfire Book: a wider, softer Sunfire.
    WideBloom,
    /// Sunfire Book: a tighter, harder Sunfire.
    SearingBloom,
    /// Sunfire Book: a little of both.
    BalancedBloom,
    /// Swiftmend Book: Swiftmend recharges much sooner.
    QuickMend,
    /// Swiftmend Book: Swiftmend refunds a lot of Astral Power.
    AstralMend,
    /// Swiftmend Book: a modest cooldown cut and refund.
    MeasuredMend,
    /// Tranquility Book: cheaper Tranquility, slightly weaker.
    GentlePeace,
    /// Tranquility Book: Tranquility recharges much sooner, weaker.
    SwiftPeace,
    /// Tranquility Book: a stronger, dearer Tranquility.
    DeepPeace,
}

impl TalentKey for DruidTalent {
    const HERO: Hero = Hero::Druid;
    const ALL: &'static [Self] = &[
        Self::SwiftWrath,
        Self::VerdantSunfire,
        Self::DeepExposure,
        Self::LivingSeed,
        Self::Overgrowth,
        Self::AstralAttunement,
        Self::BarkskinInstinct,
        Self::TranquilMind,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

impl DruidTalent {
    /// The hidden book nodes, in table order after the visible talents.
    pub const NODES: [Self; 9] = [
        Self::WideBloom,
        Self::SearingBloom,
        Self::BalancedBloom,
        Self::QuickMend,
        Self::AstralMend,
        Self::MeasuredMend,
        Self::GentlePeace,
        Self::SwiftPeace,
        Self::DeepPeace,
    ];
}

/// The built-in Druid rows: the same numbers `assets/catalog/talents.json` ships.
pub const DEFS: [TalentDef; 17] = [
    TalentDef {
        key: "swift_wrath",
        name: "SWIFT WRATH",
        tier: 1,
        cost: 3,
        description: "EVERY {every}TH CONSECUTIVE WRATH IS INSTANT AND GENERATES {astral_multiplier}X ASTRAL POWER",
        values: &[("every", 4.0), ("astral_multiplier", 2.0)],
    },
    TalentDef {
        key: "verdant_sunfire",
        name: "VERDANT SUNFIRE",
        tier: 1,
        cost: 3,
        description: "SUNFIRE RADIUS +{radius_bonus%}%. SUNFIRE TICK DAMAGE -{tick_penalty%}%",
        values: &[("radius_bonus", 0.5), ("tick_penalty", 0.2)],
    },
    TalentDef {
        key: "deep_exposure",
        name: "DEEP EXPOSURE",
        tier: 2,
        cost: 4,
        description: "NATURES EXPOSURE MAXIMUM STACKS +{extra_stacks}. ITS DURATION -{duration_penalty%}%",
        values: &[("extra_stacks", 4.0), ("duration_penalty", 0.3)],
    },
    TalentDef {
        key: "living_seed",
        name: "LIVING SEED",
        tier: 2,
        cost: 4,
        description: "REGROWTH AND SWIFTMEND LEAVE A SEED. {fraction%}% OF THE HEAL IS PAID WHEN THE TARGET NEXT TAKES DAMAGE WITHIN {window} S",
        values: &[("fraction", 0.3), ("window", 10.0)],
    },
    TalentDef {
        key: "overgrowth",
        name: "OVERGROWTH",
        tier: 3,
        cost: 4,
        description: "SWIFTMEND COOLDOWN -{cooldown_cut} S. EACH SWIFTMEND REFUNDS {astral_refund} ASTRAL POWER",
        values: &[("cooldown_cut", 3.0), ("astral_refund", 10.0)],
    },
    TalentDef {
        key: "astral_attunement",
        name: "ASTRAL ATTUNEMENT",
        tier: 3,
        cost: 4,
        description: "EMPOWERMENT COSTS {spend} ASTRAL POWER INSTEAD OF 100. EMPOWERED BONUSES -{bonus_penalty%}%",
        values: &[("spend", 75.0), ("bonus_penalty", 0.15)],
    },
    TalentDef {
        key: "barkskin_instinct",
        name: "BARKSKIN INSTINCT",
        tier: 4,
        cost: 5,
        description: "WILD RUSH ALSO GIVES ALLIES WITHIN {radius} M {reduction%}% DAMAGE REDUCTION FOR {duration} S",
        values: &[("radius", 8.0), ("reduction", 0.25), ("duration", 4.0)],
    },
    TalentDef {
        key: "tranquil_mind",
        name: "TRANQUIL MIND",
        tier: 4,
        cost: 5,
        description: "TRANQUILITY COOLDOWN -{cooldown_cut} S AND MANA COST -{mana_cut%}%. ITS HEALING -{heal_penalty%}%",
        values: &[
            ("cooldown_cut", 40.0),
            ("mana_cut", 0.5),
            ("heal_penalty", 0.15),
        ],
    },
    TalentDef {
        key: "bk_wide_bloom",
        name: "WIDE BLOOM",
        tier: 0,
        cost: 0,
        description: "SUNFIRE RADIUS +{radius_bonus%}%. SUNFIRE TICK DAMAGE -{tick_penalty%}%",
        values: &[("radius_bonus", 0.6), ("tick_penalty", 0.15)],
    },
    TalentDef {
        key: "bk_searing_bloom",
        name: "SEARING BLOOM",
        tier: 0,
        cost: 0,
        description: "SUNFIRE TICK DAMAGE +{tick_bonus%}%. SUNFIRE RADIUS -{radius_penalty%}%",
        values: &[("tick_bonus", 0.35), ("radius_penalty", 0.2)],
    },
    TalentDef {
        key: "bk_balanced_bloom",
        name: "BALANCED BLOOM",
        tier: 0,
        cost: 0,
        description: "SUNFIRE RADIUS +{radius_bonus%}% AND TICK DAMAGE +{tick_bonus%}%",
        values: &[("radius_bonus", 0.25), ("tick_bonus", 0.1)],
    },
    TalentDef {
        key: "bk_quick_mend",
        name: "QUICK MEND",
        tier: 0,
        cost: 0,
        description: "SWIFTMEND COOLDOWN -{cooldown_cut} S",
        values: &[("cooldown_cut", 4.0), ("astral_refund", 0.0)],
    },
    TalentDef {
        key: "bk_astral_mend",
        name: "ASTRAL MEND",
        tier: 0,
        cost: 0,
        description: "EACH SWIFTMEND REFUNDS {astral_refund} ASTRAL POWER",
        values: &[("cooldown_cut", 0.0), ("astral_refund", 20.0)],
    },
    TalentDef {
        key: "bk_measured_mend",
        name: "MEASURED MEND",
        tier: 0,
        cost: 0,
        description: "SWIFTMEND COOLDOWN -{cooldown_cut} S AND REFUNDS {astral_refund} ASTRAL POWER",
        values: &[("cooldown_cut", 2.0), ("astral_refund", 10.0)],
    },
    TalentDef {
        key: "bk_gentle_peace",
        name: "GENTLE PEACE",
        tier: 0,
        cost: 0,
        description: "TRANQUILITY MANA COST -{mana_cut%}%. ITS HEALING -{heal_penalty%}%",
        values: &[("mana_cut", 0.4), ("heal_penalty", 0.1), ("cooldown_cut", 0.0), ("heal_bonus", 0.0), ("mana_penalty", 0.0)],
    },
    TalentDef {
        key: "bk_swift_peace",
        name: "SWIFT PEACE",
        tier: 0,
        cost: 0,
        description: "TRANQUILITY COOLDOWN -{cooldown_cut} S. ITS HEALING -{heal_penalty%}%",
        values: &[("mana_cut", 0.0), ("heal_penalty", 0.15), ("cooldown_cut", 30.0), ("heal_bonus", 0.0), ("mana_penalty", 0.0)],
    },
    TalentDef {
        key: "bk_deep_peace",
        name: "DEEP PEACE",
        tier: 0,
        cost: 0,
        description: "TRANQUILITY HEALING +{heal_bonus%}%. ITS MANA COST +{mana_penalty%}%",
        values: &[("mana_cut", 0.0), ("heal_penalty", 0.0), ("cooldown_cut", 0.0), ("heal_bonus", 0.25), ("mana_penalty", 0.3)],
    },
];

/// What the Druid's bought talents mean. Neutral values when nothing is bought.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DruidTalentEffects {
    /// Swift Wrath: every this-many-th consecutive Wrath is instant. Zero: off.
    pub swift_wrath_every: u32,
    /// Swift Wrath: the Astral Power multiplier on that Wrath.
    pub swift_wrath_astral_scale: f32,
    /// Verdant Sunfire: multiplier on Sunfire's radius.
    pub sunfire_radius_scale: f32,
    /// Verdant Sunfire: multiplier on Sunfire's periodic damage.
    pub sunfire_tick_scale: f32,
    /// Deep Exposure: stacks added to Nature's Exposure's maximum.
    pub exposure_extra_stacks: u8,
    /// Deep Exposure: multiplier on Nature's Exposure's duration.
    pub exposure_duration_scale: f32,
    /// Living Seed: the share of the heal the seed pays. Zero: off.
    pub seed_fraction: f32,
    /// Living Seed: seconds the seed waits for damage.
    pub seed_window: f32,
    /// Overgrowth: seconds off Swiftmend's cooldown.
    pub swiftmend_cooldown_cut: f32,
    /// Overgrowth: Astral Power each Swiftmend gives back.
    pub swiftmend_astral_refund: f32,
    /// Astral Attunement: what an empowerment costs.
    pub astral_spend: f32,
    /// Astral Attunement: multiplier on every empowered bonus.
    pub empowerment_scale: f32,
    /// Barkskin Instinct: radius around the Druid that Wild Rush reaches. Zero: off.
    pub wild_rush_ally_radius: f32,
    /// Barkskin Instinct: the damage reduction allies get.
    pub wild_rush_ally_reduction: f32,
    /// Barkskin Instinct: seconds it lasts.
    pub wild_rush_ally_duration: f32,
    /// Tranquil Mind: seconds off Tranquility's cooldown.
    pub tranquility_cooldown_cut: f32,
    /// Tranquil Mind: multiplier on Tranquility's Mana cost.
    pub tranquility_mana_scale: f32,
    /// Tranquil Mind: multiplier on Tranquility's healing.
    pub tranquility_heal_scale: f32,
}

impl Default for DruidTalentEffects {
    fn default() -> Self {
        Self::from_grid(&TalentGrid::with_points(Hero::Druid, 0))
    }
}

impl DruidTalentEffects {
    /// Resolves a grid into the numbers gameplay uses.
    #[must_use]
    pub fn from_grid(grid: &TalentGrid) -> Self {
        let table = hero_talents(Hero::Druid);
        let on = |talent: DruidTalent| grid.has(talent);
        let v = |talent: DruidTalent, name: &str| table.talents[talent.index()].values.value(name);
        use DruidTalent::{
            AstralAttunement, BarkskinInstinct, DeepExposure, LivingSeed, Overgrowth, SwiftWrath,
            TranquilMind, VerdantSunfire,
        };
        let mut fx = Self {
            swift_wrath_every: 0,
            swift_wrath_astral_scale: 1.0,
            sunfire_radius_scale: 1.0,
            sunfire_tick_scale: 1.0,
            exposure_extra_stacks: 0,
            exposure_duration_scale: 1.0,
            seed_fraction: 0.0,
            seed_window: 0.0,
            swiftmend_cooldown_cut: 0.0,
            swiftmend_astral_refund: 0.0,
            astral_spend: crate::druid::tuning::ASTRAL_SPEND,
            empowerment_scale: 1.0,
            wild_rush_ally_radius: 0.0,
            wild_rush_ally_reduction: 0.0,
            wild_rush_ally_duration: 0.0,
            tranquility_cooldown_cut: 0.0,
            tranquility_mana_scale: 1.0,
            tranquility_heal_scale: 1.0,
        };
        if on(SwiftWrath) {
            fx.swift_wrath_every = v(SwiftWrath, "every").round().max(1.0) as u32;
            fx.swift_wrath_astral_scale = v(SwiftWrath, "astral_multiplier");
        }
        if on(VerdantSunfire) {
            fx.sunfire_radius_scale = 1.0 + v(VerdantSunfire, "radius_bonus");
            fx.sunfire_tick_scale = 1.0 - v(VerdantSunfire, "tick_penalty");
        }
        if on(DeepExposure) {
            fx.exposure_extra_stacks = v(DeepExposure, "extra_stacks").round().max(0.0) as u8;
            fx.exposure_duration_scale = 1.0 - v(DeepExposure, "duration_penalty");
        }
        if on(LivingSeed) {
            fx.seed_fraction = v(LivingSeed, "fraction");
            fx.seed_window = v(LivingSeed, "window");
        }
        if on(Overgrowth) {
            fx.swiftmend_cooldown_cut = v(Overgrowth, "cooldown_cut");
            fx.swiftmend_astral_refund = v(Overgrowth, "astral_refund");
        }
        if on(AstralAttunement) {
            fx.astral_spend = v(AstralAttunement, "spend");
            fx.empowerment_scale = 1.0 - v(AstralAttunement, "bonus_penalty");
        }
        if on(BarkskinInstinct) {
            fx.wild_rush_ally_radius = v(BarkskinInstinct, "radius");
            fx.wild_rush_ally_reduction = v(BarkskinInstinct, "reduction");
            fx.wild_rush_ally_duration = v(BarkskinInstinct, "duration");
        }
        if on(TranquilMind) {
            fx.tranquility_cooldown_cut = v(TranquilMind, "cooldown_cut");
            fx.tranquility_mana_scale = 1.0 - v(TranquilMind, "mana_cut");
            fx.tranquility_heal_scale = 1.0 - v(TranquilMind, "heal_penalty");
        }
        // Book options come last and compose with the talents above.
        use DruidTalent::{
            AstralMend, BalancedBloom, DeepPeace, GentlePeace, MeasuredMend, QuickMend,
            SearingBloom, SwiftPeace, WideBloom,
        };
        if on(WideBloom) {
            fx.sunfire_radius_scale *= 1.0 + v(WideBloom, "radius_bonus");
            fx.sunfire_tick_scale *= 1.0 - v(WideBloom, "tick_penalty");
        }
        if on(SearingBloom) {
            fx.sunfire_tick_scale *= 1.0 + v(SearingBloom, "tick_bonus");
            fx.sunfire_radius_scale *= 1.0 - v(SearingBloom, "radius_penalty");
        }
        if on(BalancedBloom) {
            fx.sunfire_radius_scale *= 1.0 + v(BalancedBloom, "radius_bonus");
            fx.sunfire_tick_scale *= 1.0 + v(BalancedBloom, "tick_bonus");
        }
        for mend in [QuickMend, AstralMend, MeasuredMend] {
            if on(mend) {
                fx.swiftmend_cooldown_cut += v(mend, "cooldown_cut");
                fx.swiftmend_astral_refund += v(mend, "astral_refund");
            }
        }
        for peace in [GentlePeace, SwiftPeace, DeepPeace] {
            if on(peace) {
                fx.tranquility_cooldown_cut += v(peace, "cooldown_cut");
                fx.tranquility_mana_scale *= (1.0 - v(peace, "mana_cut")) * (1.0 + v(peace, "mana_penalty"));
                fx.tranquility_heal_scale *= (1.0 + v(peace, "heal_bonus")) * (1.0 - v(peace, "heal_penalty"));
            }
        }
        fx
    }

    /// Nature's Exposure's stack ceiling with talents applied.
    #[must_use]
    pub fn exposure_max_stacks(&self) -> u8 {
        crate::druid::tuning::NATURES_EXPOSURE_MAX_STACKS + self.exposure_extra_stacks
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn node_effects(node: DruidTalent) -> DruidTalentEffects {
        DruidTalentEffects::from_grid(&TalentGrid::from_nodes(&[node]))
    }

    #[test]
    fn every_book_node_changes_its_ability_through_the_talent_path() {
        let none = DruidTalentEffects::default();
        for node in DruidTalent::NODES {
            assert!(node.spec().is_hidden(), "{node:?}");
            assert_ne!(node_effects(node), none, "{node:?} changed nothing");
        }
        let wide = node_effects(DruidTalent::WideBloom);
        assert!(wide.sunfire_radius_scale > 1.5 && wide.sunfire_tick_scale < 1.0);
        let searing = node_effects(DruidTalent::SearingBloom);
        assert!(searing.sunfire_tick_scale > 1.3 && searing.sunfire_radius_scale < 1.0);
        let balanced = node_effects(DruidTalent::BalancedBloom);
        assert!(balanced.sunfire_tick_scale > 1.0 && balanced.sunfire_radius_scale > 1.0);
        assert!((node_effects(DruidTalent::QuickMend).swiftmend_cooldown_cut - 4.0).abs() < 1e-6);
        assert!((node_effects(DruidTalent::AstralMend).swiftmend_astral_refund - 20.0).abs() < 1e-6);
        let measured = node_effects(DruidTalent::MeasuredMend);
        assert!(measured.swiftmend_cooldown_cut > 0.0 && measured.swiftmend_astral_refund > 0.0);
        assert!(node_effects(DruidTalent::GentlePeace).tranquility_mana_scale < 0.7);
        assert!(node_effects(DruidTalent::SwiftPeace).tranquility_cooldown_cut > 20.0);
        let deep = node_effects(DruidTalent::DeepPeace);
        assert!(deep.tranquility_heal_scale > 1.2 && deep.tranquility_mana_scale > 1.2);
    }

    #[test]
    fn every_druid_talent_changes_something_and_resets_cleanly() {
        let none = DruidTalentEffects::default();
        for talent in DruidTalent::ALL {
            let grid = TalentGrid::from_talents(&[*talent]);
            let fx = DruidTalentEffects::from_grid(&grid);
            assert_ne!(fx, none, "{talent:?} changed nothing");
            let mut refunded = grid;
            refunded.reset();
            assert_eq!(DruidTalentEffects::from_grid(&refunded), none);
        }
    }

    #[test]
    fn costs_follow_the_tiers() {
        let costs: Vec<u32> = DruidTalent::ALL.iter().map(|t| t.spec().cost).collect();
        assert_eq!(costs, vec![3, 3, 4, 4, 4, 4, 5, 5]);
    }
}
