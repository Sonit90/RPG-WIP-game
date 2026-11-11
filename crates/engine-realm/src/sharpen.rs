//! Sharpening equipment: pay a price, risk the item, gain a per cent of its stats.
//!
//! ```text
//! sharpen(loadout, slot, roll, economy)
//!    refuse: empty / not equipment / broken / already at the maximum / cannot afford
//!    pay the whole price                      (always, whatever follows)
//!    roll < break_chance(target level) ─► Broken   quality kept, sharpening lost, -15 %
//!    otherwise                         ─► Success  sharpening + 1  (+1 % of the base stats)
//! ```
//!
//! Pure rules over a [`Loadout`]: the persistence API runs [`sharpen`] inside one locked
//! read-modify-write of the character row, supplying the roll from the server's own random
//! source, so a client can neither choose its roll nor skip the price. Nothing here reads a
//! clock or a random source.
//!
//! The first levels are safe (their break chance is zero) and the last ones very nearly are
//! not; the numbers are `economy.json`'s `sharpening` block. What a level is *worth* is not
//! decided here: `engine_game::character::ItemProps` scales the stats, and
//! [`crate::catalog::Economy`] refuses to load unless its `step_pct` agrees with it.

use std::fmt;

use engine_game::character::{ItemProps, item_by_key};

use crate::catalog::Economy;
use crate::error::RealmError;
use crate::loadout::{BagItem, EquippedItem, Loadout, SlotRef};
use crate::loot::ensure_items_registered;

/// What an attempt did to the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharpenOutcome {
    /// The item is now this sharpening level.
    Success {
        /// Its new level.
        new_level: u8,
    },
    /// The item broke: its sharpening is gone and it grants 85 per cent of its quality.
    Broken,
}

/// Why an attempt was refused. Nothing was paid or changed in any of these cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharpenError {
    /// The address names nothing: an unknown slot, or an empty one.
    EmptySlot,
    /// The item is not equipment that can be sharpened: unknown, stackable, or granting no stats.
    NotEquipment,
    /// A broken item cannot be sharpened again.
    CannotSharpenBroken,
    /// The item is already at the maximum level (or sharpening is switched off).
    MaxLevel,
    /// The character cannot pay; carries the display name of the first resource short.
    Insufficient(String),
    /// The roll was not a number in `[0, 1)`: a caller bug, not a player's mistake.
    InvalidRoll,
    /// The price overflows what a purse can hold.
    Overflow,
}

impl fmt::Display for SharpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySlot => f.write_str("There is no item there to sharpen."),
            Self::NotEquipment => f.write_str("That item cannot be sharpened."),
            Self::CannotSharpenBroken => f.write_str("A broken item cannot be sharpened."),
            Self::MaxLevel => f.write_str("That item is already fully sharpened."),
            Self::Insufficient(resource) => write!(f, "You do not have enough {resource}."),
            Self::InvalidRoll => f.write_str("The sharpening could not be rolled."),
            Self::Overflow => f.write_str("That is more than can be held."),
        }
    }
}

impl std::error::Error for SharpenError {}

impl From<SharpenError> for RealmError {
    fn from(error: SharpenError) -> Self {
        match error {
            SharpenError::InvalidRoll => Self::protocol(error.to_string()),
            _ => Self::persistence(error.to_string()),
        }
    }
}

/// What the attempt to take the item at `slot` one level higher costs and risks, for a tooltip
/// or a confirmation.
///
/// # Errors
///
/// The refusals [`sharpen`] would give for the item itself ([`SharpenError::EmptySlot`],
/// [`SharpenError::NotEquipment`], [`SharpenError::CannotSharpenBroken`],
/// [`SharpenError::MaxLevel`]); affordability is not checked.
pub fn preview(
    loadout: &Loadout,
    slot: &SlotRef,
    economy: &Economy,
) -> Result<SharpenPreview, SharpenError> {
    ensure_items_registered();
    let (props, _) = locate(loadout, slot)?;
    let target = target_level(props, economy)?;
    let price = economy
        .sharpening
        .costs_for(target)
        .map(|cost| (cost.resource.clone(), cost.amount))
        .collect();
    Ok(SharpenPreview {
        target,
        break_chance: economy.sharpening.break_chance(target),
        price,
    })
}

/// The level an attempt on an item with `props` would aim for, if it may be attempted.
fn target_level(props: ItemProps, economy: &Economy) -> Result<u8, SharpenError> {
    if props.broken {
        return Err(SharpenError::CannotSharpenBroken);
    }
    let target = props.sharpening.saturating_add(1);
    if target > economy.sharpening.max_level {
        return Err(SharpenError::MaxLevel);
    }
    Ok(target)
}

