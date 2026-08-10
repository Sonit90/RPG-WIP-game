//! The combat stats every FireMage number is derived from, and the rule that they are
//! never stored.
//!
//! ```text
//! base + Heat threshold + Fire Mastery + Inferno State + talents  ->  Stats
//!                                                                      |
//!                    cast times, the global cooldown, Ignite's interval, crit rolls
//! ```
//!
//! ## Nothing snapshots
//!
//! [`Stats`] is *derived*, recomputed by [`super::FireMage::stats`] every time anything asks
//! for it. No damage-over-time, no zone and no pending impact keeps a copy: an Ignite
//! applied at 10 % haste that later sees 30 % haste ticks faster on its next tick, and one
//! applied without a critical strike buff starts critting the moment the buff lands. That
//! is a design requirement of the class, and it is enforced structurally — there is no field
//! anywhere in [`super::ignite`] or [`super::zone`] that a snapshot could live in.

/// Spell power a FireMage has with no gear at all.
///
/// A hundred, so an ability's coefficient reads directly as damage: a 260 % spell power
/// Flame Lance hits for 260 before Heat, critical strikes and talents.
pub const BASE_SPELL_POWER: f32 = 100.0;

/// Haste a FireMage has with no gear and no buffs.
pub const BASE_HASTE: f32 = 0.0;

/// Chance any fire spell critically strikes, before buffs.
pub const BASE_CRIT_CHANCE: f32 = 0.15;

/// Damage multiplier of a critical strike, before talents.
pub const BASE_CRIT_DAMAGE: f32 = 2.0;

/// Which stat Fire Mastery specialises into.
///
/// The choice is a specialisation rather than a purchase: it decides both what the active
/// ability grants and which stat the 70+ Heat threshold and the Inferno State hand over.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FireMastery {
    /// Haste: faster casts, a shorter global cooldown, faster Ignite ticks.
    #[default]
    Haste,
    /// Critical strike: more crits everywhere, including on burns already running.
    Crit,
}

impl FireMastery {
    /// Both specialisations, in menu order.
    pub const ALL: [Self; 2] = [Self::Haste, Self::Crit];

    /// Uppercase name; the UI font has no lowercase glyphs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Haste => "HASTE MASTERY",
            Self::Crit => "CRIT MASTERY",
        }
    }
}

/// What the character is wearing, as flat additions to the base stats.
///
/// Gear is not a snapshot: it is one of the inputs [`super::FireMage::stats`] adds up on
/// every call, exactly like Heat's band and Fire Mastery. A fully geared FireMage is a
/// `Gear` value, not a different code path, which is what lets the combat simulator balance
/// the class at the gear level it will actually be played at instead of at naked base stats.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Gear {
    /// Spell power on top of the base.
    pub spell_power: f32,
    /// Haste on top of the base, as a fraction.
    pub haste: f32,
    /// Critical strike chance on top of the base, as a fraction.
    pub crit_chance: f32,
    /// Critical strike damage on top of the base multiplier.
    pub crit_damage: f32,
}

impl Gear {
    /// Nothing equipped.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            spell_power: 0.0,
            haste: 0.0,
            crit_chance: 0.0,
            crit_damage: 0.0,
        }
    }

    /// Gear that brings the character to `haste` and `crit` *in total*.
    ///
    /// Stated as totals because that is how a player reads a character sheet: "45 % haste"
    /// means the number on the sheet, not the number added to the fifteen per cent everyone
    /// starts with. The subtraction happens here, once.
    #[must_use]
    pub fn totalling(haste: f32, crit: f32) -> Self {
        Self {
            spell_power: 0.0,
            haste: (haste - BASE_HASTE).max(0.0),
            crit_chance: (crit - BASE_CRIT_CHANCE).max(0.0),
            crit_damage: 0.0,
        }
    }
}

/// Every stat that decides what a point of damage becomes, right now.
///
/// Produced on demand and thrown away. Holding one across a tick would be a snapshot, which
/// is the one thing this class must not do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Spell power: what every ability coefficient is a percentage of.
    pub spell_power: f32,
    /// Haste, as a fraction: `0.15` is +15 %.
    pub haste: f32,
    /// Critical strike chance, as a fraction.
    pub crit_chance: f32,
    /// Damage multiplier of a critical strike.
    pub crit_damage: f32,
}

