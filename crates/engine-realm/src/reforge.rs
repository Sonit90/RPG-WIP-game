//! Reforging: pay gold, re-roll an item's base quality from a published table.
//!
//! ```text
//! reforge(loadout, slot, draw, economy)
//!    refuse: empty / not equipment / broken / cannot afford
//!    pay the whole price                      (always, whatever follows)
//!    new quality = roll(table)                REPLACES the old one - no keep-best
//!    sharpening and the broken flag are never touched
//! ```
//!
//! The table is `economy.json`'s `reforge` block, shown to the player before they confirm
//! ([`ReforgeTable::bands`], [`ReforgeTable::expected_value`]). Its mean sits a little below
//! what a drop rolls, so the gamble has a real downside.
//!
//! Pure rules: the persistence API runs [`reforge`] inside one locked read-modify-write of the
//! character row and supplies `draw` from the server's own operating-system random source, so
//! a client can neither choose its roll nor skip the price. Nothing here reads a clock or a
//! random source. Effective quality stays the one formula, `base + sharpening`
//! (`ItemProps::total_pct`).

use std::fmt;

use engine_game::character::{ItemProps, item_by_key};

use crate::catalog::{Economy, ReforgeBand};
use crate::loadout::{Loadout, SlotRef};
use crate::loot::ensure_items_registered;
use crate::sharpen::{Place, SharpenError, locate};

/// The resource a reforge is paid in.
pub const GOLD: &str = "gold";

/// The published outcome table: where a reforged item's new base quality lands.
#[derive(Debug, Clone, PartialEq)]
pub struct ReforgeTable {
    bands: Vec<ReforgeBand>,
}

/// The outcome table of `economy`; empty (reforging off) when the catalog has none.
#[must_use]
pub fn reforge_odds(economy: &Economy) -> ReforgeTable {
    ReforgeTable {
        bands: economy.reforge.bands.clone(),
    }
}

impl ReforgeTable {
    /// The bands, lowest first. Chances sum to one.
    #[must_use]
    pub fn bands(&self) -> &[ReforgeBand] {
        &self.bands
    }

    /// Whether the table has no band (reforging is off).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// The mean new quality: each band is uniform, so its midpoint weighted by its chance.
    #[must_use]
    pub fn expected_value(&self) -> f64 {
        self.bands
            .iter()
            .map(|band| band.chance * (f64::from(band.min) + f64::from(band.max)) / 2.0)
            .sum()
    }

    /// The chance of landing at `quality` or better.
    #[must_use]
    pub fn chance_at_least(&self, quality: u8) -> f64 {
        self.bands
            .iter()
            .map(|band| {
                if band.min >= quality {
                    band.chance
                } else if band.max >= quality {
                    let count = f64::from(band.max - band.min) + 1.0;
                    band.chance * (f64::from(band.max - quality) + 1.0) / count
                } else {
                    0.0
                }
            })
            .sum()
    }

    /// The chance of landing strictly below `quality`.
    #[must_use]
    pub fn chance_below(&self, quality: u8) -> f64 {
        (1.0 - self.chance_at_least(quality)).max(0.0)
    }
}

/// A number in `[0, 1)` from 64 random bits (the top 53, exactly representable).
fn unit(bits: u64) -> f64 {
    (bits >> 11) as f64 / (1_u64 << 53) as f64
}

/// **One roll** of the table: a band by its chance, then a whole quality uniform inside it.
///
/// `draw` yields 64 random bits per call and is called twice. The server feeds it from the
/// operating system's random source; tests feed it a seeded generator. An empty table rolls
/// the lowest quality (it cannot be reached through [`reforge`], which refuses first).
pub fn roll(draw: &mut dyn FnMut() -> u64, table: &ReforgeTable) -> u8 {
    let Some(last) = table.bands.last() else {
        return engine_game::character::QUALITY_MIN;
    };
    let pick = unit(draw());
    let within = unit(draw());
    let mut acc = 0.0;
    let band = table
        .bands
        .iter()
        .find(|band| {
            acc += band.chance;
            pick < acc
        })
        // Rounding in the sum: the last band takes the sliver left over.
        .unwrap_or(last);
    let count = u32::from(band.max - band.min) + 1;
    let offset = ((within * f64::from(count)) as u32).min(count - 1);
    band.min + u8::try_from(offset).unwrap_or(0)
}

/// Why a reforge was refused. Nothing was paid or changed in any of these cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReforgeError {
    /// Reforging is switched off (the catalog has no outcome table).
    Disabled,
    /// The address names nothing: an unknown slot, or an empty one.
    EmptySlot,
    /// The item is not equipment: unknown, stackable, or granting no stats.
    NotEquipment,
    /// A broken item may not be reforged.
    Broken,
    /// The character cannot pay; carries the resource's display name.
    Insufficient(String),
}