/// What one more level costs and risks.
#[derive(Debug, Clone, PartialEq)]
pub struct SharpenPreview {
    /// The level the attempt aims for.
    pub target: u8,
    /// The chance the attempt breaks the item, `0..=1`.
    pub break_chance: f64,
    /// The price, as `(resource id, amount)`.
    pub price: Vec<(String, u64)>,
}

/// A sharpenable item's props and where it is stored.
pub(crate) enum Place {
    Worn(usize),
    Bag(usize),
}

/// Finds the item at `slot` and checks it is sharpenable equipment.
pub(crate) fn locate(
    loadout: &Loadout,
    slot: &SlotRef,
) -> Result<(ItemProps, Place), SharpenError> {
    let (key, props, place) = match slot {
        SlotRef::Equipment { slot } => {
            let index = loadout
                .equipment
                .iter()
                .position(|entry| entry.slot == *slot)
                .ok_or(SharpenError::EmptySlot)?;
            let entry: &EquippedItem = &loadout.equipment[index];
            (&entry.item, entry.props(), Place::Worn(index))
        }
        SlotRef::Inventory { index } => {
            let position = loadout
                .inventory
                .iter()
                .position(|entry| entry.index == *index && entry.quantity > 0)
                .ok_or(SharpenError::EmptySlot)?;
            let entry: &BagItem = &loadout.inventory[position];
            if entry.quantity != 1 {
                return Err(SharpenError::NotEquipment);
            }
            (&entry.item, entry.props(), Place::Bag(position))
        }
    };
    let def = item_by_key(key).ok_or(SharpenError::NotEquipment)?;
    if def.max_stack > 1 || def.grants_nothing() {
        return Err(SharpenError::NotEquipment);
    }
    Ok((props, place))
}

/// **One sharpening attempt** on the item at `slot` (worn or in the bag).
///
/// `roll` is a number in `[0, 1)` the *caller* drew from a server-side random source; the
/// attempt breaks the item when `roll < break_chance(target level)`, so a level with a break
/// chance of zero is safe whatever the roll. The whole price is paid on every attempt that is
/// allowed, success or break, and an attempt that is refused pays nothing.
///
/// # Errors
///
/// [`SharpenError::EmptySlot`], [`SharpenError::NotEquipment`],
/// [`SharpenError::CannotSharpenBroken`], [`SharpenError::MaxLevel`],
/// [`SharpenError::Insufficient`] when any resource of the price is short (nothing is
/// deducted), [`SharpenError::InvalidRoll`]. The loadout is unchanged on every error.
pub fn sharpen(
    loadout: &mut Loadout,
    slot: &SlotRef,
    roll: f64,
    economy: &Economy,
) -> Result<SharpenOutcome, SharpenError> {
    ensure_items_registered();
    if !(0.0..1.0).contains(&roll) {
        return Err(SharpenError::InvalidRoll);
    }
    let (props, place) = locate(loadout, slot)?;
    let target = target_level(props, economy)?;

    // All or nothing: check the whole price before taking any of it.
    let price: Vec<_> = economy.sharpening.costs_for(target).collect();
    for cost in &price {
        if loadout.resources.get(&cost.resource).copied().unwrap_or(0) < cost.amount {
            return Err(SharpenError::Insufficient(
                economy
                    .resource(&cost.resource)
                    .map_or_else(|| cost.resource.clone(), |resource| resource.name.clone()),
            ));
        }
    }
    for cost in price {
        let held = loadout.resources.get(&cost.resource).copied().unwrap_or(0);
        match held - cost.amount {
            0 => {
                loadout.resources.remove(&cost.resource);
            }
            left => {
                loadout.resources.insert(cost.resource.clone(), left);
            }
        }
    }

    let broke = roll < economy.sharpening.break_chance(target);
    let after = if broke {
        ItemProps::new(props.quality, 0, true)
    } else {
        ItemProps::new(props.quality, target, false)
    };
    match place {
        Place::Worn(index) => set_worn(&mut loadout.equipment[index], after),
        Place::Bag(index) => set_bag(&mut loadout.inventory[index], after),
    }
    Ok(if broke {
        SharpenOutcome::Broken
    } else {
        SharpenOutcome::Success { new_level: target }
    })
}

fn set_worn(entry: &mut EquippedItem, props: ItemProps) {
    (entry.quality, entry.sharpening, entry.broken) =
        (props.quality, props.sharpening, props.broken);
}