impl Default for Stats {
    fn default() -> Self {
        Self::base()
    }
}

impl Stats {
    /// A FireMage with no gear, no buffs, no Heat and no talents.
    #[must_use]
    pub const fn base() -> Self {
        Self {
            spell_power: BASE_SPELL_POWER,
            haste: BASE_HASTE,
            crit_chance: BASE_CRIT_CHANCE,
            crit_damage: BASE_CRIT_DAMAGE,
        }
    }

    /// What a base cast time of `seconds` currently takes.
    #[must_use]
    pub fn cast_time(&self, seconds: f32) -> f32 {
        (seconds / (1.0 + self.haste)).max(0.0)
    }

    /// What a base interval of `seconds` currently is - a tick, a global cooldown.
    #[must_use]
    pub fn interval(&self, seconds: f32) -> f32 {
        // Floored well above zero: an interval that reached zero would fire an unbounded
        // number of ticks in one step.
        (seconds / (1.0 + self.haste)).max(0.01)
    }

    /// Crit chance, clamped to something a roll can use.
    #[must_use]
    pub fn crit(&self) -> f32 {
        self.crit_chance.clamp(0.0, 1.0)
    }

    /// Adds what the character is wearing.
    pub fn equip(&mut self, gear: &Gear) {
        self.spell_power += gear.spell_power;
        self.haste += gear.haste;
        self.crit_chance += gear.crit_chance;
        self.crit_damage += gear.crit_damage;
    }

    /// Adds haste, crit or both, according to a Fire Mastery specialisation.
    ///
    /// `dual` splits the grant across both stats instead of putting all of it in one, which
    /// is what the Dual Fire Mastery talent buys.
    pub fn grant(&mut self, mastery: FireMastery, amount: f32, dual: bool) {
        if dual {
            self.haste += amount;
            self.crit_chance += amount;
            return;
        }
        match mastery {
            FireMastery::Haste => self.haste += amount,
            FireMastery::Crit => self.crit_chance += amount,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haste_shortens_casts_and_intervals_by_the_same_rule() {
        let mut stats = Stats::base();
        assert!((stats.cast_time(3.0) - 3.0).abs() < 1e-6);
        assert!((stats.interval(2.0) - 2.0).abs() < 1e-6);

        stats.haste = 1.0;
        assert!((stats.cast_time(3.0) - 1.5).abs() < 1e-6);
        assert!((stats.interval(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_interval_never_reaches_zero() {
        let stats = Stats {
            haste: 1_000_000.0,
            ..Stats::base()
        };
        assert!(stats.interval(2.0) > 0.0);
    }

    #[test]
    fn gear_is_stated_as_the_totals_a_character_sheet_shows() {
        let geared = Gear::totalling(0.45, 0.20);
        let mut stats = Stats::base();
        stats.equip(&geared);
        assert!((stats.haste - 0.45).abs() < 1e-6);
        assert!((stats.crit_chance - 0.20).abs() < 1e-6);

        // And nothing is subtracted below zero for a stat the base already exceeds.
        let poor = Gear::totalling(0.0, 0.0);
        assert_eq!(poor.crit_chance, 0.0);
    }

    #[test]
    fn a_specialisation_puts_its_grant_where_it_belongs() {
        let mut haste = Stats::base();
        haste.grant(FireMastery::Haste, 0.15, false);
        assert!((haste.haste - 0.15).abs() < 1e-6);
        assert!((haste.crit_chance - BASE_CRIT_CHANCE).abs() < 1e-6);

        let mut crit = Stats::base();
        crit.grant(FireMastery::Crit, 0.15, false);
        assert!((crit.crit_chance - (BASE_CRIT_CHANCE + 0.15)).abs() < 1e-6);
        assert!((crit.haste - BASE_HASTE).abs() < 1e-6);

        // Dual pays both, and the caller passes the halved amount.
        let mut dual = Stats::base();
        dual.grant(FireMastery::Haste, 0.10, true);
        assert!((dual.haste - 0.10).abs() < 1e-6);
        assert!((dual.crit_chance - (BASE_CRIT_CHANCE + 0.10)).abs() < 1e-6);
    }
}
