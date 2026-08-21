//! Every stat a character has, as one table rather than as fields.
//!
//! ```text
//! Stat          the seven names, and which group each belongs to
//!   |
//!   v
//! StatBlock     one f32 per name. What a hero is born with, what an item grants,
//!               and what the character sheet shows are all this one type.
//!   |
//!   v
//! derived       max health, and the percentages a rating converts to
//! ```
//!
//! # Why one block instead of a struct of named fields
//!
//! A struct with `strength`, `intellect`, `stamina`, `crit`, `haste`, `versatility` and
//! `spirit` fields makes adding an eighth stat a change in every place that adds two of them
//! up: the item table, the sheet, the tooltip, the panel. A block indexed by [`Stat`] makes
//! it a variant and a row of text, and the summing code never changes at all. That is the
//! whole of "data-driven" here — the aggregation is written once, against the enum.
//!
//! # Ratings and percentages
//!
//! The four secondary stats are *ratings*: whole points that convert to a percentage at a
//! fixed exchange rate ([`RATING_PER_PERCENT`]). The rating is what an item grants and what
//! is summed; the percentage is what gameplay multiplies by and what the sheet prints beside
//! it. The conversion lives here, once, so a tooltip and a damage calculation cannot
//! disagree about what a rating of 350 means.
//!
//! Every secondary stat currently starts at zero on every hero and on every item, so every
//! percentage this module can produce is currently `0.0`. The exchange rate exists so that
//! turning the mechanic on later is a change to the item table and nothing else.

/// Which group a stat is shown and reasoned about in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatGroup {
    /// The stats a character is built out of: the main stat and Stamina.
    Primary,
    /// The ratings gear grants: Critical Strike, Haste, Versatility, Spirit.
    Secondary,
}

/// One stat.
///
/// The order is the order the character sheet lists them in, primaries first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stat {
    /// The Warrior's main stat.
    Strength,
    /// The FireMage's and the Druid's main stat.
    Intellect,
    /// Maximum health. Every hero has it; see [`health_from_stamina`].
    Stamina,
    /// Chance to critically strike, as a rating.
    CriticalStrike,
    /// Faster damage-over-time ticks and a shorter global cooldown, as a rating.
    Haste,
    /// More damage dealt, more healing done and less damage taken, as a rating.
    Versatility,
    /// The chance to bank a point of Ultimate and gain a stacking main-stat buff, as a
    /// rating. See [`super::spirit`].
    Spirit,
}

impl Stat {
    /// Every stat, in character-sheet order.
    pub const ALL: [Self; 7] = [
        Self::Strength,
        Self::Intellect,
        Self::Stamina,
        Self::CriticalStrike,
        Self::Haste,
        Self::Versatility,
        Self::Spirit,
    ];

    /// How many stats there are. The width of a [`StatBlock`].
    pub const COUNT: usize = Self::ALL.len();

    /// Dense index into a [`StatBlock`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The stat at `index`, if there is one.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// Uppercase display name. The UI font has no lowercase glyphs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Strength => "STRENGTH",
            Self::Intellect => "INTELLECT",
            Self::Stamina => "STAMINA",
            Self::CriticalStrike => "CRITICAL STRIKE",
            Self::Haste => "HASTE",
            Self::Versatility => "VERSATILITY",
            Self::Spirit => "SPIRIT",
        }
    }

    /// The name a saved file uses. Stable, and deliberately not the display name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Strength => "strength",
            Self::Intellect => "intellect",
            Self::Stamina => "stamina",
            Self::CriticalStrike => "critical_strike",
            Self::Haste => "haste",
            Self::Versatility => "versatility",
            Self::Spirit => "spirit",
        }
    }

    /// Which group the sheet lists this stat under.
    #[must_use]
    pub fn group(self) -> StatGroup {
        match self {
            Self::Strength | Self::Intellect | Self::Stamina => StatGroup::Primary,
            Self::CriticalStrike | Self::Haste | Self::Versatility | Self::Spirit => {
                StatGroup::Secondary
            }
        }
    }

    /// Whether this stat is a rating that also reads as a percentage.
    #[must_use]
    pub fn is_rating(self) -> bool {
        matches!(self.group(), StatGroup::Secondary)
    }

    /// One line of what the stat does, for a tooltip and for the sheet.
    ///
    /// Written as what the stat *is*, including the parts no system reads yet: the
    /// definition is the contract the mechanic will be built against, and it is the same
    /// sentence whether or not anything is currently multiplying by it.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Strength => "MAIN STAT OF THE WARRIOR. INCREASES ATTACK POWER.",
            Self::Intellect => "MAIN STAT OF THE FIREMAGE AND DRUID. INCREASES SPELL POWER.",
            Self::Stamina => "INCREASES MAXIMUM HEALTH.",
            Self::CriticalStrike => "INCREASES THE CHANCE TO CRITICALLY STRIKE.",
            Self::Haste => "SPEEDS UP DAMAGE OVER TIME AND SHORTENS THE GLOBAL COOLDOWN.",
            Self::Versatility => "INCREASES DAMAGE AND HEALING DONE. REDUCES DAMAGE TAKEN.",
            Self::Spirit => "CHANCE TO GAIN AN ULTIMATE POINT AND A STACKING MAIN-STAT BUFF.",
        }
    }
}

