//! The Druid's rows of the ability table.
//!
//! Mana costs, Astral Power generation, heal and damage coefficients. What Astral Power does
//! to an ability is in [`super::tuning`]; the shared shape of a row and the cooldown runtime
//! live in [`crate::ability`].

use super::tuning::{
    LIFEBLOOM_MANA, MOONFIRE_MANA, NATURES_CLEANSE_MANA, NOURISH_MANA, REGROWTH_MANA,
    REJUVENATION_MANA, SUNFIRE_MANA, SWIFTMEND_MANA, TRANQUILITY_MANA, WILD_GROWTH_MANA,
};
use crate::ability::text::TextValue;
use crate::ability::{AbilityDef, ClassRow, DruidRow, FireAbility, TargetMode, ThreatScale};
use crate::combat::DamageType;

/// What every Druid row starts from: nature, healer threat, and a [`DruidRow`].
const BASE: AbilityDef = AbilityDef {
    damage_type: DamageType::Nature,
    threat: ThreatScale::HEALER,
    class: ClassRow::Druid(DruidRow::NONE),
    ..AbilityDef::BASE
};

/// The Druid's abilities, in [`FireAbility`] order.
pub(crate) const ROWS: [AbilityDef; 13] = [
    AbilityDef {
        id: FireAbility::Moonfire,
        name: "MOONFIRE",
        description: "Burn your target with moonlight, dealing Arcane damage immediately and again every {tick_interval} for {duration}.",
        empowered: "Deals {damage_bonus} more damage, and the burn never expires.",
        range: 30.0,
        duration: 18.0,
        coefficient: 1.20,
        tick_coefficient: 0.35,
        tick_interval: 2.0,
        requires_line_of_sight: true,
        class: ClassRow::Druid(DruidRow {
            mana_cost: MOONFIRE_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Sunfire,
        name: "SUNFIRE",
        description: "Scorch your target and every enemy within {radius} of it, dealing damage immediately and again every {tick_interval} for {duration}.",
        empowered: "Deals {damage_bonus} more damage, and for {conversion_duration} you are healed for {conversion} of the damage it deals.",
        range: 30.0,
        radius: 8.0,
        duration: 18.0,
        coefficient: 1.00,
        tick_coefficient: 0.35,
        tick_interval: 2.0,
        requires_line_of_sight: true,
        class: ClassRow::Druid(DruidRow {
            mana_cost: SUNFIRE_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Wrath,
        name: "WRATH",
        description: "Hurl a bolt of nature at your target and apply Nature's Exposure, increasing the Nature and Arcane damage it takes by {exposure} per stack for {exposure_duration}. Costs no Mana, and has a {mana_chance} chance to restore {mana_restore} of your maximum Mana.",
        empowered: "For {wrath_duration}, Wrath also applies Nature's Exposure to every enemy within {wrath_radius} of its target.",
        cast_time: 1.5,
        range: 30.0,
        duration: 15.0,
        coefficient: 2.20,
        requires_line_of_sight: true,
        class: ClassRow::Druid(DruidRow {
            astral_generation: 15.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Rejuvenation,
        name: "REJUVENATION",
        description: "Heal a friendly target every {tick_interval} for {duration}.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        range: 40.0,
        duration: 18.0,
        tick_coefficient: 0.45,
        tick_interval: 3.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: REJUVENATION_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Regrowth,
        name: "REGROWTH",
        description: "Heal a friendly target for a large amount, then again every {tick_interval} for {duration}.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        cast_time: 1.5,
        range: 40.0,
        duration: 12.0,
        coefficient: 3.00,
        tick_coefficient: 0.30,
        tick_interval: 3.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: REGROWTH_MANA,
            astral_generation: 10.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Lifebloom,
        name: "LIFEBLOOM",
        description: "Heal a friendly target every {tick_interval} for {duration}. When Lifebloom expires naturally, it blooms for a large additional heal.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        range: 40.0,
        duration: 15.0,
        tick_coefficient: 0.25,
        tick_interval: 1.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: LIFEBLOOM_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            bloom_coefficient: 2.50,
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::WildGrowth,
        name: "WILD GROWTH",
        description: "Heal your target and every injured ally within {radius} of it every {tick_interval} for {duration}.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        cooldown: 15.0,
        range: 40.0,
        radius: 15.0,
        duration: 7.0,
        tick_coefficient: 0.30,
        tick_interval: 1.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: WILD_GROWTH_MANA,
            astral_generation: 12.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Swiftmend,
        name: "SWIFTMEND",
        description: "Consume one of your heal-over-time effects on a friendly target to instantly heal it for a large amount.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        cooldown: 12.0,
        range: 40.0,
        coefficient: 4.00,
        class: ClassRow::Druid(DruidRow {
            mana_cost: SWIFTMEND_MANA,
            astral_generation: 10.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Nourish,
        name: "NOURISH",
        description: "Heal a friendly target, healing for {per_hot} more for each of your heal-over-time effects already on it, with no limit.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        cast_time: 2.0,
        range: 40.0,
        coefficient: 2.80,
        class: ClassRow::Druid(DruidRow {
            mana_cost: NOURISH_MANA,
            astral_generation: 8.0,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::NaturesCleanse,
        name: "NATURES CLEANSE",
        description: "Remove all Magic, Poison, Curse and Disease effects from a friendly target.",
        target: TargetMode::Friendly,
        cooldown: 8.0,
        range: 40.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: NATURES_CLEANSE_MANA,
            astral_generation: 5.0,
            ..DruidRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::WildRush,
        name: "WILD RUSH",
        description: "Increase your movement speed by {speed} and reduce all damage you take by {reduction} for {duration}.",
        target: TargetMode::SelfCast,
        cooldown: 25.0,
        duration: 6.0,
        class: ClassRow::Druid(DruidRow::NONE),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::AstralPower,
        name: "ASTRAL POWER",
        description: "Spend a full bar of {cost} Astral Power to empower your next compatible spell. The empowerment is consumed when that spell is cast.",
        target: TargetMode::SelfCast,
        class: ClassRow::Druid(DruidRow::NONE),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Tranquility,
        name: "TRANQUILITY",
        description: "Channel for {cast_time}, healing every ally within {radius} every {tick_interval}.",
        empowered: "Heals for {heal_bonus} more.",
        target: TargetMode::Friendly,
        cast_time: 6.0,
        cooldown: 120.0,
        range: 40.0,
        radius: 20.0,
        duration: 6.0,
        tick_coefficient: 1.20,
        tick_interval: 1.0,
        class: ClassRow::Druid(DruidRow {
            mana_cost: TRANQUILITY_MANA,
            astral_empowerable: true,
            ..DruidRow::NONE
        }),
        ..BASE
    },
];

/// The Druid's template tokens: what Astral Power and the class's rules are worth.
pub(crate) fn token(ability: FireAbility, key: &str) -> Option<TextValue> {
    use super::tuning::*;
    use TextValue::{Count, Metres, Percent, Seconds};
    Some(match (ability, key) {
        (_, "heal_bonus") => Percent(ASTRAL_HEAL_BONUS),
        (FireAbility::Moonfire, "damage_bonus") => Percent(ASTRAL_MOONFIRE_DAMAGE),
        (FireAbility::Sunfire, "damage_bonus") => Percent(ASTRAL_SUNFIRE_DAMAGE),
        (FireAbility::Sunfire, "conversion") => Percent(ASTRAL_SUNFIRE_CONVERSION),
        (FireAbility::Sunfire, "conversion_duration") => Seconds(ASTRAL_SUNFIRE_DURATION),
        (FireAbility::Wrath, "exposure") => Percent(NATURES_EXPOSURE_PER_STACK),
        (FireAbility::Wrath, "exposure_duration") => Seconds(NATURES_EXPOSURE_DURATION),
        (FireAbility::Wrath, "mana_chance") => Percent(WRATH_MANA_CHANCE),
        (FireAbility::Wrath, "mana_restore") => Percent(WRATH_MANA_RESTORE),
        (FireAbility::Wrath, "wrath_duration") => Seconds(ASTRAL_WRATH_DURATION),
        (FireAbility::Wrath, "wrath_radius") => Metres(ASTRAL_WRATH_RADIUS),
        (FireAbility::Nourish, "per_hot") => Percent(NOURISH_PER_HOT),
        (FireAbility::WildRush, "speed") => Percent(WILD_RUSH_SPEED),
        (FireAbility::WildRush, "reduction") => Percent(WILD_RUSH_REDUCTION),
        (FireAbility::AstralPower, "cost") => Count(ASTRAL_SPEND),
        _ => return None,
    })
}