impl fmt::Display for ReforgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("Reforging is not available."),
            Self::EmptySlot => f.write_str("There is no item there to reforge."),
            Self::NotEquipment => f.write_str("That item cannot be reforged."),
            Self::Broken => f.write_str("A broken item cannot be reforged."),
            Self::Insufficient(resource) => write!(f, "You do not have enough {resource}."),
        }
    }
}

impl std::error::Error for ReforgeError {}

impl From<SharpenError> for ReforgeError {
    fn from(error: SharpenError) -> Self {
        match error {
            SharpenError::EmptySlot => Self::EmptySlot,
            _ => Self::NotEquipment,
        }
    }
}

/// What reforging the item at a slot would cost, for a panel or a confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct ReforgeQuote {
    /// The item's stable key.
    pub item: String,
    /// Its props now.
    pub props: ItemProps,
    /// The gold it costs.
    pub gold: u64,
    /// The outcome table.
    pub table: ReforgeTable,
}

/// The gold price of reforging an item of `item_level` with `sharpening` levels.
#[must_use]
pub fn cost(economy: &Economy, item_level: u16, sharpening: u8) -> u64 {
    let rule = &economy.reforge.cost;
    rule.gold
        .saturating_add(rule.per_item_level.saturating_mul(u64::from(item_level)))
        .saturating_add(
            rule.per_sharpening_level
                .saturating_mul(u64::from(sharpening)),
        )
}

/// Checks the item at `slot` may be reforged and what it would cost; affordability is not
/// checked.
///
/// # Errors
///
/// [`ReforgeError::Disabled`], [`ReforgeError::EmptySlot`], [`ReforgeError::NotEquipment`],
/// [`ReforgeError::Broken`].
pub fn quote(
    loadout: &Loadout,
    slot: &SlotRef,
    economy: &Economy,
) -> Result<ReforgeQuote, ReforgeError> {
    ensure_items_registered();
    let table = reforge_odds(economy);
    if table.is_empty() {
        return Err(ReforgeError::Disabled);
    }
    let (props, _) = locate(loadout, slot)?;
    if props.broken && !economy.reforge.eligible.allow_broken {
        return Err(ReforgeError::Broken);
    }
    let item = match slot {
        SlotRef::Equipment { slot } => loadout
            .equipment
            .iter()
            .find(|entry| entry.slot == *slot)
            .map(|entry| entry.item.clone()),
        SlotRef::Inventory { index } => loadout
            .inventory
            .iter()
            .find(|entry| entry.index == *index)
            .map(|entry| entry.item.clone()),
    }
    .ok_or(ReforgeError::EmptySlot)?;
    let item_level = item_by_key(&item).map_or(0, |def| def.item_level);
    Ok(ReforgeQuote {
        gold: cost(economy, item_level, props.sharpening),
        item,
        props,
        table,
    })
}

/// What a reforge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReforgeOutcome {
    /// The item's stable key.
    pub item: String,
    /// The base quality before.
    pub old_quality: u8,
    /// The base quality now (not necessarily better).
    pub new_quality: u8,
    /// The gold paid.
    pub cost: u64,
    /// The gold left.
    pub gold_left: u64,
}

