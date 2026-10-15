//! The Warrior's eight talents and what they mean.
//!
//! ```text
//! DEFS (defaults) / talents.json  ->  TalentGrid  ->  WarriorTalentEffects
//! ```
//!
//! [`WarriorTalentEffects`] is the third layer: every field is a number the Warrior's
//! executor reads (`warrior_exec`, the ability book), neutral when the talent is not bought.
//! No call site asks whether a talent is owned.

use super::{TalentDef, TalentGrid, TalentKey, hero_talents};
use crate::hero::Hero;
use crate::warrior::tuning::{FERVOR_DAMAGE_PER_STACK, FERVOR_DAMAGE_TAKEN_PER_STACK, VALOR_MAX};

/// Every Warrior talent, in table order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WarriorTalent {
    /// Battle Fervor needs fewer Valor.
    UnbrokenMomentum,
    /// War Stomp reaches further and stacks Sundered faster; Colossus Strike hits softer.
    Shockwave,
    /// Colossus Strike hits much harder on a nearly dead target, and recharges slower.
    Executioner,
    /// The last tick of Colossus Strike's bleed pays Valor.
    BloodiedValor,
    /// Battle Fervor trades damage dealt for damage taken.
    FerventGuard,
    /// Shield Charge holds two charges and hits softer.
    RelentlessCharge,
    /// Every Battle Fervor stack gained shortens Shield Block's cooldown.
    BulwarkRhythm,
    /// Last Stand grants Battle Fervor at once, and recharges slower.
    Unyielding,
}