/// Health one point of Stamina is worth.
///
/// Ten, chosen so the base Stamina every hero starts with lands exactly on
/// [`engine_core::tuning::player::MAX_HEALTH`] — the number every enemy attack in the game
/// is already tuned against. A test in [`super`] holds the two together, so changing either
/// without the other is a test failure rather than a silently rebalanced roster.
pub const HEALTH_PER_STAMINA: f32 = 10.0;

/// Rating that buys one per cent of a secondary stat.
///
/// One flat exchange rate for all four rather than one per stat: no gear in the game grants
/// any of them yet, so a per-stat curve would be four invented numbers where one placeholder
/// does. It is the single place to change when there is.
pub const RATING_PER_PERCENT: f32 = 100.0;

/// Maximum health for a given amount of Stamina.
///
/// The one definition. Combat, the character sheet and the unit frame all read it, so there
/// is no second answer to "how much health does this character have".
#[must_use]
pub fn health_from_stamina(stamina: f32) -> f32 {
    (stamina.max(0.0) * HEALTH_PER_STAMINA).max(1.0)
}

/// A value for every stat.
///
/// What a hero starts with, what an item grants and what the sheet displays are all this
/// type, which is what makes aggregation a single call rather than seven additions.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StatBlock {
    /// One value per [`Stat`], indexed by [`Stat::index`].
    values: [f32; Stat::COUNT],
}

impl StatBlock {
    /// Every stat at zero.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            values: [0.0; Stat::COUNT],
        }
    }

    /// A block with one stat set.
    #[must_use]
    pub fn of(stat: Stat, value: f32) -> Self {
        let mut block = Self::zero();
        block.set(stat, value);
        block
    }

    /// The same, in a `const` context: what the item table is built out of.
    ///
    /// Separate from [`StatBlock::of`] because indexing a `[f32; N]` is const and
    /// [`StatBlock::set`] is not — it refuses a non-finite value, which needs a float
    /// comparison. A literal in the item table is written by hand and checked by a test, so
    /// nothing is lost by the const path not repeating the guard.
    #[must_use]
    pub const fn of_const(stat: Stat, value: f32) -> Self {
        let mut values = [0.0; Stat::COUNT];
        values[stat as usize] = value;
        Self { values }
    }

    /// What `stat` currently reads.
    #[must_use]
    pub fn get(&self, stat: Stat) -> f32 {
        self.values[stat.index()]
    }

    /// Replaces one stat.
    ///
    /// A non-finite value is refused rather than stored: one bad item would otherwise turn
    /// every derived number in the game — health, damage, a bar's fraction — into `NaN`.
    pub fn set(&mut self, stat: Stat, value: f32) {
        if value.is_finite() {
            self.values[stat.index()] = value;
        }
    }

    /// Adds to one stat.
    pub fn add(&mut self, stat: Stat, value: f32) {
        self.set(stat, self.get(stat) + value);
    }

    /// Adds every stat of `other` to this one. **The aggregation, written once.**
    pub fn add_block(&mut self, other: &Self) {
        for stat in Stat::ALL {
            self.add(stat, other.get(stat));
        }
    }

    /// Multiplies one stat by `factor`. What a percentage buff does.
    pub fn scale(&mut self, stat: Stat, factor: f32) {
        self.set(stat, self.get(stat) * factor);
    }

    /// Whether every stat is zero. What "this item grants no combat stats" means.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.iter().all(|value| *value == 0.0)
    }

    /// Every stat with a non-zero value, in [`Stat::ALL`] order.
    pub fn entries(&self) -> impl Iterator<Item = (Stat, f32)> + '_ {
        Stat::ALL
            .into_iter()
            .map(|stat| (stat, self.get(stat)))
            .filter(|(_, value)| *value != 0.0)
    }

    /// What `stat` is worth as a fraction, for a rating.
    ///
    /// `0.15` means fifteen per cent. A primary stat has no percentage and answers zero,
    /// which saves every caller from asking which kind of stat it is holding.
    #[must_use]
    pub fn percent(&self, stat: Stat) -> f32 {
        if !stat.is_rating() {
            return 0.0;
        }
        self.get(stat) / (RATING_PER_PERCENT * 100.0)
    }
}