/// **One reforge** of the item at `slot` (worn or in the bag).
///
/// The whole price is paid on every reforge that is allowed, and the new base quality replaces
/// the old whatever it is. Sharpening and the broken flag are left alone. `draw` is the
/// caller's random source (see [`roll`]); a refused reforge draws nothing.
///
/// # Errors
///
/// Those of [`quote`], and [`ReforgeError::Insufficient`] when the purse is short. The loadout
/// is unchanged on every error.
pub fn reforge(
    loadout: &mut Loadout,
    slot: &SlotRef,
    draw: &mut dyn FnMut() -> u64,
    economy: &Economy,
) -> Result<ReforgeOutcome, ReforgeError> {
    let quote = quote(loadout, slot, economy)?;
    let held = loadout.resources.get(GOLD).copied().unwrap_or(0);
    let left = held.checked_sub(quote.gold).ok_or_else(|| {
        ReforgeError::Insufficient(
            economy
                .resource(GOLD)
                .map_or_else(|| GOLD.to_owned(), |resource| resource.name.clone()),
        )
    })?;
    let (_, place) = locate(loadout, slot)?;
    if left == 0 {
        loadout.resources.remove(GOLD);
    } else {
        loadout.resources.insert(GOLD.to_owned(), left);
    }

    let new_quality = roll(draw, &quote.table);
    match place {
        Place::Worn(index) => loadout.equipment[index].quality = new_quality,
        Place::Bag(index) => loadout.inventory[index].quality = new_quality,
    }
    Ok(ReforgeOutcome {
        item: quote.item,
        old_quality: quote.props.quality,
        new_quality,
        cost: quote.gold,
        gold_left: left,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::economy::add_resources;
    use crate::loadout::{BagItem, EquippedItem};
    use engine_game::firemage::Rng;

    fn economy() -> Economy {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .economy()
            .clone()
    }

    const KEY: &str = "ashgraves_seal";

    fn slot() -> SlotRef {
        SlotRef::Equipment {
            slot: "ring1".into(),
        }
    }

    fn loadout(quality: u8, sharpening: u8, gold: u64) -> Loadout {
        ensure_items_registered();
        let mut loadout = Loadout::default();
        let mut entry = EquippedItem::new("ring1", KEY);
        entry.quality = quality;
        entry.sharpening = sharpening;
        loadout.equipment.push(entry);
        if gold > 0 {
            add_resources(&mut loadout, &[(GOLD.into(), gold)]);
        }
        loadout
    }

    #[test]
    fn the_shipped_table_has_a_mean_below_a_drops_and_a_real_top() {
        let table = reforge_odds(&economy());
        let mean = table.expected_value();
        assert!((mean - 71.528).abs() < 1e-9, "mean {mean}");
        // A drop at key level 1 rolls 72.8 on average (loot.json): reforging is a slightly
        // losing bet.
        assert!(mean < 72.77 && mean > 65.0);
        assert!((table.chance_at_least(100) - 0.05).abs() < 1e-9);
        assert!((table.chance_at_least(110) - 0.001).abs() < 1e-9);
        assert!((table.chance_at_least(50) - 1.0).abs() < 1e-9);
        assert!((table.chance_below(70) - 0.5).abs() < 1e-9);
        assert!((table.chance_at_least(105) - 0.01).abs() < 1e-9);
    }

    #[test]
    fn a_roll_is_deterministic_for_a_seed_and_always_inside_the_bands() {
        let table = reforge_odds(&economy());
        let run = |seed| {
            let mut rng = Rng::new(seed);
            (0..64)
                .map(|_| roll(&mut || rng.next_u64(), &table))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        for quality in run(99) {
            assert!(
                table
                    .bands()
                    .iter()
                    .any(|band| (band.min..=band.max).contains(&quality))
            );
        }
        // The extremes of the random bits are the extremes of the table.
        assert_eq!(roll(&mut || 0, &table), 50);
        assert_eq!(roll(&mut || u64::MAX, &table), 110);
    }

    #[test]
    fn many_rolls_match_the_published_chances() {
        let table = reforge_odds(&economy());
        let mut rng = Rng::new(0x5EED);
        let n = 400_000_u32;
        let mut counts = vec![0_u32; table.bands().len()];
        let mut total = 0_u64;
        let mut hundreds = [0_u32; 5];
        for _ in 0..n {
            let quality = roll(&mut || rng.next_u64(), &table);
            total += u64::from(quality);
            let band = table
                .bands()
                .iter()
                .position(|band| (band.min..=band.max).contains(&quality))
                .expect("inside a band");
            counts[band] += 1;
            if (100..=104).contains(&quality) {
                hundreds[usize::from(quality - 100)] += 1;
            }
        }
        for (band, count) in table.bands().iter().zip(&counts) {
            let seen = f64::from(*count) / f64::from(n);
            assert!(
                (seen - band.chance).abs() < 0.003,
                "{}..={}: {seen} vs {}",
                band.min,
                band.max,
                band.chance
            );
        }
        let mean = total as f64 / f64::from(n);
        assert!((mean - table.expected_value()).abs() < 0.1, "mean {mean}");
        // Inside a band every whole percent is equally likely.
        let each = f64::from(hundreds.iter().sum::<u32>()) / 5.0;
        assert!(
            hundreds
                .iter()
                .all(|n| (f64::from(*n) - each).abs() < each * 0.1),
            "{hundreds:?}"
        );
    }

    #[test]
    fn a_reforge_pays_and_replaces_the_quality_even_for_the_worse() {
        let economy = economy();
        let mut loadout = loadout(110, 3, 1_000);
        let price = quote(&loadout, &slot(), &economy).expect("allowed").gold;
        let outcome = reforge(&mut loadout, &slot(), &mut || 0, &economy).expect("allowed");
        // A 110 can only get worse: there is no keep-best.
        assert_eq!((outcome.old_quality, outcome.new_quality), (110, 50));
        assert_eq!(loadout.equipment[0].quality, 50);
        assert_eq!(outcome.cost, price);
        assert_eq!(outcome.gold_left, 1_000 - price);
        assert_eq!(loadout.resources.get(GOLD), Some(&(1_000 - price)));
    }

    #[test]
    fn sharpening_is_untouched_and_effective_quality_is_base_plus_sharpening() {
        let economy = economy();
        let mut loadout = loadout(80, 4, 1_000);
        reforge(&mut loadout, &slot(), &mut || u64::MAX, &economy).expect("allowed");
        let item = &loadout.equipment[0];
        assert_eq!(
            (item.quality, item.sharpening, item.broken),
            (110, 4, false)
        );
        assert!((item.props().total_pct() - 114.0).abs() < 1e-4);
    }

    #[test]
    fn the_price_scales_with_item_level_and_sharpening() {
        let economy = economy();
        let level = item_by_key(KEY).expect("registered").item_level;
        ensure_items_registered();
        let plain = quote(&loadout(100, 0, 0), &slot(), &economy)
            .expect("ok")
            .gold;
        let sharp = quote(&loadout(100, 5, 0), &slot(), &economy)
            .expect("ok")
            .gold;
        let rule = economy.reforge.cost;
        assert_eq!(plain, rule.gold + rule.per_item_level * u64::from(level));
        assert_eq!(sharp, plain + 5 * rule.per_sharpening_level);
        assert!(plain > 0, "a gold sink, never free");
    }

    #[test]
    fn too_little_gold_refuses_and_changes_nothing() {
        let economy = economy();
        let price = quote(&loadout(70, 0, 0), &slot(), &economy)
            .expect("ok")
            .gold;
        let mut poor = loadout(70, 0, price - 1);
        let before = poor.clone();
        let mut drawn = false;
        let error = reforge(
            &mut poor,
            &slot(),
            &mut || {
                drawn = true;
                0
            },
            &economy,
        )
        .expect_err("short");
        assert_eq!(error, ReforgeError::Insufficient("Gold".into()));
        assert_eq!(poor, before);
        assert!(!drawn, "a refused reforge rolls nothing");
        // Exactly enough is enough, and empties the purse.
        let mut exact = loadout(70, 0, price);
        reforge(&mut exact, &slot(), &mut || 0, &economy).expect("exact");
        assert!(!exact.resources.contains_key(GOLD));
    }

    #[test]
    fn broken_empty_and_stackable_items_are_refused() {
        let economy = economy();
        let mut broken = loadout(70, 0, 1_000);
        broken.equipment[0].broken = true;
        assert_eq!(
            reforge(&mut broken, &slot(), &mut || 0, &economy),
            Err(ReforgeError::Broken)
        );
        let mut empty = loadout(70, 0, 1_000);
        let nowhere = SlotRef::Equipment {
            slot: "head".into(),
        };
        assert_eq!(
            reforge(&mut empty, &nowhere, &mut || 0, &economy),
            Err(ReforgeError::EmptySlot)
        );
        let mut stack = loadout(70, 0, 1_000);
        stack.inventory.push(BagItem::new(0, KEY, 2));
        assert_eq!(
            reforge(
                &mut stack,
                &SlotRef::Inventory { index: 0 },
                &mut || 0,
                &economy
            ),
            Err(ReforgeError::NotEquipment)
        );
        assert_eq!(stack.resources.get(GOLD), Some(&1_000));
    }

    #[test]
    fn an_item_in_the_bag_can_be_reforged() {
        let economy = economy();
        ensure_items_registered();
        let mut loadout = Loadout::default();
        let mut bag = BagItem::new(3, KEY, 1);
        bag.quality = 60;
        bag.sharpening = 2;
        loadout.inventory.push(bag);
        add_resources(&mut loadout, &[(GOLD.into(), 1_000)]);
        let outcome = reforge(
            &mut loadout,
            &SlotRef::Inventory { index: 3 },
            &mut || u64::MAX,
            &economy,
        )
        .expect("allowed");
        assert_eq!((outcome.old_quality, outcome.new_quality), (60, 110));
        assert_eq!(loadout.inventory[0].quality, 110);
        assert_eq!(loadout.inventory[0].sharpening, 2);
    }

    #[test]
    fn a_catalog_without_a_table_has_reforging_off() {
        let mut economy = economy();
        economy.reforge.bands.clear();
        assert_eq!(
            quote(&loadout(70, 0, 1_000), &slot(), &economy),
            Err(ReforgeError::Disabled)
        );
    }
}
