//! The Warrior's rows of the ability table.
//!
//! Cooldowns, durations, ranges, radii, coefficients and what each spender costs. The
//! magnitudes of what twenty Deflection buys are named constants in [`super::tuning`]; the
//! shared shape of a row and the cooldown runtime live in [`crate::ability`].

use crate::ability::text::TextValue;
use crate::ability::{
    AbilityDef, ClassRow, FireAbility, Motion, TargetMode, ThreatScale, WarriorRow,
};
use crate::combat::DamageType;

/// What every Warrior row starts from: physical, tank threat, and a [`WarriorRow`].
const BASE: AbilityDef = AbilityDef {
    damage_type: DamageType::Physical,
    threat: ThreatScale::TANK,
    class: ClassRow::Warrior(WarriorRow::NONE),
    ..AbilityDef::BASE
};

/// The Warrior's abilities, in [`FireAbility`] order: the bar, Taunt, then Last Stand.
pub(crate) const ROWS: [AbilityDef; 12] = [
    AbilityDef {
        id: FireAbility::Recuperate,
        name: "RECUPERATE",
        description: "Strike your target and gain a stack of Recuperate, restoring {heal_per_stack} of your maximum Health every {tick_interval} for {duration}. Stacks up to {max_stacks} times.",
        empowered: "At a full bank of {threshold} Deflection, consumes {cost} Deflection to grant 2 stacks instead of 1.",
        range: 5.0,
        duration: 10.0,
        coefficient: 1.20,
        tick_interval: 1.0,
        requires_line_of_sight: true,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            empower_at: 100.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Swing,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::WarStomp,
        name: "WAR STOMP",
        description: "Stomp towards your target, dealing Physical damage to every enemy in a {arc} arc within {range} and applying Sundered, which reduces attack speed by {attack_speed} per stack for {duration}. Stacks up to {max_stacks} times. While any enemy has {max_stacks} stacks, your Armor is increased by {armor}.",
        empowered: "Applies 2 stacks of Sundered instead of 1.",
        target: TargetMode::TargetCone,
        cooldown: 4.0,
        range: 8.0,
        duration: 10.0,
        coefficient: 1.10,
        threat: ThreatScale::TANK.with_damage(3.0),
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Cleave,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::ColossusStrike,
        name: "COLOSSUS STRIKE",
        description: "Smash the ground, dealing Physical damage plus {impact_health} of your maximum Health to all enemies within {radius}, and making them bleed for {duration}. Every tick of the bleed scales with your maximum Health at the moment it lands.",
        empowered: "The bleed deals {bleed_bonus} more damage.",
        target: TargetMode::SelfCast,
        cooldown: 8.0,
        radius: 8.0,
        duration: 8.0,
        coefficient: 2.00,
        tick_coefficient: 1.50,
        tick_interval: 2.0,
        threat: ThreatScale::TANK.with_damage(3.0),
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            health_coefficient: 0.01,
            tick_health_coefficient: 0.0075,
            ..WarriorRow::NONE
        }),
        motion: Motion::Smash,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::ShieldCharge,
        name: "SHIELD CHARGE",
        description: "Charge at an enemy up to {range} away. On arrival, deal Physical damage to all enemies within {radius} and stun your target for {stun}.",
        empowered: "Impact damage is increased by {impact_bonus}.",
        cooldown: 15.0,
        range: 25.0,
        radius: 5.0,
        coefficient: 2.50,
        requires_line_of_sight: true,
        threat: ThreatScale::TANK.with_damage(4.0),
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Lunge,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::ShieldBash,
        name: "SHIELD BASH",
        description: "Bash your target with your shield, dealing Physical damage and interrupting its spellcasting.",
        empowered: "Also silences the target for {silence}.",
        cooldown: 15.0,
        range: 5.0,
        coefficient: 1.50,
        requires_line_of_sight: true,
        threat: ThreatScale::TANK.with_damage(3.0),
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Bash,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::ShieldBlock,
        name: "SHIELD BLOCK",
        description: "Raise your shield for {duration}, increasing your Block chance by {block}, plus more the higher your maximum Health. Attacks against you cannot be critical strikes while it lasts.",
        empowered: "Increases your Block chance by an additional {block_bonus}.",
        target: TargetMode::SelfCast,
        cooldown: 15.0,
        duration: 6.0,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Guard,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::PunctureBlood,
        name: "PUNCTURE BLOOD",
        description: "Sacrifice {health_cost} of your current Health to gain an absorb shield equal to {shield} of your maximum Health for {duration}.",
        empowered: "The shield is {shield_bonus} stronger. The Health cost is unchanged.",
        target: TargetMode::SelfCast,
        cooldown: 30.0,
        duration: 10.0,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::PowerUp,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Adrenaline,
        name: "ADRENALINE",
        description: "For {duration}, {share} of all damage you take is stored. When Adrenaline ends, the stored amount is restored to you as healing over {recovery}.",
        empowered: "Stores {empowered_share} of damage taken instead.",
        target: TargetMode::SelfCast,
        cooldown: 30.0,
        duration: 6.0,
        tick_interval: 1.0,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::PowerUp,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::BattleTrance,
        name: "BATTLE TRANCE",
        description: "For {duration}, every Block, Parry or Dodge restores {heal} of your maximum Health, up to {per_second} times per second, and generates twice as much Deflection.",
        target: TargetMode::SelfCast,
        cooldown: 45.0,
        duration: 10.0,
        class: ClassRow::Warrior(WarriorRow::NONE),
        motion: Motion::Battlecry,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::SpellReflect,
        name: "SPELL REFLECT",
        description: "Reflect enemy spells back at their caster for {duration}. While Spell Reflect is on cooldown, every Block, Parry or Dodge reduces its remaining cooldown by {cooldown_per_event}.",
        empowered: "The next spell you reflect deals {reflect_bonus} more damage.",
        target: TargetMode::SelfCast,
        cooldown: 45.0,
        duration: 6.0,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Guard,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Taunt,
        name: "TAUNT",
        description: "Taunt an enemy up to {range} away, forcing it to attack you for {duration} and raising your threat to the top of its threat table. Requires line of sight.",
        cooldown: 8.0,
        range: 30.0,
        duration: 3.0,
        requires_line_of_sight: true,
        turns_to_target: true,
        class: ClassRow::Warrior(WarriorRow::NONE),
        motion: Motion::Battlecry,
        ..BASE
    },
    AbilityDef {
        id: FireAbility::LastStand,
        name: "LAST STAND",
        description: "For {duration}, increase your maximum Health by at least {health}, more the higher your maximum Health, heal for the same amount, and take {reduction} less damage.",
        empowered: "Reduces damage taken by {empowered_reduction} instead.",
        target: TargetMode::SelfCast,
        cooldown: 90.0,
        duration: 15.0,
        class: ClassRow::Warrior(WarriorRow {
            deflection_cost: 20.0,
            ..WarriorRow::NONE
        }),
        motion: Motion::Battlecry,
        ..BASE
    },
];

