//! The FireMage's rows of the ability table.
//!
//! Every number the class's abilities are made of: cast times, cooldowns, charges, ranges,
//! areas, Heat and the spell-power coefficients. The shared shape of a row, the identity enum
//! and the cooldown runtime live in [`crate::ability`]; this file is only the FireMage's data.

use crate::ability::text::TextValue;
use crate::ability::{AbilityDef, ClassRow, FireAbility, FireMageRow, TargetMode, ThreatScale};
use crate::combat::DamageType;

/// What every FireMage row starts from: fire, damage-dealer threat, and a [`FireMageRow`].
const BASE: AbilityDef = AbilityDef {
    damage_type: DamageType::Fire,
    threat: ThreatScale::DAMAGE_DEALER,
    class: ClassRow::FireMage(FireMageRow::NONE),
    ..AbilityDef::BASE
};

/// The FireMage's abilities, in [`FireAbility`] order: the bar, then the Ultimate.
pub(crate) const ROWS: [AbilityDef; 11] = [
    AbilityDef {
        id: FireAbility::Firebolt,
        name: "FIREBOLT",
        description: "Hurl a bolt of fire at your target, dealing Fire damage. Always applies Ignite to a target that is not burning; otherwise it has a {ignite_chance} chance to add a stack.",
        cast_time: 1.5,
        range: 35.0,
        coefficient: 1.5,
        requires_line_of_sight: true,
        class: ClassRow::FireMage(FireMageRow {
            ignite_stacks: 1,
            ignite_chance: 0.50,
            heat: 3.0,
            energy: 1.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::FlameLance,
        name: "FLAME LANCE",
        description: "Launch a lance of fire at your target, dealing Fire damage and applying {ignite_stacks} stacks of Ignite. A critical strike reduces the remaining cooldown of Pyroblast by {crit_reduction}.",
        cast_time: 1.5,
        cooldown: 8.0,
        charges: 2,
        range: 35.0,
        coefficient: 2.60,
        requires_line_of_sight: true,
        class: ClassRow::FireMage(FireMageRow {
            ignite_stacks: 2,
            ignite_chance: 1.0,
            heat: 4.0,
            energy: 2.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::FlameCone,
        name: "FLAME CONE",
        description: "Breathe fire in a cone {range} long, dealing Fire damage and applying {ignite_stacks} stacks of Ignite to every enemy caught in it.",
        target: TargetMode::Cone,
        cooldown: 10.0,
        range: 12.0,
        coefficient: 1.40,
        class: ClassRow::FireMage(FireMageRow {
            ignite_stacks: 2,
            ignite_chance: 1.0,
            heat: 3.0,
            energy: 2.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Meteor,
        name: "METEOR",
        description: "Call down a meteor on the chosen location. After {impact_delay}, it deals Fire damage to all enemies within {radius}, plus {ignite_bonus} more for every stack of Ignite on each of them.",
        target: TargetMode::Ground,
        cooldown: 20.0,
        range: 35.0,
        radius: 5.0,
        impact_delay: 1.0,
        coefficient: 5.00,
        class: ClassRow::FireMage(FireMageRow {
            ignite_bonus_per_stack: 0.15,
            heat: 5.0,
            energy: 5.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Pyroblast,
        name: "PYROBLAST",
        description: "Hurl an immense fiery boulder at your target, dealing heavy Fire damage, plus {ignite_bonus} more for every stack of Ignite on the target.",
        cast_time: 3.0,
        cooldown: 30.0,
        range: 35.0,
        coefficient: 6.50,
        requires_line_of_sight: true,
        class: ClassRow::FireMage(FireMageRow {
            ignite_bonus_per_stack: 0.15,
            heat: 5.0,
            energy: 10.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Firestorm,
        name: "FIRESTORM",
        description: "Set a strip of ground {range} long and {width} wide ablaze for {duration}, burning every enemy inside it every {tick_interval}. Each tick has a {ignite_chance} chance to apply Ignite.",
        target: TargetMode::Line,
        cooldown: 24.0,
        range: 20.0,
        width: 10.0,
        duration: 6.0,
        tick_coefficient: 0.70,
        tick_interval: 0.5,
        class: ClassRow::FireMage(FireMageRow {
            ignite_stacks: 1,
            ignite_chance: 0.15,
            heat: 5.0,
            energy: 8.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Detonate,
        name: "DETONATE",
        description: "Detonate a fully stacked Ignite on your target and every burning enemy within {radius} of it, consuming each burn to instantly deal {fraction} of its remaining damage. Requires maximum Ignite stacks on the target.",
        range: 35.0,
        radius: 10.0,
        requires_line_of_sight: true,
        cooldown_hasted: false,
        class: ClassRow::FireMage(FireMageRow {
            energy: 3.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::FireMastery,
        name: "FIRE MASTERY",
        description: "Gain {bonus} Haste or Critical Strike, whichever you specialize in, for {duration}.",
        target: TargetMode::SelfCast,
        cooldown: 60.0,
        duration: 20.0,
        class: ClassRow::FireMage(FireMageRow::NONE),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::FlameShield,
        name: "FLAME SHIELD",
        description: "Wrap yourself in flame, reducing all damage you take by {reduction} for {duration}.",
        target: TargetMode::SelfCast,
        cooldown: 30.0,
        duration: 5.0,
        class: ClassRow::FireMage(FireMageRow::NONE),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Blink,
        name: "BLINK",
        description: "Teleport up to {range} forward.",
        target: TargetMode::Movement,
        cooldown: 20.0,
        range: 10.0,
        class: ClassRow::FireMage(FireMageRow::NONE),
        ..BASE
    },
    AbilityDef {
        id: FireAbility::Cataclysm,
        name: "CATACLYSM",
        description: "Not yet available.",
        target: TargetMode::Ground,
        cast_time: 1.0,
        range: 35.0,
        radius: 10.0,
        duration: 8.0,
        impact_delay: 0.5,
        coefficient: 12.00,
        tick_coefficient: 1.50,
        tick_interval: 1.0,
        class: ClassRow::FireMage(FireMageRow {
            ignite_stacks: 3,
            ignite_chance: 1.0,
            energy_cost: 100.0,
            ..FireMageRow::NONE
        }),
        ..BASE
    },
];

/// Ignite: not an ability but the row the burn's damage is credited to. Last in the table.
pub(crate) const IGNITE: AbilityDef = AbilityDef {
    id: FireAbility::Ignite,
    name: "IGNITE",
    description: "A stacking burn that deals Fire damage every {tick_interval} for {duration}. Each stack adds to the damage.",
    duration: 30.0,
    tick_coefficient: 0.20,
    tick_interval: 2.0,
    class: ClassRow::FireMage(FireMageRow::NONE),
    ..BASE
};

/// The FireMage's template tokens: the rules in the executor and the talents that a sentence
/// names.
pub(crate) fn token(ability: FireAbility, key: &str) -> Option<TextValue> {
    use TextValue::{Percent, Seconds};
    Some(match (ability, key) {
        (FireAbility::FlameLance, "crit_reduction") => {
            Seconds(crate::firemage::talent::CRIT_FEEDBACK_BASE)
        }
        (FireAbility::Detonate, "fraction") => Percent(crate::sim::DETONATE_FRACTION),
        (FireAbility::FireMastery, "bonus") => {
            Percent(crate::firemage::talent::MASTERY_ACTIVE_SINGLE)
        }
        (FireAbility::FlameShield, "reduction") => Percent(crate::firemage::FLAME_SHIELD_REDUCTION),
        _ => return None,
    })
}
