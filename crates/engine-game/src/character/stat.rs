//! Every stat a character has, as one table rather than as fields.
//!
//! ```text
//! Stat          the names (eleven on a sheet, plus the item-only MainStat), by group
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
//! Seven of the eleven are *ratings*: whole points that convert to a fraction. The rating is
//! what an item grants and what is summed; the fraction is what gameplay multiplies by and
//! what the sheet prints beside it. The conversion lives here, once, so a tooltip and a
//! damage calculation cannot disagree about what a rating of 350 means.
//!
//! Two rules, one per kind of rating. The four *secondary* stats (Critical Strike, Haste,
//! Versatility, Spirit) use the diminishing curve [`secondary_fraction`]: `x / (x + 100)`.
//! The three *avoidance* stats (Block, Parry, Dodge) keep the linear
//! [`RATING_PER_PERCENT`] rule, because the Warrior is born with some of each and its
//! resource is tuned against exactly that - see `warrior.md`.
//!
//! # The main-stat placeholder
//!
//! Items are hero-agnostic, so a piece of gear cannot say "Intellect" or "Strength". It says
//! [`Stat::MainStat`], a twelfth slot in the block that only items carry; the character sheet
//! folds it into the wearer's real main stat before anything reads the total, so no consumer
//! of a sheet's stats ever sees a non-zero `MainStat`.

use crate::hero::Hero;

/// Which group a stat is shown and reasoned about in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatGroup {
    /// The stats a character is built out of: the main stat and Stamina.
    Primary,
    /// The ratings gear grants: Critical Strike, Haste, Versatility, Spirit.
    Secondary,
    /// What decides whether an incoming attack lands, and for how much: Armour, Block,
    /// Parry and Dodge.
    ///
    /// A group of its own rather than four more Secondary rows, because the sheet reads
    /// them together and because Armour is the one stat in the game that is neither a
    /// primary nor a rating - it is a flat number that converts to a percentage through a
    /// curve. See [`armor_reduction`].
    Defensive,
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
    /// Flat armour. Reduces physical damage taken through [`armor_reduction`].
    Armor,
    /// The chance to block an incoming physical attack, as a rating.
    Block,
    /// The chance to parry an incoming physical attack, as a rating.
    Parry,
    /// The chance to dodge an incoming physical attack, as a rating.
    Dodge,
    /// **Items only.** "Whatever the wearer's main stat is": Intellect on a mage or a druid,
    /// Strength on a warrior.
    ///
    /// Last in the enum so it is not in [`Stat::ALL`] and therefore never a row of the
    /// character sheet; it is in [`Stat::STORED`], so a block can hold it and an item tooltip
    /// can list it. The sheet folds it away - see [`super::CharacterSheet::gear_stats`].
    MainStat,
}

impl Stat {
    /// Every stat the character sheet lists, in sheet order.
    ///
    /// Deliberately without [`Stat::MainStat`]: that is an item-side placeholder, not a stat a
    /// character has. Iterate this to print a sheet; iterate [`Stat::STORED`] to visit every
    /// value a block can hold.
    pub const ALL: [Self; 11] = [
        Self::Strength,
        Self::Intellect,
        Self::Stamina,
        Self::CriticalStrike,
        Self::Haste,
        Self::Versatility,
        Self::Spirit,
        Self::Armor,
        Self::Block,
        Self::Parry,
        Self::Dodge,
    ];

    /// Every slot of a [`StatBlock`]: [`Stat::ALL`] plus the item-only [`Stat::MainStat`].
    pub const STORED: [Self; 12] = [
        Self::Strength,
        Self::Intellect,
        Self::Stamina,
        Self::CriticalStrike,
        Self::Haste,
        Self::Versatility,
        Self::Spirit,
        Self::Armor,
        Self::Block,
        Self::Parry,
        Self::Dodge,
        Self::MainStat,
    ];

    /// How many values a [`StatBlock`] holds.
    pub const COUNT: usize = Self::STORED.len();

