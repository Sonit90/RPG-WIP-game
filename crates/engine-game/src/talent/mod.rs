//! Talents for every hero: one framework, one set of rules.
//!
//! ```text
//! talents.json -> TalentCatalog (table) -> TalentGrid (what is bought) -> <Hero>TalentEffects
//! ```
//!
//! Three layers, the same for every hero:
//!
//! 1. **The table** - [`HeroTalents`]: a hero's talents as plain data (key, name, tier, cost,
//!    description and a bag of named numbers, [`TalentValues`]). This crate is serde-free; the
//!    shipped numbers live in `assets/catalog/talents.json`, which `engine-realm` parses into
//!    a [`TalentCatalog`] and [`install`]s. Until one is installed (every unit test in this
//!    crate) the built-in defaults - the same numbers the JSON ships - are used.
//! 2. **The grid** - [`TalentGrid`]: a bitset of up to [`MAX_TALENTS`] purchases over a hero's
//!    table, with the points spent and left. It knows nothing about what a talent *does*.
//! 3. **The effects** - [`crate::firemage::TalentEffects`], [`druid::DruidTalentEffects`],
//!    [`warrior::WarriorTalentEffects`]: what the bought talents mean, resolved once into the
//!    numbers gameplay reads, never scattered as `if has_talent(..)`.
//!
//! A hero's talents are an enum implementing [`TalentKey`]: its variant order is the table's
//! row order. Adding a talent is a row in the JSON, a variant, and an effect field.

pub mod druid;
pub mod warrior;

use std::sync::OnceLock;

use crate::hero::Hero;

/// The most talents one hero may have: the grid is a `u32` bitset.
pub const MAX_TALENTS: usize = 32;

/// How many tiers the menu groups talents into.
pub const TALENT_TIERS: u8 = 4;

/// One talent, as the built-in default table writes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentDef {
    /// Stable identity, what a saved character stores.
    pub key: &'static str,
    /// Uppercase name; the UI font has no lowercase glyphs.
    pub name: &'static str,
    /// Tier, 1 to [`TALENT_TIERS`]. Grouping for the menu, not a gate.
    pub tier: u8,
    /// Talent points it costs.
    pub cost: u32,
    /// The tooltip, with `{value}` / `{value%}` placeholders filled from `values`.
    pub description: &'static str,
    /// Every number the talent's effect reads, by name.
    pub values: &'static [(&'static str, f32)],
}

/// A talent's named numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TalentValues(Vec<(String, f32)>);

impl TalentValues {
    /// Values from `(name, number)` pairs.
    #[must_use]
    pub fn new(pairs: impl IntoIterator<Item = (String, f32)>) -> Self {
        let mut pairs: Vec<(String, f32)> = pairs.into_iter().collect();
        // Sorted, so two tables with the same numbers are equal however they were written.
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        Self(pairs)
    }

    /// The default values of a built-in definition.
    #[must_use]
    pub fn of_def(def: &TalentDef) -> Self {
        Self::new(
            def.values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), *value)),
        )
    }

    /// The number called `name`, if there is one.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<f32> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| *value)
    }

    /// The number called `name`. Zero for a name the table lacks, which [`install`] rules out
    /// for every name an effect reads.
    #[must_use]
    pub fn value(&self, name: &str) -> f32 {
        self.get(name).unwrap_or(0.0)
    }

    /// Every `(name, number)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, f32)> {
        self.0.iter().map(|(name, value)| (name.as_str(), *value))
    }
}

/// One talent as the game runs it.
#[derive(Debug, Clone, PartialEq)]
pub struct TalentSpec {
    /// Stable identity.
    pub key: String,
    /// Uppercase display name.
    pub name: String,
    /// Tier, 1 to [`TALENT_TIERS`].
    pub tier: u8,
    /// Talent points it costs.
    pub cost: u32,
    /// The tooltip, placeholders already filled.
    pub description: String,
    /// The numbers its effect reads.
    pub values: TalentValues,
}

