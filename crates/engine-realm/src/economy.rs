//! Resources: what a character holds, what salvage yields and what an exchange costs.
//!
//! ```text
//! bag item ──salvage──▶ resources ◀──run rewards (rewards.rs)
//!                          │ exchange (fixed rates, economy.json)
//!                          ▼
//!                       resources
//! ```
//!
//! Every function here edits a [`Loadout`] in place and either does all of it or none of it:
//! the persistence API runs them inside one locked read-modify-write of the character row, so
//! a refused salvage or exchange leaves nothing half done.

use std::fmt;

use engine_game::character::item_by_key;

use crate::catalog::Economy;
use crate::error::RealmError;
use crate::loadout::Loadout;

/// The most trades one exchange request may make.
pub const MAX_EXCHANGE_TIMES: u32 = 100;

/// Why a salvage or an exchange was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomyError {
    /// The bag cell holds nothing (or the address is not a bag cell).
    EmptyCell,
    /// The stored item no longer exists, so it has no level to salvage by.
    UnknownItem,
    /// No exchange has this id.
    UnknownExchange,
    /// The number of trades is outside `1..=`[`MAX_EXCHANGE_TIMES`].
    InvalidTimes,
    /// The character holds too little of this resource (its display name).
    Insufficient(String),
    /// The numbers do not fit, so the trade is refused rather than wrapped.
    Overflow,
}

impl fmt::Display for EconomyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCell => f.write_str("There is no item there to salvage."),
            Self::UnknownItem => f.write_str("That item cannot be salvaged."),
            Self::UnknownExchange => f.write_str("That exchange does not exist."),
            Self::InvalidTimes => write!(
                f,
                "You can exchange between 1 and {MAX_EXCHANGE_TIMES} times at once."
            ),
            Self::Insufficient(resource) => write!(f, "You do not have enough {resource}."),
            Self::Overflow => f.write_str("That is more than can be held."),
        }
    }
}

impl std::error::Error for EconomyError {}

impl From<EconomyError> for RealmError {
    fn from(error: EconomyError) -> Self {
        match error {
            EconomyError::UnknownExchange | EconomyError::InvalidTimes => {
                Self::protocol(error.to_string())
            }
            _ => Self::persistence(error.to_string()),
        }
    }
}

/// Adds `gains` to the loadout's resources, saturating at `u64::MAX`.
pub fn add_resources(loadout: &mut Loadout, gains: &[(String, u64)]) {
    for (id, amount) in gains {
        let held = loadout.resources.entry(id.clone()).or_insert(0);
        *held = held.saturating_add(*amount);
    }
}

/// Breaks one item of the bag stack in `index` down into the salvage resource.
///
/// Only the bag: worn gear is never salvaged. Removes one of the stack (the cell empties when
/// the last one goes) and pays `base + per_item_level × item level`. Returns the resource id
/// and the amount.
///
/// # Errors
///
/// [`EconomyError::EmptyCell`] for an empty cell, [`EconomyError::UnknownItem`] for a stored
/// item that no longer exists (the loadout is left as it was).
pub fn salvage(
    loadout: &mut Loadout,
    index: u32,
    economy: &Economy,
) -> Result<(String, u64), EconomyError> {
    let position = loadout
        .inventory
        .iter()
        .position(|entry| entry.index == index && entry.quantity > 0)
        .ok_or(EconomyError::EmptyCell)?;
    let def = item_by_key(&loadout.inventory[position].item).ok_or(EconomyError::UnknownItem)?;
    let rule = &economy.salvage;
    let amount = u64::from(rule.base)
        .saturating_add(u64::from(rule.per_item_level) * u64::from(def.item_level));

    let entry = &mut loadout.inventory[position];
    entry.quantity -= 1;
    if entry.quantity == 0 {
        loadout.inventory.remove(position);
    }
    add_resources(loadout, &[(rule.resource.clone(), amount)]);
    Ok((rule.resource.clone(), amount))
}

