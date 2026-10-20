//! What a run drops beyond the chest: monster items and currency, and how the run's summary
//! carries the whole haul.
//!
//! ```text
//! server fight ─ WireEvent::MobLoot / MobCurrency ─► [`drops_in`] ─► [`Drop`]s (per account)
//!      realm module: item  ─► one durable `pending_loot` row, delivered to the mailbox
//!                    gold, aether shards ─► a per-player tally, folded into the run's result
//! run summary (client screen) ◄─ [`encode_items`] / [`encode_currencies`] text columns
//! ```
//!
//! The functions are pure and native-testable; the module only stores what they return. Nothing
//! here trusts a client: the events it reads come from the realm's own simulation.

use engine_game::character::{ItemProps, item_by_key};
use serde::{Deserialize, Serialize};

use crate::combat::{EntityRef, WireEvent};
use crate::loot::{builtin_rules, ensure_items_registered};

/// The most any single drop may be worth. A sanity bound on what the realm itself rolled.
const MAX_DROP_AMOUNT: u32 = 1_000_000;

/// One thing a monster dropped for one player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drop {
    /// A piece of equipment.
    Item {
        /// The player's account.
        account: String,
        /// The item's stable key.
        item: String,
        /// Its rolled properties.
        props: ItemProps,
    },
    /// A currency: gold or the modifier-reroll currency.
    Currency {
        /// The player's account.
        account: String,
        /// The resource's id in the economy catalog.
        resource: String,
        /// How many.
        amount: u32,
    },
}

/// The monster drops among a tick's `events`, in order. Drops for anything but a player, of an
/// item or resource this build does not know, or of nothing, are ignored.
#[must_use]
pub fn drops_in(events: &[WireEvent]) -> Vec<Drop> {
    ensure_items_registered();
    let rules = builtin_rules();
    let mut drops = Vec::new();
    for event in events {
        match event {
            WireEvent::MobLoot {
                player: EntityRef::Player(account),
                item,
                quality,
                sharpening,
                broken,
            } => {
                let Some(def) = item_by_key(item) else {
                    continue;
                };
                drops.push(Drop::Item {
                    account: account.clone(),
                    item: def.key.to_owned(),
                    props: ItemProps::new(*quality, *sharpening, *broken),
                });
            }
            WireEvent::MobCurrency {
                player: EntityRef::Player(account),
                resource,
                amount,
            } => {
                let known = rules.is_some_and(|rules| {
                    resource == rules.currency.resource || resource == rules.gold.resource
                });
                if known && *amount > 0 {
                    drops.push(Drop::Currency {
                        account: account.clone(),
                        resource: resource.clone(),
                        amount: (*amount).min(MAX_DROP_AMOUNT),
                    });
                }
            }
            _ => {}
        }
    }
    drops
}

/// Adds `amount` of `resource` to a tally, merging with an existing line. Saturates.
pub fn add_currency(tally: &mut Vec<(String, u32)>, resource: &str, amount: u32) {
    if let Some((_, held)) = tally.iter_mut().find(|(id, _)| id == resource) {
        *held = held.saturating_add(amount);
    } else {
        tally.push((resource.to_owned(), amount));
    }
}

/// The id of a boss-chest item: the player's `index`th roll of the instance with `uid`. The
/// delivery key the persistence API deduplicates on.
#[must_use]
pub fn chest_loot_id(uid: u128, account_id: &str, index: usize) -> String {
    format!(
        "{}:{account_id}:loot:{index}",
        crate::instance::uid_hex(uid)
    )
}

/// The id of a monster's drop: the player's `n`th (counting from zero) of the instance.
#[must_use]
pub fn mob_loot_id(uid: u128, account_id: &str, n: u32) -> String {
    format!("{}:{account_id}:mob:{n}", crate::instance::uid_hex(uid))
}

/// What one player has taken from monsters in one run, as the realm keeps it between ticks.
///
/// The item count doubles as the number of the next drop's id, so an id is never reused even
/// once earlier deliveries are acknowledged and gone. Currency joins a tally until the run's
/// result takes it ([`RunTally::settle`]); drops after that pay nothing more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunTally {
    /// Items dropped so far.
    pub items: u32,
    /// Currencies dropped so far, `(resource id, amount)`.
    pub currencies: Vec<(String, u32)>,
    /// Whether the result has taken the currencies.
    pub settled: bool,
}

impl RunTally {
    /// Counts one dropped item and returns its number (for [`mob_loot_id`]).
    pub fn add_item(&mut self) -> u32 {
        let n = self.items;
        self.items = self.items.saturating_add(1);
        n
    }

    /// Adds currency. Returns `false` (and adds nothing) once the tally is settled.
    pub fn add_currency(&mut self, resource: &str, amount: u32) -> bool {
        if self.settled {
            return false;
        }
        add_currency(&mut self.currencies, resource, amount);
        true
    }

    /// Hands the currencies to the run's result, once: a second call gets nothing.
    pub fn settle(&mut self) -> Vec<(String, u32)> {
        if self.settled {
            return Vec::new();
        }
        self.settled = true;
        std::mem::take(&mut self.currencies)
    }
}

/// A currency tally as the text column stores it: `[["gold",12],["aether_shard",1]]`.
#[must_use]
pub fn encode_currencies(tally: &[(String, u32)]) -> String {
    serde_json::to_string(tally).unwrap_or_else(|_| "[]".to_owned())
}