impl TalentSpec {
    /// A spec from a template description: placeholders are filled from `values`.
    #[must_use]
    pub fn new(
        key: &str,
        name: &str,
        tier: u8,
        cost: u32,
        template: &str,
        values: TalentValues,
    ) -> Self {
        Self {
            key: key.to_owned(),
            name: name.to_owned(),
            tier,
            cost,
            description: fill_description(template, &values),
            values,
        }
    }

    fn of_def(def: &TalentDef) -> Self {
        Self::new(
            def.key,
            def.name,
            def.tier,
            def.cost,
            def.description,
            TalentValues::of_def(def),
        )
    }
}

/// Fills `{name}` with the number and `{name%}` with the number as a whole percent.
///
/// An unknown name is left as written, which a test of the shipped table rules out.
#[must_use]
pub fn fill_description(template: &str, values: &TalentValues) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        let (name, percent) = token
            .strip_suffix('%')
            .map_or((token, false), |name| (name, true));
        match values.get(name) {
            Some(value) if percent => out.push_str(&format!("{}", (value * 100.0).round())),
            Some(value) => {
                if (value - value.round()).abs() < 1e-4 {
                    out.push_str(&format!("{}", value.round()));
                } else {
                    out.push_str(&format!("{value}"));
                }
            }
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// One hero's talents, in row order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeroTalents {
    /// The talents; row `i` is the hero's talent enum variant `i`.
    pub talents: Vec<TalentSpec>,
}

impl HeroTalents {
    /// The built-in table of `defs`.
    #[must_use]
    pub fn from_defs(defs: &[TalentDef]) -> Self {
        Self {
            talents: defs.iter().map(TalentSpec::of_def).collect(),
        }
    }

    /// The row with this key.
    #[must_use]
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.talents.iter().position(|talent| talent.key == key)
    }

    /// Points needed to own every talent.
    #[must_use]
    pub fn total_cost(&self) -> u32 {
        self.talents.iter().map(|talent| talent.cost).sum()
    }
}

/// How talent points are earned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentProgression {
    /// Points every character starts with.
    pub starting_points: u32,
    /// Points a successful run grants.
    pub points_per_completion: u32,
    /// The lowest key level that also grants the bonus.
    pub bonus_points_min_key_level: u32,
    /// Extra points at or above that key level.
    pub bonus_points: u32,
}

impl Default for TalentProgression {
    fn default() -> Self {
        Self {
            starting_points: 5,
            points_per_completion: 1,
            bonus_points_min_key_level: 3,
            bonus_points: 1,
        }
    }
}

impl TalentProgression {
    /// What one run pays: nothing on a failure.
    #[must_use]
    pub fn points_for_run(&self, success: bool, key_level: u32) -> u32 {
        if !success {
            return 0;
        }
        let bonus = if key_level >= self.bonus_points_min_key_level {
            self.bonus_points
        } else {
            0
        };
        self.points_per_completion.saturating_add(bonus)
    }

    /// Points a character that has earned `earned` has in total.
    #[must_use]
    pub fn total_points(&self, earned: u32) -> u32 {
        self.starting_points.saturating_add(earned)
    }
}

/// Every hero's talents and the progression.
#[derive(Debug, Clone, PartialEq)]
pub struct TalentCatalog {
    /// How points are earned.
    pub progression: TalentProgression,
    /// Per hero, indexed by [`Hero::index`].
    pub heroes: [HeroTalents; 3],
}

impl Default for TalentCatalog {
    fn default() -> Self {
        Self {
            progression: TalentProgression::default(),
            heroes: [
                HeroTalents::from_defs(&crate::firemage::talent::DEFS),
                HeroTalents::from_defs(&warrior::DEFS),
                HeroTalents::from_defs(&druid::DEFS),
            ],
        }
    }
}