/// The Warrior's template tokens: the magnitudes in [`super::tuning`] that a sentence names.
pub(crate) fn token(ability: FireAbility, key: &str) -> Option<TextValue> {
    use super::tuning::*;
    use crate::status::StatusKind;
    use TextValue::{Count, Degrees, Percent, Seconds};
    Some(match (ability, key) {
        (FireAbility::Recuperate, "heal_per_stack") => Percent(RECUPERATE_HEAL_PER_STACK),
        (FireAbility::Recuperate, "max_stacks") => {
            Count(f32::from(StatusKind::Recuperating.max_stacks()))
        }
        (FireAbility::WarStomp, "arc") => {
            Degrees((crate::ability::TARGET_CLEAVE_HALF_ANGLE * 2.0).to_degrees())
        }
        (FireAbility::WarStomp, "attack_speed") => Percent(WAR_STOMP_ATTACK_SPEED_PER_STACK),
        (FireAbility::WarStomp, "max_stacks") => {
            Count(f32::from(StatusKind::Sundered.max_stacks()))
        }
        (FireAbility::WarStomp, "armor") => Percent(WAR_STOMP_ARMOR_BONUS),
        (FireAbility::ColossusStrike, "impact_health") => {
            Percent(ability.definition().warrior().health_coefficient)
        }
        (FireAbility::ColossusStrike, "bleed_bonus") => Percent(COLOSSUS_BLEED_EMPOWERED),
        (FireAbility::ShieldCharge, "stun") => Seconds(SHIELD_CHARGE_STUN),
        (FireAbility::ShieldCharge, "impact_bonus") => Percent(SHIELD_CHARGE_EMPOWERED_DAMAGE),
        (FireAbility::ShieldBash, "silence") => Seconds(SHIELD_BASH_SILENCE),
        (FireAbility::ShieldBlock, "block") => Percent(SHIELD_BLOCK_BASE),
        (FireAbility::ShieldBlock, "block_bonus") => Percent(SHIELD_BLOCK_EMPOWERED),
        (FireAbility::PunctureBlood, "health_cost") => Percent(PUNCTURE_HEALTH_COST),
        (FireAbility::PunctureBlood, "shield") => Percent(PUNCTURE_SHIELD),
        (FireAbility::PunctureBlood, "shield_bonus") => Percent(PUNCTURE_EMPOWERED_SCALE - 1.0),
        (FireAbility::Adrenaline, "share") => Percent(ADRENALINE_SHARE),
        (FireAbility::Adrenaline, "empowered_share") => Percent(ADRENALINE_EMPOWERED_SHARE),
        (FireAbility::Adrenaline, "recovery") => Seconds(ADRENALINE_RECOVERY_SECONDS),
        (FireAbility::BattleTrance, "heal") => Percent(BATTLE_TRANCE_HEAL),
        (FireAbility::BattleTrance, "per_second") => {
            Count(f32::from(BATTLE_TRANCE_HEALS_PER_SECOND))
        }
        (FireAbility::SpellReflect, "cooldown_per_event") => {
            Seconds(SPELL_REFLECT_COOLDOWN_PER_EVENT)
        }
        (FireAbility::SpellReflect, "reflect_bonus") => Percent(SPELL_REFLECT_EMPOWERED_DAMAGE),
        (FireAbility::LastStand, "health") => Percent(LAST_STAND_BASE),
        (FireAbility::LastStand, "reduction") => Percent(LAST_STAND_REDUCTION),
        (FireAbility::LastStand, "empowered_reduction") => Percent(LAST_STAND_EMPOWERED_REDUCTION),
        _ => return None,
    })
}