fn set_bag(entry: &mut BagItem, props: ItemProps) {
    (entry.quality, entry.sharpening, entry.broken) =
        (props.quality, props.sharpening, props.broken);
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::economy::add_resources;
    use engine_game::character::{ItemStack, Stat};

    fn economy() -> Economy {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .economy()
            .clone()
    }

    const KEY: &str = "ashgraves_seal";

    fn worn(props: ItemProps) -> Loadout {
        let mut loadout = Loadout::default();
        let mut entry = EquippedItem::new("ring1", KEY);
        set_worn(&mut entry, props);
        loadout.equipment.push(entry);
        loadout
    }

    fn slot() -> SlotRef {
        SlotRef::Equipment {
            slot: "ring1".into(),
        }
    }

    fn rich() -> Loadout {
        let mut loadout = worn(ItemProps::default());
        add_resources(
            &mut loadout,
            &[
                ("crypt_dust".into(), 100_000),
                ("steward_ember".into(), 100),
            ],
        );
        loadout
    }

    fn props_of(loadout: &Loadout) -> ItemProps {
        loadout.equipment[0].props()
    }

    fn main_stat_ratio(props: ItemProps) -> f32 {
        let def = item_by_key(KEY).expect("registered");
        ItemStack::with_props(def.id, props)
            .stats()
            .get(Stat::MainStat)
            / def.stats.get(Stat::MainStat)
    }

    #[test]
    fn each_success_adds_exactly_one_per_cent_of_the_base_stats() {
        let economy = economy();
        let mut loadout = rich();
        let before = main_stat_ratio(props_of(&loadout));
        // Roll just under one: the safe levels cannot break whatever it is.
        let outcome = sharpen(&mut loadout, &slot(), 0.999, &economy).expect("allowed");
        assert_eq!(outcome, SharpenOutcome::Success { new_level: 1 });
        assert_eq!(props_of(&loadout).sharpening, 1);
        assert!((main_stat_ratio(props_of(&loadout)) - before - 0.01).abs() < 1e-5);
    }

    #[test]
    fn the_first_four_levels_are_safe_and_the_rest_break_at_the_configured_chance() {
        let economy = economy();
        let chances = [0.95, 0.96, 0.97, 0.98, 0.99, 0.995];
        for (index, chance) in chances.into_iter().enumerate() {
            let target = u8::try_from(index + 5).expect("small");
            let at_previous = ItemProps::new(100, target - 1, false);
            // A roll just under the chance breaks; one at or over it succeeds.
            let mut breaks = worn(at_previous);
            let mut holds = worn(at_previous);
            for loadout in [&mut breaks, &mut holds] {
                add_resources(
                    loadout,
                    &[("crypt_dust".into(), 10_000), ("steward_ember".into(), 50)],
                );
            }
            assert_eq!(
                sharpen(&mut breaks, &slot(), chance - 1e-9, &economy),
                Ok(SharpenOutcome::Broken)
            );
            assert_eq!(
                sharpen(&mut holds, &slot(), chance, &economy),
                Ok(SharpenOutcome::Success { new_level: target })
            );
        }
        for target in 1..=4 {
            let mut loadout = worn(ItemProps::new(100, target - 1, false));
            add_resources(&mut loadout, &[("crypt_dust".into(), 10_000)]);
            assert_eq!(
                sharpen(&mut loadout, &slot(), 0.0, &economy),
                Ok(SharpenOutcome::Success { new_level: target }),
                "+{target} is safe even on the worst roll"
            );
        }
    }

    #[test]
    fn a_break_costs_the_sharpening_and_leaves_eighty_five_per_cent_of_the_quality() {
        let economy = economy();
        let mut loadout = rich();
        loadout.equipment[0].quality = 110;
        loadout.equipment[0].sharpening = 6;
        assert_eq!(
            sharpen(&mut loadout, &slot(), 0.0, &economy),
            Ok(SharpenOutcome::Broken)
        );
        let after = props_of(&loadout);
        assert_eq!(after, ItemProps::new(110, 0, true));
        assert!((after.total_pct() - 93.5).abs() < 1e-4);
        // At quality 100 and no sharpening, a broken item is exactly 85 per cent.
        assert!((ItemProps::new(100, 0, true).multiplier() - 0.85).abs() < 1e-6);
    }

    #[test]
    fn a_broken_item_cannot_be_sharpened_and_nothing_is_paid() {
        let economy = economy();
        let mut loadout = rich();
        loadout.equipment[0].broken = true;
        let before = loadout.clone();
        assert_eq!(
            sharpen(&mut loadout, &slot(), 0.5, &economy),
            Err(SharpenError::CannotSharpenBroken)
        );
        assert_eq!(loadout, before);
    }

    #[test]
    fn the_maximum_level_stops_further_attempts() {
        let economy = economy();
        let mut loadout = rich();
        loadout.equipment[0].sharpening = economy.sharpening.max_level;
        let before = loadout.clone();
        assert_eq!(
            sharpen(&mut loadout, &slot(), 0.5, &economy),
            Err(SharpenError::MaxLevel)
        );
        assert_eq!(loadout, before);
    }

    #[test]
    fn the_price_is_paid_on_success_and_on_a_break() {
        let economy = economy();
        for (target, roll) in [(1_u8, 0.9), (5, 0.0), (5, 0.999)] {
            let mut loadout = worn(ItemProps::new(100, target - 1, false));
            add_resources(
                &mut loadout,
                &[("crypt_dust".into(), 1_000), ("steward_ember".into(), 10)],
            );
            let price: Vec<_> = economy.sharpening.costs_for(target).cloned().collect();
            assert!(!price.is_empty());
            let before = loadout.resources.clone();
            sharpen(&mut loadout, &slot(), roll, &economy).expect("affordable");
            for cost in price {
                assert_eq!(
                    loadout.resources.get(&cost.resource).copied().unwrap_or(0),
                    before[&cost.resource] - cost.amount,
                    "{} at +{target}",
                    cost.resource
                );
            }
        }
    }

    #[test]
    fn an_attempt_that_cannot_be_paid_for_is_refused_whole() {
        let economy = economy();
        // +5 costs dust and an ember: enough dust, no ember.
        let mut loadout = worn(ItemProps::new(100, 4, false));
        add_resources(&mut loadout, &[("crypt_dust".into(), 5_000)]);
        let before = loadout.clone();
        assert_eq!(
            sharpen(&mut loadout, &slot(), 0.5, &economy),
            Err(SharpenError::Insufficient("Steward's Ember".into()))
        );
        assert_eq!(
            loadout, before,
            "the dust was taken though the attempt was refused"
        );
        // And nothing at all in the purse.
        let mut empty = worn(ItemProps::default());
        assert_eq!(
            sharpen(&mut empty, &slot(), 0.5, &economy),
            Err(SharpenError::Insufficient("Crypt Dust".into()))
        );
    }

    #[test]
    fn only_sharpenable_equipment_can_be_sharpened() {
        let economy = economy();
        let mut loadout = rich();
        // Nothing there; an unknown slot; a white piece that grants nothing; an unknown item.
        let empty = SlotRef::Equipment {
            slot: "head".into(),
        };
        assert_eq!(
            sharpen(&mut loadout, &empty, 0.5, &economy),
            Err(SharpenError::EmptySlot)
        );
        let cell = SlotRef::Inventory { index: 3 };
        assert_eq!(
            sharpen(&mut loadout, &cell, 0.5, &economy),
            Err(SharpenError::EmptySlot)
        );
        loadout
            .inventory
            .push(BagItem::new(3, "simple_cloth_hood", 1));
        assert_eq!(
            sharpen(&mut loadout, &cell, 0.5, &economy),
            Err(SharpenError::NotEquipment)
        );
        loadout.inventory[0].item = "no_such_item".into();
        assert_eq!(
            sharpen(&mut loadout, &cell, 0.5, &economy),
            Err(SharpenError::NotEquipment)
        );
    }

    #[test]
    fn a_bag_item_can_be_sharpened_too() {
        let economy = economy();
        let mut loadout = rich();
        loadout.inventory.push(BagItem::new(2, KEY, 1));
        let cell = SlotRef::Inventory { index: 2 };
        assert_eq!(
            sharpen(&mut loadout, &cell, 0.9, &economy),
            Ok(SharpenOutcome::Success { new_level: 1 })
        );
        assert_eq!(loadout.inventory[0].sharpening, 1);
        assert_eq!(loadout.equipment[0].sharpening, 0);
    }

    #[test]
    fn a_bad_roll_is_refused() {
        let economy = economy();
        let mut loadout = rich();
        for roll in [-0.1, 1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                sharpen(&mut loadout, &slot(), roll, &economy),
                Err(SharpenError::InvalidRoll)
            );
        }
    }

    #[test]
    fn a_preview_states_the_target_the_chance_and_the_price() {
        let economy = economy();
        let loadout = worn(ItemProps::new(100, 4, false));
        let shown = preview(&loadout, &slot(), &economy).expect("allowed");
        assert_eq!(shown.target, 5);
        assert!((shown.break_chance - 0.95).abs() < 1e-12);
        assert_eq!(
            shown.price,
            vec![
                ("crypt_dust".to_owned(), 75),
                ("steward_ember".to_owned(), 1)
            ]
        );
    }

    #[test]
    fn errors_read_as_sentences_and_convert() {
        let error: RealmError = SharpenError::Insufficient("Crypt Dust".into()).into();
        assert_eq!(error.message, "You do not have enough Crypt Dust.");
        assert_eq!(error.category, crate::ErrorCategory::Persistence);
        let error: RealmError = SharpenError::InvalidRoll.into();
        assert_eq!(error.category, crate::ErrorCategory::Protocol);
    }
}