impl TalentCatalog {
    /// The built-in rows a hero's table must agree with, as `(key, value names)`.
    fn default_rows(hero: Hero) -> Vec<(&'static str, Vec<&'static str>)> {
        let defs: &[TalentDef] = match hero {
            Hero::FireMage => &crate::firemage::talent::DEFS,
            Hero::Warrior => &warrior::DEFS,
            Hero::Druid => &druid::DEFS,
        };
        defs.iter()
            .map(|def| (def.key, def.values.iter().map(|(name, _)| *name).collect()))
            .collect()
    }

    /// Whether the table is one the game can run: the same talents in the same order as the
    /// code's, every number an effect reads present, sane tiers and costs.
    ///
    /// # Errors
    ///
    /// A sentence naming the first thing wrong.
    pub fn validate(&self) -> Result<(), String> {
        for hero in Hero::ALL {
            let table = &self.heroes[hero.index()];
            let rows = Self::default_rows(hero);
            if table.talents.len() != rows.len() || rows.len() > MAX_TALENTS {
                return Err(format!(
                    "{} has {} talents, the game implements {}",
                    hero.name(),
                    table.talents.len(),
                    rows.len()
                ));
            }
            for (spec, (key, names)) in table.talents.iter().zip(&rows) {
                if spec.key != *key {
                    return Err(format!(
                        "{}: talent '{}' is where the game expects '{key}'",
                        hero.name(),
                        spec.key
                    ));
                }
                if !(1..=TALENT_TIERS).contains(&spec.tier) || spec.cost == 0 {
                    return Err(format!("{}: '{key}' has a bad tier or cost", hero.name()));
                }
                for name in names {
                    if spec.values.get(name).is_none() {
                        return Err(format!("{}: '{key}' lacks the value '{name}'", hero.name()));
                    }
                }
            }
        }
        Ok(())
    }
}

static CATALOG: OnceLock<TalentCatalog> = OnceLock::new();

/// Installs the shipped table. The first install wins; later ones are ignored.
///
/// # Errors
///
/// The table is not one the game can run (see [`TalentCatalog::validate`]).
pub fn install(catalog: TalentCatalog) -> Result<(), String> {
    catalog.validate()?;
    let _ = CATALOG.set(catalog);
    Ok(())
}

/// The talent catalog in force: the installed one, else the built-in defaults.
#[must_use]
pub fn catalog() -> &'static TalentCatalog {
    CATALOG.get_or_init(TalentCatalog::default)
}

/// `hero`'s talents in force.
#[must_use]
pub fn hero_talents(hero: Hero) -> &'static HeroTalents {
    &catalog().heroes[hero.index()]
}

/// How points are earned, in force.
#[must_use]
pub fn progression() -> TalentProgression {
    catalog().progression
}

/// A hero's talent enum: row `index()` of that hero's table.
pub trait TalentKey: Copy + PartialEq + 'static {
    /// Whose talent this is.
    const HERO: Hero;
    /// Every talent, in table order.
    const ALL: &'static [Self];
    /// The table row.
    fn index(self) -> usize;
    /// This talent's table row.
    #[must_use]
    fn spec(self) -> &'static TalentSpec {
        &hero_talents(Self::HERO).talents[self.index()]
    }
}

/// Why a talent could not be bought, or a saved set not be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalentError {
    /// It is already owned.
    AlreadyOwned,
    /// There are not enough points.
    NotEnoughPoints,
    /// The key names no talent of this hero.
    UnknownTalent,
}

impl TalentError {
    /// Short uppercase description, for the UI font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyOwned => "ALREADY LEARNED",
            Self::NotEnoughPoints => "NOT ENOUGH POINTS",
            Self::UnknownTalent => "UNKNOWN TALENT",
        }
    }
}

/// What the player has bought of one hero's table, and what they have left to spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TalentGrid {
    hero: Hero,
    /// One bit per table row.
    purchased: u32,
    available: u32,
    spent: u32,
}