impl core::ops::Add for StatBlock {
    type Output = Self;

    fn add(mut self, other: Self) -> Self {
        self.add_block(&other);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stat_has_a_distinct_index_a_name_and_a_key() {
        for (index, stat) in Stat::ALL.into_iter().enumerate() {
            assert_eq!(stat.index(), index);
            assert_eq!(Stat::from_index(index), Some(stat));
            assert!(!stat.name().is_empty());
            assert!(!stat.description().is_empty());
        }
        for stat in Stat::ALL {
            let clashes = Stat::ALL
                .into_iter()
                .filter(|other| other.key() == stat.key());
            assert_eq!(clashes.count(), 1, "{} shares a key", stat.name());
        }
    }

    #[test]
    fn the_groups_split_the_seven_into_three_and_four() {
        let primary = Stat::ALL
            .into_iter()
            .filter(|stat| stat.group() == StatGroup::Primary)
            .count();
        let secondary = Stat::ALL
            .into_iter()
            .filter(|stat| stat.group() == StatGroup::Secondary)
            .count();
        assert_eq!((primary, secondary), (3, 4));
    }

    #[test]
    fn blocks_add_stat_by_stat() {
        let mut block = StatBlock::of(Stat::Intellect, 100.0);
        block.add_block(&StatBlock::of(Stat::Stamina, 50.0));
        block.add_block(&StatBlock::of(Stat::Intellect, 10.0));
        assert_eq!(block.get(Stat::Intellect), 110.0);
        assert_eq!(block.get(Stat::Stamina), 50.0);
        assert_eq!(block.get(Stat::Haste), 0.0);
        assert!(!block.is_empty());
        assert!(StatBlock::zero().is_empty());
    }

    #[test]
    fn a_non_finite_value_never_reaches_a_block() {
        let mut block = StatBlock::zero();
        block.set(Stat::Stamina, f32::NAN);
        block.add(Stat::Stamina, f32::INFINITY);
        assert_eq!(block.get(Stat::Stamina), 0.0);
    }

    #[test]
    fn a_rating_reads_as_a_percentage_and_a_primary_stat_does_not() {
        let mut block = StatBlock::zero();
        block.set(Stat::Haste, RATING_PER_PERCENT * 15.0);
        assert!((block.percent(Stat::Haste) - 0.15).abs() < 1e-6);

        block.set(Stat::Intellect, 500.0);
        assert_eq!(block.percent(Stat::Intellect), 0.0);
    }

    #[test]
    fn health_comes_from_stamina_and_never_reaches_zero() {
        assert_eq!(health_from_stamina(50.0), 500.0);
        assert!(health_from_stamina(0.0) > 0.0);
        assert!(health_from_stamina(-10.0) > 0.0);
    }
}