/// Reads [`encode_currencies`]' text. Empty or malformed text is an empty tally.
#[must_use]
pub fn decode_currencies(text: &str) -> Vec<(String, u32)> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(text).unwrap_or_default()
}

/// One item line of a run summary: what the chest or a monster gave, with its quality.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryItem {
    /// The item's stable key.
    pub item: String,
    /// Rolled quality, per cent of the base stats.
    pub quality: u8,
    /// Sharpening level (zero for anything just dropped).
    #[serde(default)]
    #[serde(rename = "enhancement", alias = "sharpening")]
    pub sharpening: u8,
    /// Whether it is broken.
    #[serde(default)]
    pub broken: bool,
}

impl SummaryItem {
    /// The line for `item` with `props`.
    #[must_use]
    pub fn new(item: &str, props: ItemProps) -> Self {
        Self {
            item: item.to_owned(),
            quality: props.quality,
            sharpening: props.sharpening,
            broken: props.broken,
        }
    }

    /// The item's properties, clamped into the legal ranges.
    #[must_use]
    pub fn props(&self) -> ItemProps {
        ItemProps::new(self.quality, self.sharpening, self.broken)
    }
}

/// Summary item lines as the text column stores them.
#[must_use]
pub fn encode_items(items: &[SummaryItem]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_owned())
}

/// Reads [`encode_items`]' text. Empty or malformed text is no items.
#[must_use]
pub fn decode_items(text: &str) -> Vec<SummaryItem> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(text).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_game::character::loot_pool;

    fn player(account: &str) -> EntityRef {
        EntityRef::Player(account.to_owned())
    }

    #[test]
    fn drops_are_read_per_player_and_unknowns_are_ignored() {
        let rules = builtin_rules().expect("the rules build");
        let key = loot_pool()[0].key.to_owned();
        let events = vec![
            WireEvent::MobLoot {
                player: player("a"),
                item: key.clone(),
                quality: 88,
                sharpening: 0,
                broken: false,
            },
            WireEvent::MobLoot {
                player: player("a"),
                item: "moon_cheese".into(),
                quality: 88,
                sharpening: 0,
                broken: false,
            },
            WireEvent::MobCurrency {
                player: player("b"),
                resource: rules.gold.resource.to_owned(),
                amount: 7,
            },
            WireEvent::MobCurrency {
                player: player("b"),
                resource: "moon_cheese".into(),
                amount: 7,
            },
            WireEvent::MobCurrency {
                player: EntityRef::Monster(3),
                resource: rules.gold.resource.to_owned(),
                amount: 7,
            },
            WireEvent::MobCurrency {
                player: player("b"),
                resource: rules.currency.resource.to_owned(),
                amount: 0,
            },
        ];
        let drops = drops_in(&events);
        assert_eq!(
            drops,
            vec![
                Drop::Item {
                    account: "a".into(),
                    item: key,
                    props: ItemProps::rolled(88),
                },
                Drop::Currency {
                    account: "b".into(),
                    resource: rules.gold.resource.to_owned(),
                    amount: 7,
                },
            ]
        );
    }

    #[test]
    fn tallies_merge_and_round_trip_through_text() {
        let mut tally = Vec::new();
        add_currency(&mut tally, "gold", 5);
        add_currency(&mut tally, "aether_shard", 1);
        add_currency(&mut tally, "gold", 7);
        assert_eq!(
            tally,
            vec![("gold".to_owned(), 12), ("aether_shard".to_owned(), 1)]
        );
        assert_eq!(decode_currencies(&encode_currencies(&tally)), tally);
        add_currency(&mut tally, "gold", u32::MAX);
        assert_eq!(tally[0].1, u32::MAX);
        assert!(decode_currencies("").is_empty());
        assert!(decode_currencies("{nope").is_empty());
    }

    #[test]
    fn ids_are_distinct_per_item_and_a_tally_pays_its_currency_once() {
        let hex = "00000000000000000000000000000007";
        assert_eq!(chest_loot_id(7, "a", 1), format!("{hex}:a:loot:1"));
        assert_ne!(chest_loot_id(7, "a", 0), chest_loot_id(7, "a", 1));
        assert_eq!(mob_loot_id(7, "a", 3), format!("{hex}:a:mob:3"));
        // A wiped realm restarts its instance ids, never its uids.
        assert_ne!(chest_loot_id(7, "a", 1), chest_loot_id(8, "a", 1));
        let mut tally = RunTally::default();
        assert_eq!(
            (tally.add_item(), tally.add_item(), tally.add_item()),
            (0, 1, 2)
        );
        assert!(tally.add_currency("gold", 5));
        assert!(tally.add_currency("gold", 6));
        assert!(tally.add_currency("aether_shard", 1));
        assert_eq!(
            tally.settle(),
            vec![("gold".to_owned(), 11), ("aether_shard".to_owned(), 1)]
        );
        // Folded into one result only; a late drop pays nothing, and items keep counting.
        assert!(tally.settle().is_empty());
        assert!(!tally.add_currency("gold", 9));
        assert_eq!(tally.add_item(), 3);
    }

    #[test]
    fn summary_items_round_trip_with_their_props() {
        let items = vec![
            SummaryItem::new("ashgraves_seal", ItemProps::rolled(106)),
            SummaryItem::new("gate_wardens_helm", ItemProps::new(70, 0, true)),
        ];
        let back = decode_items(&encode_items(&items));
        assert_eq!(back, items);
        assert_eq!(back[0].props(), ItemProps::rolled(106));
        assert!(back[1].props().broken);
        assert!(decode_items("").is_empty());
    }
}