impl TalentKey for WarriorTalent {
    const HERO: Hero = Hero::Warrior;
    const ALL: &'static [Self] = &[
        Self::UnbrokenMomentum,
        Self::Shockwave,
        Self::Executioner,
        Self::BloodiedValor,
        Self::FerventGuard,
        Self::RelentlessCharge,
        Self::BulwarkRhythm,
        Self::Unyielding,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// The built-in Warrior rows: the same numbers `assets/catalog/talents.json` ships.
pub const DEFS: [TalentDef; 8] = [
    TalentDef {
        key: "unbroken_momentum",
        name: "UNBROKEN MOMENTUM",
        tier: 1,
        cost: 3,
        description: "BATTLE FERVOR NEEDS {valor_needed} VALOR INSTEAD OF 5",
        values: &[("valor_needed", 4.0)],
    },
    TalentDef {
        key: "shockwave",
        name: "SHOCKWAVE",
        tier: 1,
        cost: 3,
        description: "WAR STOMP RADIUS +{radius_bonus%}% AND IT APPLIES {extra_stacks} EXTRA SUNDERED STACK. COLOSSUS STRIKE DAMAGE -{colossus_penalty%}%",
        values: &[
            ("radius_bonus", 0.4),
            ("extra_stacks", 1.0),
            ("colossus_penalty", 0.1),
        ],
    },
    TalentDef {
        key: "executioner",
        name: "EXECUTIONER",
        tier: 2,
        cost: 4,
        description: "COLOSSUS STRIKE DEALS +{bonus%}% TO TARGETS BELOW {threshold%}% HEALTH. ITS COOLDOWN +{cooldown_penalty} S",
        values: &[
            ("bonus", 0.6),
            ("threshold", 0.35),
            ("cooldown_penalty", 2.0),
        ],
    },
    TalentDef {
        key: "bloodied_valor",
        name: "BLOODIED VALOR",
        tier: 2,
        cost: 4,
        description: "THE FINAL TICK OF THE COLOSSUS STRIKE BLEED GRANTS +{valor} VALOR",
        values: &[("valor", 1.0)],
    },
    TalentDef {
        key: "fervent_guard",
        name: "FERVENT GUARD",
        tier: 3,
        cost: 4,
        description: "BATTLE FERVOR PER STACK BECOMES +{damage%}% DAMAGE DEALT AND -{taken%}% DAMAGE TAKEN",
        values: &[("damage", 0.06), ("taken", 0.06)],
    },
    TalentDef {
        key: "relentless_charge",
        name: "RELENTLESS CHARGE",
        tier: 3,
        cost: 4,
        description: "SHIELD CHARGE HAS {charges} CHARGES. ITS IMPACT DAMAGE -{impact_penalty%}%",
        values: &[("charges", 2.0), ("impact_penalty", 0.25)],
    },
    TalentDef {
        key: "bulwark_rhythm",
        name: "BULWARK RHYTHM",
        tier: 4,
        cost: 5,
        description: "EACH BATTLE FERVOR STACK GAINED CUTS SHIELD BLOCK REMAINING COOLDOWN BY {cooldown_cut} S",
        values: &[("cooldown_cut", 5.0)],
    },
    TalentDef {
        key: "unyielding",
        name: "UNYIELDING",
        tier: 4,
        cost: 5,
        description: "LAST STAND GRANTS {fervor_stacks} BATTLE FERVOR STACKS AT ONCE. ITS COOLDOWN +{cooldown_penalty} S",
        values: &[("fervor_stacks", 2.0), ("cooldown_penalty", 30.0)],
    },
];

/// What the Warrior's bought talents mean. Neutral values when nothing is bought.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarriorTalentEffects {
    /// Unbroken Momentum: the Valor that fills the bar and becomes Battle Fervor.
    pub valor_capacity: u8,
    /// Shockwave: multiplier on War Stomp's reach.
    pub war_stomp_range_scale: f32,
    /// Shockwave: Sundered stacks War Stomp applies beyond the first.
    pub war_stomp_extra_stacks: u8,
    /// Shockwave: multiplier on Colossus Strike's damage, impact and bleed.
    pub colossus_damage_scale: f32,
    /// Executioner: extra Colossus Strike damage against a target under the threshold, as a
    /// fraction. Zero: off.
    pub executioner_bonus: f32,
    /// Executioner: the health fraction under which the bonus applies.
    pub executioner_threshold: f32,
    /// Executioner: seconds added to Colossus Strike's cooldown.
    pub colossus_cooldown_extra: f32,
    /// Bloodied Valor: Valor the bleed's final tick pays. Zero: off.
    pub bloodied_valor: u8,
    /// Fervent Guard: Battle Fervor's damage dealt per stack, as a fraction.
    pub fervor_damage_per_stack: f32,
    /// Fervent Guard: Battle Fervor's damage taken lost per stack, as a fraction.
    pub fervor_taken_per_stack: f32,
    /// Relentless Charge: charges Shield Charge holds beyond the first.
    pub shield_charge_extra_charges: u8,
    /// Relentless Charge: multiplier on Shield Charge's impact damage.
    pub shield_charge_impact_scale: f32,
    /// Bulwark Rhythm: seconds off Shield Block's remaining cooldown per Fervor stack gained.
    pub bulwark_cooldown_cut: f32,
    /// Unyielding: Battle Fervor stacks Last Stand grants at once. Zero: off.
    pub last_stand_fervor_stacks: u8,
    /// Unyielding: seconds added to Last Stand's cooldown.
    pub last_stand_cooldown_extra: f32,
}

impl Default for WarriorTalentEffects {
    fn default() -> Self {
        Self::from_grid(&TalentGrid::with_points(Hero::Warrior, 0))
    }
}