impl Default for TalentGrid {
    fn default() -> Self {
        Self::new(Hero::FireMage)
    }
}

impl TalentGrid {
    /// A grid with the starting points and nothing bought.
    #[must_use]
    pub fn new(hero: Hero) -> Self {
        Self::with_points(hero, progression().starting_points)
    }

    /// A grid with an explicit number of points, for tests, builds and the simulator.
    #[must_use]
    pub fn with_points(hero: Hero, points: u32) -> Self {
        Self {
            hero,
            purchased: 0,
            available: points,
            spent: 0,
        }
    }

    /// A grid holding exactly `talents`, ignoring the budget.
    #[must_use]
    pub fn from_talents<T: TalentKey>(talents: &[T]) -> Self {
        let mut grid = Self::with_points(T::HERO, hero_talents(T::HERO).total_cost());
        for talent in talents {
            let _ = grid.purchase(*talent);
        }
        grid
    }

    /// A grid holding the talents `keys` name, with `total_points` to spend in all.
    ///
    /// This is how a saved character is checked: the same function on the API and in the
    /// realm, so they cannot disagree.
    ///
    /// # Errors
    ///
    /// [`TalentError::UnknownTalent`] for a key the hero lacks, [`TalentError::NotEnoughPoints`]
    /// when the set costs more than `total_points`. Duplicate keys count once.
    pub fn from_keys<S: AsRef<str>>(
        hero: Hero,
        keys: &[S],
        total_points: u32,
    ) -> Result<Self, TalentError> {
        let table = hero_talents(hero);
        let mut grid = Self::with_points(hero, total_points);
        for key in keys {
            let index = table
                .index_of(key.as_ref())
                .ok_or(TalentError::UnknownTalent)?;
            match grid.purchase_index(index) {
                Ok(()) | Err(TalentError::AlreadyOwned) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(grid)
    }

    /// Whose table this is a grid over.
    #[must_use]
    pub fn hero(&self) -> Hero {
        self.hero
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

    /// Spent plus left.
    #[must_use]
    pub fn total_points(&self) -> u32 {
        self.available + self.spent
    }

    /// Grants points, as progression would.
    pub fn grant_points(&mut self, points: u32) {
        self.available = self.available.saturating_add(points);
    }

    fn cost_of(&self, index: usize) -> u32 {
        hero_talents(self.hero)
            .talents
            .get(index)
            .map_or(u32::MAX, |talent| talent.cost)
    }

    /// Whether row `index` is owned.
    #[must_use]
    pub fn has_index(&self, index: usize) -> bool {
        index < MAX_TALENTS && self.purchased & (1 << index) != 0
    }

    /// Whether `talent` is owned.
    #[must_use]
    pub fn has<T: TalentKey>(&self, talent: T) -> bool {
        T::HERO == self.hero && self.has_index(talent.index())
    }

    /// Whether `talent` could be bought right now.
    #[must_use]
    pub fn can_purchase<T: TalentKey>(&self, talent: T) -> bool {
        self.purchase_error_index(talent.index()).is_none()
    }

    /// Why row `index` cannot be bought, or `None` if it can.
    #[must_use]
    pub fn purchase_error_index(&self, index: usize) -> Option<TalentError> {
        if index >= hero_talents(self.hero).talents.len() {
            return Some(TalentError::UnknownTalent);
        }
        if self.has_index(index) {
            return Some(TalentError::AlreadyOwned);
        }
        if self.available < self.cost_of(index) {
            return Some(TalentError::NotEnoughPoints);
        }
        None
    }

    /// Why `talent` cannot be bought, or `None` if it can.
    #[must_use]
    pub fn purchase_error<T: TalentKey>(&self, talent: T) -> Option<TalentError> {
        self.purchase_error_index(talent.index())
    }

    /// Buys row `index`, spending its cost.
    ///
    /// # Errors
    ///
    /// Returns [`TalentError`] if it is owned already, unknown or cannot be afforded.
    pub fn purchase_index(&mut self, index: usize) -> Result<(), TalentError> {
        if let Some(error) = self.purchase_error_index(index) {
            return Err(error);
        }
        let cost = self.cost_of(index);
        self.available -= cost;
        self.spent += cost;
        self.purchased |= 1 << index;
        Ok(())
    }

    /// Buys `talent`, spending its cost.
    ///
    /// # Errors
    ///
    /// Returns [`TalentError`] if it is owned already or cannot be afforded.
    pub fn purchase<T: TalentKey>(&mut self, talent: T) -> Result<(), TalentError> {
        if T::HERO != self.hero {
            return Err(TalentError::UnknownTalent);
        }
        self.purchase_index(talent.index())
    }

    /// How many talents are owned.
    #[must_use]
    pub fn purchased_count(&self) -> u32 {
        self.purchased.count_ones()
    }

    /// Every owned talent.
    pub fn purchased<T: TalentKey>(&self) -> impl Iterator<Item = T> + '_ {
        T::ALL.iter().copied().filter(|id| self.has(*id))
    }

    /// The keys of every owned talent, in table order.
    #[must_use]
    pub fn purchased_keys(&self) -> Vec<String> {
        hero_talents(self.hero)
            .talents
            .iter()
            .enumerate()
            .filter(|(index, _)| self.has_index(*index))
            .map(|(_, talent)| talent.key.clone())
            .collect()
    }

    /// Refunds everything, returning the grid to a fresh build.
    pub fn reset(&mut self) {
        self.available += self.spent;
        self.spent = 0;
        self.purchased = 0;
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
    fn the_default_table_is_valid_and_complete() {
        TalentCatalog::default().validate().expect("valid");
        assert_eq!(hero_talents(Hero::FireMage).talents.len(), 12);
        assert_eq!(hero_talents(Hero::Druid).talents.len(), 8);
        assert_eq!(hero_talents(Hero::Warrior).talents.len(), 8);
    }

    #[test]
    fn placeholders_fill_from_values() {
        let values = TalentValues::new([("a".to_owned(), 0.25), ("b".to_owned(), 4.0)]);
        assert_eq!(
            fill_description("{a%} PERCENT, {b} STACKS", &values),
            "25 PERCENT, 4 STACKS"
        );
        assert_eq!(fill_description("{zzz}", &values), "{zzz}");
    }

    #[test]
    fn keys_resolve_and_are_checked_against_points() {
        let keys = ["swift_wrath", "deep_exposure"];
        let grid = TalentGrid::from_keys(Hero::Druid, &keys, 7).expect("fits");
        assert_eq!(grid.purchased_count(), 2);
        assert_eq!(grid.available_points(), 0);
        assert_eq!(
            TalentGrid::from_keys(Hero::Druid, &keys, 6),
            Err(TalentError::NotEnoughPoints)
        );
        assert_eq!(
            TalentGrid::from_keys(Hero::Druid, &["nope"], 99),
            Err(TalentError::UnknownTalent)
        );
        assert_eq!(
            TalentGrid::from_keys(Hero::Warrior, &keys, 99),
            Err(TalentError::UnknownTalent)
        );
    }

    #[test]
    fn progression_pays_successful_runs_only() {
        let p = TalentProgression::default();
        assert_eq!(p.points_for_run(false, 10), 0);
        assert_eq!(p.points_for_run(true, 1), 1);
        assert_eq!(p.points_for_run(true, 3), 2);
    }

    #[test]
    fn reset_refunds_every_point() {
        let mut grid = TalentGrid::with_points(Hero::Druid, 20);
        grid.purchase(druid::DruidTalent::SwiftWrath).expect("buy");
        assert_eq!(grid.available_points(), 17);
        grid.reset();
        assert_eq!(grid.available_points(), 20);
        assert_eq!(grid.purchased_count(), 0);
    }
}