    /// Dense index into a [`StatBlock`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The stat at `index`, if there is one.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::STORED.get(index).copied()
    }

    /// The name to show for this stat on `hero`'s gear: the placeholder resolves to the
    /// hero's own main stat ("INTELLECT", "STRENGTH"), everything else to [`Stat::name`].
    #[must_use]
    pub fn display_for(self, hero: Hero) -> &'static str {
        match self {
            Self::MainStat => hero.main_stat().name(),
            other => other.name(),
        }
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
            Self::Armor => "ARMOUR",
            Self::Block => "BLOCK",
            Self::Parry => "PARRY",
            Self::Dodge => "DODGE",
            Self::MainStat => "MAIN STAT",
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
            Self::Armor => "armor",
            Self::Block => "block",
            Self::Parry => "parry",
            Self::Dodge => "dodge",
            Self::MainStat => "main_stat",
        }
    }

    /// The stat saved or authored under `key` (see [`Stat::key`]), if this build has one.
    ///
    /// How a data file names a stat: by key, never by index.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::STORED.into_iter().find(|stat| stat.key() == key)
    }

    /// Which group the sheet lists this stat under.
    #[must_use]
    pub fn group(self) -> StatGroup {
        match self {
            Self::Strength | Self::Intellect | Self::Stamina | Self::MainStat => StatGroup::Primary,
            Self::CriticalStrike | Self::Haste | Self::Versatility | Self::Spirit => {
                StatGroup::Secondary
            }
            Self::Armor | Self::Block | Self::Parry | Self::Dodge => StatGroup::Defensive,
        }
    }

    /// Whether this stat is a rating that also reads as a percentage.
    ///
    /// Every secondary stat is, and three of the four defensive ones are. Armour is the
    /// exception: it is a flat number that becomes a percentage through a curve rather than
    /// through [`RATING_PER_PERCENT`], so printing `armour / 10000` beside it would be a
    /// percentage nothing in the damage pipeline uses.
    #[must_use]
    pub fn is_rating(self) -> bool {
        match self.group() {
            StatGroup::Primary => false,
            StatGroup::Secondary => true,
            StatGroup::Defensive => !matches!(self, Self::Armor),
        }
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
            Self::Armor => "REDUCES PHYSICAL DAMAGE TAKEN.",
            Self::Block => "CHANCE TO BLOCK A PHYSICAL ATTACK FOR REDUCED DAMAGE.",
            Self::Parry => "CHANCE TO PARRY A PHYSICAL ATTACK AND TAKE NONE OF IT.",
            Self::Dodge => "CHANCE TO DODGE A PHYSICAL ATTACK AND TAKE NONE OF IT.",
            Self::MainStat => "COUNTS AS YOUR MAIN STAT: INTELLECT OR STRENGTH.",
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

/// Rating that buys one per cent of an *avoidance* stat (Block, Parry, Dodge).
///
/// A flat linear rate: the Warrior is born with fixed amounts of each and its resource is
/// tuned against them. The four secondary stats do not use it; they use [`secondary_fraction`].
pub const RATING_PER_PERCENT: f32 = 100.0;

/// The rating at which a secondary stat is worth half: `secondary_fraction(100) == 0.5`.
pub const SECONDARY_HALF_POINT: f32 = 100.0;

/// What a secondary rating is worth as a fraction: `x / (x + 100)`.
///
/// **The one definition** for Critical Strike, Haste, Versatility and Spirit - the stat block,
/// the Spirit proc chance and the combat pipeline all read it. Asymptotic, like armour, so
/// stacking rating never reaches (let alone exceeds) a hundred per cent. A rating of 2 is
/// about 1.96 %. Zero, negative and non-finite ratings are worth nothing.
#[must_use]
pub fn secondary_fraction(rating: f32) -> f32 {
    if !rating.is_finite() || rating <= 0.0 {
        return 0.0;
    }
    rating / (rating + SECONDARY_HALF_POINT)
}

/// The rating that is worth `fraction` through [`secondary_fraction`]. The inverse, for
/// content that is specified in per cent (a "30 % haste" buff) rather than in rating.
///
/// `const` so a status table can use it. A fraction outside `(0, 1)` has no finite positive
/// rating and answers zero rather than infinity.
#[must_use]
pub const fn rating_for_fraction(fraction: f32) -> f32 {
    if fraction <= 0.0 || fraction >= 1.0 {
        return 0.0;
    }
    SECONDARY_HALF_POINT * fraction / (1.0 - fraction)
}

/// Armour that halves the physical damage taken.
///
/// The single constant of the mitigation curve `armour / (armour + K)`, which is asymptotic:
/// it never reaches a hundred per cent however much armour is stacked, so no amount of gear
/// makes a character immune.
pub const ARMOR_HALVING_POINT: f32 = 1000.0;

/// The hardest any armour value may hit, as a fraction.
///
/// The curve is already asymptotic; this is the belt-and-braces clamp that keeps a
/// hand-edited item file from producing a character nothing can hurt.
pub const ARMOR_MAX_REDUCTION: f32 = 0.75;

/// What `armor` takes off a physical hit, as a fraction in `0.0..=ARMOR_MAX_REDUCTION`.
///
/// **The one definition.** The damage pipeline, the character sheet and the Warrior's
/// Deflection bonus all read it, so there is no second answer to "what is my armour worth".
#[must_use]
pub fn armor_reduction(armor: f32) -> f32 {
    if !armor.is_finite() || armor <= 0.0 {
        return 0.0;
    }
    (armor / (armor + ARMOR_HALVING_POINT)).clamp(0.0, ARMOR_MAX_REDUCTION)
}

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
        for stat in Stat::STORED {
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

    /// Every stat with a non-zero value, in [`Stat::STORED`] order (so the item-only
    /// [`Stat::MainStat`] comes last).
    pub fn entries(&self) -> impl Iterator<Item = (Stat, f32)> + '_ {
        Stat::STORED
            .into_iter()
            .map(|stat| (stat, self.get(stat)))
            .filter(|(_, value)| *value != 0.0)
    }

    /// What `stat` is worth as a fraction, for a rating.
    ///
    /// `0.15` means fifteen per cent. The four secondary stats go through
    /// [`secondary_fraction`]; Block, Parry and Dodge are linear at [`RATING_PER_PERCENT`].
    /// A primary stat has no percentage and answers zero, which saves every caller from
    /// asking which kind of stat it is holding.
    #[must_use]
    pub fn percent(&self, stat: Stat) -> f32 {
        if !stat.is_rating() {
            return 0.0;
        }
        match stat.group() {
            StatGroup::Secondary => secondary_fraction(self.get(stat)),
            _ => self.get(stat) / (RATING_PER_PERCENT * 100.0),
        }
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
    fn the_groups_split_the_sheets_eleven_into_three_four_and_four() {
        let count = |group| {
            Stat::ALL
                .into_iter()
                .filter(|stat| stat.group() == group)
                .count()
        };
        assert_eq!(
            (
                count(StatGroup::Primary),
                count(StatGroup::Secondary),
                count(StatGroup::Defensive),
            ),
            (3, 4, 4)
        );
    }

    #[test]
    fn armour_is_the_one_defensive_stat_that_is_not_a_rating() {
        assert!(!Stat::Armor.is_rating());
        for stat in [Stat::Block, Stat::Parry, Stat::Dodge] {
            assert!(stat.is_rating(), "{}", stat.name());
        }
    }

    #[test]
    fn armour_mitigation_climbs_towards_a_ceiling_it_never_reaches() {
        assert_eq!(armor_reduction(0.0), 0.0);
        assert_eq!(armor_reduction(-100.0), 0.0);
        assert!((armor_reduction(ARMOR_HALVING_POINT) - 0.5).abs() < 1e-6);
        assert!(armor_reduction(f32::MAX) <= ARMOR_MAX_REDUCTION);
        let mut previous = 0.0;
        for armor in [100.0, 500.0, 1_000.0, 5_000.0, 50_000.0] {
            let reduction = armor_reduction(armor);
            assert!(reduction >= previous, "{armor}");
            previous = reduction;
        }
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
    fn a_secondary_rating_follows_x_over_x_plus_100() {
        assert_eq!(secondary_fraction(0.0), 0.0);
        assert_eq!(secondary_fraction(-5.0), 0.0);
        assert_eq!(secondary_fraction(f32::NAN), 0.0);
        assert!((secondary_fraction(2.0) - 2.0 / 102.0).abs() < 1e-6);
        assert!((secondary_fraction(100.0) - 0.5).abs() < 1e-6);
        assert!(secondary_fraction(1.0e9) <= 1.0);

        let mut block = StatBlock::zero();
        for stat in [
            Stat::Haste,
            Stat::CriticalStrike,
            Stat::Versatility,
            Stat::Spirit,
        ] {
            block.set(stat, 2.0);
            assert!(
                (block.percent(stat) - 0.0196).abs() < 1e-4,
                "{}",
                stat.name()
            );
        }
    }

    #[test]
    fn the_inverse_rating_round_trips() {
        for fraction in [0.05, 0.3, 0.75] {
            let back = secondary_fraction(rating_for_fraction(fraction));
            assert!((back - fraction).abs() < 1e-5, "{fraction}");
        }
        assert_eq!(rating_for_fraction(0.0), 0.0);
        assert_eq!(rating_for_fraction(1.0), 0.0);
    }

    #[test]
    fn avoidance_stays_linear_and_a_primary_stat_has_no_percentage() {
        let mut block = StatBlock::zero();
        block.set(Stat::Dodge, RATING_PER_PERCENT * 15.0);
        assert!((block.percent(Stat::Dodge) - 0.15).abs() < 1e-6);

        block.set(Stat::Intellect, 500.0);
        assert_eq!(block.percent(Stat::Intellect), 0.0);
        block.set(Stat::MainStat, 500.0);
        assert_eq!(block.percent(Stat::MainStat), 0.0);
    }

    #[test]
    fn main_stat_is_stored_but_never_a_sheet_row() {
        assert!(!Stat::ALL.contains(&Stat::MainStat));
        assert_eq!(Stat::STORED.len(), Stat::COUNT);
        for (index, stat) in Stat::STORED.into_iter().enumerate() {
            assert_eq!(stat.index(), index);
            assert_eq!(Stat::from_index(index), Some(stat));
        }
        assert_eq!(Stat::MainStat.group(), StatGroup::Primary);
        assert_eq!(Stat::MainStat.display_for(Hero::Warrior), "STRENGTH");
        assert_eq!(Stat::MainStat.display_for(Hero::FireMage), "INTELLECT");
        assert_eq!(Stat::Haste.display_for(Hero::Warrior), "HASTE");

        let mut block = StatBlock::of(Stat::MainStat, 2.0);
        block.add_block(&StatBlock::of(Stat::MainStat, 2.0));
        assert_eq!(block.get(Stat::MainStat), 4.0);
        assert_eq!(block.entries().count(), 1);
    }

    #[test]
    fn health_comes_from_stamina_and_never_reaches_zero() {
        assert_eq!(health_from_stamina(50.0), 500.0);
        assert!(health_from_stamina(0.0) > 0.0);
        assert!(health_from_stamina(-10.0) > 0.0);
    }
}