impl WarriorTalentEffects {
    /// Resolves a grid into the numbers gameplay uses.
    #[must_use]
    pub fn from_grid(grid: &TalentGrid) -> Self {
        let table = hero_talents(Hero::Warrior);
        let on = |talent: WarriorTalent| grid.has(talent);
        let v =
            |talent: WarriorTalent, name: &str| table.talents[talent.index()].values.value(name);
        use WarriorTalent::{
            BloodiedValor, BulwarkRhythm, Executioner, FerventGuard, RelentlessCharge, Shockwave,
            UnbrokenMomentum, Unyielding,
        };
        let mut fx = Self {
            valor_capacity: VALOR_MAX,
            war_stomp_range_scale: 1.0,
            war_stomp_extra_stacks: 0,
            colossus_damage_scale: 1.0,
            executioner_bonus: 0.0,
            executioner_threshold: 0.0,
            colossus_cooldown_extra: 0.0,
            bloodied_valor: 0,
            fervor_damage_per_stack: FERVOR_DAMAGE_PER_STACK,
            fervor_taken_per_stack: FERVOR_DAMAGE_TAKEN_PER_STACK,
            shield_charge_extra_charges: 0,
            shield_charge_impact_scale: 1.0,
            bulwark_cooldown_cut: 0.0,
            last_stand_fervor_stacks: 0,
            last_stand_cooldown_extra: 0.0,
        };
        if on(UnbrokenMomentum) {
            fx.valor_capacity =
                (v(UnbrokenMomentum, "valor_needed").round().max(1.0) as u8).min(VALOR_MAX);
        }
        if on(Shockwave) {
            fx.war_stomp_range_scale = 1.0 + v(Shockwave, "radius_bonus");
            fx.war_stomp_extra_stacks = v(Shockwave, "extra_stacks").round().max(0.0) as u8;
            fx.colossus_damage_scale = 1.0 - v(Shockwave, "colossus_penalty");
        }
        if on(Executioner) {
            fx.executioner_bonus = v(Executioner, "bonus");
            fx.executioner_threshold = v(Executioner, "threshold");
            fx.colossus_cooldown_extra = v(Executioner, "cooldown_penalty");
        }
        if on(BloodiedValor) {
            fx.bloodied_valor = v(BloodiedValor, "valor").round().max(0.0) as u8;
        }
        if on(FerventGuard) {
            fx.fervor_damage_per_stack = v(FerventGuard, "damage");
            fx.fervor_taken_per_stack = v(FerventGuard, "taken");
        }
        if on(RelentlessCharge) {
            fx.shield_charge_extra_charges =
                (v(RelentlessCharge, "charges").round().max(1.0) as u8).saturating_sub(1);
            fx.shield_charge_impact_scale = 1.0 - v(RelentlessCharge, "impact_penalty");
        }
        if on(BulwarkRhythm) {
            fx.bulwark_cooldown_cut = v(BulwarkRhythm, "cooldown_cut");
        }
        if on(Unyielding) {
            fx.last_stand_fervor_stacks = v(Unyielding, "fervor_stacks").round().max(0.0) as u8;
            fx.last_stand_cooldown_extra = v(Unyielding, "cooldown_penalty");
        }
        fx
    }

    /// The factor Fervent Guard puts on the damage Battle Fervor already adds, for `stacks`.
    ///
    /// The status registry row states the untalented numbers; the talent replaces them, so
    /// the correction is the ratio of the two. One when the talent is not bought.
    #[must_use]
    pub fn fervor_dealt_correction(&self, stacks: u8) -> f32 {
        let s = f32::from(stacks);
        (1.0 + self.fervor_damage_per_stack * s) / (1.0 + FERVOR_DAMAGE_PER_STACK * s)
    }

    /// The same for damage taken.
    #[must_use]
    pub fn fervor_taken_correction(&self, stacks: u8) -> f32 {
        let s = f32::from(stacks);
        (1.0 - self.fervor_taken_per_stack * s).max(0.0)
            / (1.0 - FERVOR_DAMAGE_TAKEN_PER_STACK * s).max(1e-3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_warrior_talent_changes_something_and_resets_cleanly() {
        let none = WarriorTalentEffects::default();
        for talent in WarriorTalent::ALL {
            let grid = TalentGrid::from_talents(&[*talent]);
            let fx = WarriorTalentEffects::from_grid(&grid);
            assert_ne!(fx, none, "{talent:?} changed nothing");
            let mut refunded = grid;
            refunded.reset();
            assert_eq!(WarriorTalentEffects::from_grid(&refunded), none);
        }
    }

    #[test]
    fn costs_follow_the_tiers() {
        let costs: Vec<u32> = WarriorTalent::ALL.iter().map(|t| t.spec().cost).collect();
        assert_eq!(costs, vec![3, 3, 4, 4, 4, 4, 5, 5]);
    }

    #[test]
    fn the_corrections_are_neutral_without_the_talent() {
        let none = WarriorTalentEffects::default();
        for stacks in 0..=3 {
            assert!((none.fervor_dealt_correction(stacks) - 1.0).abs() < 1e-6);
            assert!((none.fervor_taken_correction(stacks) - 1.0).abs() < 1e-6);
        }
    }
}