/// Makes the exchange `exchange_id` `times` times over.
///
/// # Errors
///
/// [`EconomyError::InvalidTimes`] outside `1..=`[`MAX_EXCHANGE_TIMES`],
/// [`EconomyError::UnknownExchange`], [`EconomyError::Insufficient`] when the character cannot
/// pay, [`EconomyError::Overflow`] when the result would not fit. The loadout is unchanged on
/// every error.
pub fn exchange(
    loadout: &mut Loadout,
    exchange_id: &str,
    times: u32,
    economy: &Economy,
) -> Result<(), EconomyError> {
    if !(1..=MAX_EXCHANGE_TIMES).contains(&times) {
        return Err(EconomyError::InvalidTimes);
    }
    let rule = economy
        .exchange(exchange_id)
        .ok_or(EconomyError::UnknownExchange)?;
    let cost = u64::from(rule.from_amount)
        .checked_mul(u64::from(times))
        .ok_or(EconomyError::Overflow)?;
    let gain = u64::from(rule.to_amount)
        .checked_mul(u64::from(times))
        .ok_or(EconomyError::Overflow)?;
    let held = loadout.resources.get(&rule.from).copied().unwrap_or(0);
    let remaining = held.checked_sub(cost).ok_or_else(|| {
        EconomyError::Insufficient(
            economy
                .resource(&rule.from)
                .map_or_else(|| rule.from.clone(), |resource| resource.name.clone()),
        )
    })?;
    let received = loadout
        .resources
        .get(&rule.to)
        .copied()
        .unwrap_or(0)
        .checked_add(gain)
        .ok_or(EconomyError::Overflow)?;
    if remaining == 0 {
        loadout.resources.remove(&rule.from);
    } else {
        loadout.resources.insert(rule.from.clone(), remaining);
    }
    loadout.resources.insert(rule.to.clone(), received);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::loadout::{BagItem, EquippedItem};

    fn economy() -> Economy {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .economy()
            .clone()
    }

    fn item() -> &'static engine_game::character::ItemDef {
        &engine_game::character::loot_pool()[0]
    }

    fn loadout_with_stack(quantity: u16) -> Loadout {
        let mut loadout = Loadout::default();
        loadout.inventory.push(BagItem {
            index: 4,
            item: item().key.into(),
            quantity,
        });
        loadout
    }

    #[test]
    fn salvage_pays_base_plus_item_level_and_removes_one() {
        let economy = economy();
        let mut loadout = loadout_with_stack(2);
        let (resource, amount) = salvage(&mut loadout, 4, &economy).expect("a bag item");
        assert_eq!(resource, "bone_shard");
        assert_eq!(
            amount,
            u64::from(economy.salvage.base)
                + u64::from(economy.salvage.per_item_level) * u64::from(item().item_level)
        );
        assert_eq!(loadout.inventory[0].quantity, 1);
        assert_eq!(loadout.resources.get("bone_shard"), Some(&amount));
        // The last one empties the cell.
        salvage(&mut loadout, 4, &economy).expect("the last one");
        assert!(loadout.inventory.is_empty());
        assert_eq!(loadout.resources.get("bone_shard"), Some(&(amount * 2)));
    }

    #[test]
    fn salvage_refuses_empty_cells_worn_gear_and_unknown_items() {
        let economy = economy();
        let mut loadout = loadout_with_stack(1);
        loadout.equipment.push(EquippedItem {
            slot: "chest".into(),
            item: item().key.into(),
        });
        // Worn gear is not in the bag, so nothing addresses it.
        assert_eq!(
            salvage(&mut loadout, 5, &economy),
            Err(EconomyError::EmptyCell)
        );
        loadout.inventory.push(BagItem {
            index: 9,
            item: "no_such_item".into(),
            quantity: 1,
        });
        let before_unknown = loadout.clone();
        assert_eq!(
            salvage(&mut loadout, 9, &economy),
            Err(EconomyError::UnknownItem)
        );
        assert_eq!(loadout, before_unknown);
        assert_eq!(loadout.equipment.len(), 1);
        assert!(loadout.resources.is_empty());
    }

    #[test]
    fn exchange_trades_at_the_catalog_rate() {
        let economy = economy();
        let mut loadout = Loadout::default();
        add_resources(&mut loadout, &[("crypt_dust".into(), 45)]);
        exchange(&mut loadout, "dust_to_ember", 2, &economy).expect("enough dust");
        assert_eq!(loadout.resources.get("crypt_dust"), Some(&5));
        assert_eq!(loadout.resources.get("steward_ember"), Some(&2));
        // Spending everything removes the entry.
        let mut exact = Loadout::default();
        add_resources(&mut exact, &[("crypt_dust".into(), 20)]);
        exchange(&mut exact, "dust_to_ember", 1, &economy).expect("exactly enough");
        assert!(!exact.resources.contains_key("crypt_dust"));
    }

    #[test]
    fn exchange_refuses_what_it_cannot_do_and_changes_nothing() {
        let economy = economy();
        let mut loadout = Loadout::default();
        add_resources(&mut loadout, &[("crypt_dust".into(), 19)]);
        let before = loadout.clone();
        assert_eq!(
            exchange(&mut loadout, "dust_to_ember", 1, &economy),
            Err(EconomyError::Insufficient("Crypt Dust".into()))
        );
        assert_eq!(
            exchange(&mut loadout, "nope", 1, &economy),
            Err(EconomyError::UnknownExchange)
        );
        for times in [0, MAX_EXCHANGE_TIMES + 1] {
            assert_eq!(
                exchange(&mut loadout, "dust_to_ember", times, &economy),
                Err(EconomyError::InvalidTimes)
            );
        }
        assert_eq!(loadout, before);
        // The ceiling itself is allowed when affordable.
        add_resources(&mut loadout, &[("crypt_dust".into(), 2_100)]);
        exchange(&mut loadout, "dust_to_ember", MAX_EXCHANGE_TIMES, &economy).expect("100 trades");
        // A full purse of the received resource refuses instead of wrapping.
        loadout.resources.insert("steward_ember".into(), u64::MAX);
        assert_eq!(
            exchange(&mut loadout, "dust_to_ember", 1, &economy),
            Err(EconomyError::Overflow)
        );
    }

    #[test]
    fn adding_resources_saturates() {
        let mut loadout = Loadout::default();
        add_resources(&mut loadout, &[("crypt_dust".into(), u64::MAX)]);
        add_resources(&mut loadout, &[("crypt_dust".into(), 5)]);
        assert_eq!(loadout.resources.get("crypt_dust"), Some(&u64::MAX));
    }

    #[test]
    fn errors_read_as_sentences_and_convert() {
        let error: RealmError = EconomyError::Insufficient("Crypt Dust".into()).into();
        assert_eq!(error.message, "You do not have enough Crypt Dust.");
        assert_eq!(error.category, crate::ErrorCategory::Persistence);
        let error: RealmError = EconomyError::InvalidTimes.into();
        assert_eq!(error.category, crate::ErrorCategory::Protocol);
    }
}
