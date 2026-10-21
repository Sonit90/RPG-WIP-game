//! The equipment-set catalog: `assets/catalog/sets.json`.
//!
//! ```text
//! sets.json ──► SetCatalog::parse(text, &LootCatalog)   (serde, validated both ways)
//!                  └─► engine_game::character::register_sets
//! ```
//!
//! Compiled into every program that reads it, like the other catalogs, so the client, the
//! realm module and the API agree on every set. `engine-game` computes what a set does; this
//! module only reads the file and refuses a file that contradicts the item catalog:
//!
//! * every piece names an item that exists, and that item's `set` names this set;
//! * every item that names a set is listed by that set;
//! * bonus counts ascend, none above the piece count, and every piece count is reachable
//!   (a set cannot need two helms);
//! * stats, abilities and effect kinds are ones the game has.

use std::collections::BTreeMap;

use engine_game::character::{
    ItemFamily, SetBonus, SetDef, SetEffect, Stat, register_sets,
};
use serde::Deserialize;

use crate::catalog::{CATALOG_VERSION, CatalogError};
use crate::loot::LootCatalog;

/// The set catalog as written.
pub(crate) const SETS_JSON: &str = include_str!("../../../assets/catalog/sets.json");

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct EffectRecord {
    kind: String,
    #[serde(default)]
    ability: Option<String>,
    value: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct BonusRecord {
    count: u8,
    #[serde(default)]
    stats: BTreeMap<String, f32>,
    #[serde(default)]
    effects: Vec<EffectRecord>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct SetRecord {
    id: String,
    name: String,
    pieces: Vec<String>,
    bonuses: Vec<BonusRecord>,
}

/// The whole set catalog, parsed and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCatalog {
    /// Every set, in file order.
    pub sets: Vec<SetDef>,
}

#[derive(Deserialize)]
struct SetFile {
    version: u32,
    sets: Vec<SetRecord>,
}

fn err(message: impl Into<String>) -> CatalogError {
    CatalogError(format!("sets.json: {}", message.into()))
}

impl SetCatalog {
    /// Reads `text` and checks it against `loot`.
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version, a duplicate set, a set that fails
    /// [`SetDef::validate`], an unknown stat, ability or effect kind, a piece that is not an
    /// item (or is an item of another set), an item naming a set that does not list it, or a
    /// bonus tier that no combination of slots can reach.
    pub fn parse(text: &str, loot: &LootCatalog) -> Result<Self, CatalogError> {
        let file: SetFile = serde_json::from_str(text).map_err(|error| err(error.to_string()))?;
        if file.version != CATALOG_VERSION {
            return Err(err(format!(
                "this build reads catalog version {CATALOG_VERSION}"
            )));
        }
        let mut sets: Vec<SetDef> = Vec::with_capacity(file.sets.len());
        for record in file.sets {
            if sets.iter().any(|set| set.id == record.id) {
                return Err(err(format!("set {:?} is listed twice", record.id)));
            }
            let mut bonuses = Vec::with_capacity(record.bonuses.len());
            for bonus in &record.bonuses {
                let mut stats = Vec::with_capacity(bonus.stats.len());
                for (name, value) in &bonus.stats {
                    let stat = Stat::from_key(name).ok_or_else(|| {
                        err(format!("set {:?} has unknown stat {name:?}", record.id))
                    })?;
                    stats.push((stat, *value));
                }
                let mut effects = Vec::with_capacity(bonus.effects.len());
                for effect in &bonus.effects {
                    effects.push(
                        SetEffect::parse(&effect.kind, effect.ability.as_deref(), effect.value)
                            .map_err(|why| err(format!("set {:?}: {why}", record.id)))?,
                    );
                }
                bonuses.push(SetBonus {
                    count: bonus.count,
                    stats,
                    effects,
                });
            }
            let set = SetDef {
                id: record.id,
                name: record.name,
                pieces: record.pieces,
                bonuses,
            };
            set.validate().map_err(err)?;
            sets.push(set);
        }
        let catalog = Self { sets };
        catalog.check_against(loot)?;
        Ok(catalog)
    }

    /// The two-way check between sets and items.
    fn check_against(&self, loot: &LootCatalog) -> Result<(), CatalogError> {
        for set in &self.sets {
            let mut per_family: Vec<(ItemFamily, usize)> = Vec::new();
            for piece in &set.pieces {
                let row = loot.items.iter().find(|row| &row.id == piece).ok_or_else(|| {
                    err(format!("set {:?} names unknown item {piece:?}", set.id))
                })?;
                if row.set.as_deref() != Some(set.id.as_str()) {
                    return Err(err(format!(
                        "item {piece:?} is a piece of set {:?} but its set is {:?}",
                        set.id, row.set
                    )));
                }
                if let Some(family) = ItemFamily::from_key(&row.slot) {
                    match per_family.iter_mut().find(|(known, _)| *known == family) {
                        Some((_, count)) => *count += 1,
                        None => per_family.push((family, 1)),
                    }
                }
            }
            for (family, count) in per_family {
                if count > family.slots().count() {
                    return Err(err(format!(
                        "set {:?} has {count} {} pieces but only {} fit on a body",
                        set.id,
                        family.name(),
                        family.slots().count()
                    )));
                }
            }
        }
        for row in &loot.items {
            if let Some(id) = &row.set
                && !self
                    .sets
                    .iter()
                    .any(|set| &set.id == id && set.has_piece(&row.id))
            {
                return Err(err(format!(
                    "item {:?} names set {id:?}, which does not list it",
                    row.id
                )));
            }
        }
        Ok(())
    }

    /// Registers the sets with `engine-game` (idempotent; the first in the process wins).
    pub fn register(&self) {
        register_sets(self.sets.clone());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::loot::LOOT_JSON;
    use engine_game::character::{
        EquipSlot, ItemStack, active_sets, item_by_key,
    };
    use engine_game::hero::Hero;

    fn loot() -> LootCatalog {
        LootCatalog::parse(LOOT_JSON).expect("the shipped loot parses")
    }

    fn shipped() -> SetCatalog {
        SetCatalog::parse(SETS_JSON, &loot()).expect("the shipped sets parse")
    }

    fn sets_with(from: &str, to: &str) -> String {
        assert!(SETS_JSON.contains(from), "{from} is not in the shipped file");
        SETS_JSON.replacen(from, to, 1)
    }

    #[test]
    fn the_shipped_sets_read_and_cover_every_class() {
        let catalog = shipped();
        let majors = catalog.sets.iter().filter(|set| set.pieces.len() >= 4);
        let minors = catalog.sets.iter().filter(|set| set.pieces.len() == 2);
        assert!(minors.count() >= 2);
        let majors: Vec<_> = majors.collect();
        for hero in Hero::ALL {
            assert!(
                majors.iter().any(|set| {
                    set.bonuses
                        .iter()
                        .flat_map(|bonus| &bonus.effects)
                        .filter_map(|effect| effect.ability())
                        .any(|ability| ability.hero() == hero)
                }),
                "{hero:?} has no major set"
            );
        }
        for set in &majors {
            assert!(set.bonuses.len() >= 3, "{} lacks tiers", set.id);
        }
    }

    #[test]
    fn every_piece_exists_and_names_its_set_and_every_set_item_is_listed() {
        let loot = loot();
        let catalog = shipped();
        for set in &catalog.sets {
            for piece in &set.pieces {
                let row = loot.items.iter().find(|row| &row.id == piece).expect("piece");
                assert_eq!(row.set.as_deref(), Some(set.id.as_str()));
            }
        }
        for row in loot.items.iter().filter(|row| row.set.is_some()) {
            assert!(catalog.sets.iter().any(|set| set.has_piece(&row.id)));
        }
    }

    #[test]
    fn every_set_piece_can_actually_drop() {
        let loot = loot();
        for set in &shipped().sets {
            for piece in &set.pieces {
                assert!(
                    loot.boss_tables
                        .values()
                        .any(|table| table.iter().any(|entry| &entry.item == piece)),
                    "{piece} is in no boss table"
                );
            }
        }
    }

    #[test]
    fn bad_set_files_are_refused() {
        let loot = loot();
        for (name, bad) in [
            ("unknown piece", sets_with("\"candlelit_vow_band\", \"candlelit_vow_locket\"", "\"candlelit_vow_band\", \"nope\"")),
            ("descending counts", sets_with("\"count\": 4", "\"count\": 1")),
            ("count above pieces", sets_with("\"count\": 5", "\"count\": 6")),
            ("unknown stat", sets_with("\"critical_strike\": 6", "\"luck\": 6")),
            ("unknown ability", sets_with("\"firebolt\"", "\"fireball\"")),
            ("unknown kind", sets_with("\"damage_pct\"", "\"extra_charge\"")),
            ("duplicate set", sets_with("\"id\": \"ossuary_wardplate\"", "\"id\": \"candlelit_vows\"")),
            ("wrong version", sets_with("\"version\": 1", "\"version\": 9")),
            ("empty bonus", sets_with("{ \"count\": 2, \"stats\": { \"stamina\": 10, \"versatility\": 5 } }", "{ \"count\": 2 }")),
            ("unreachable pieces", sets_with("\"ossuary_pauldrons\", \"ossuary_greaves\"", "\"ossuary_greaves\", \"ossuary_greaves\"")),
        ] {
            assert!(SetCatalog::parse(&bad, &loot).is_err(), "{name} was accepted");
        }
    }

    #[test]
    fn the_item_side_of_the_link_is_checked_too() {
        let item_says_other = LOOT_JSON.replacen(
            "\"set\": \"candlelit_vows\"",
            "\"set\": \"ossuary_wardplate\"",
            1,
        );
        let item_without_set = LOOT_JSON.replacen("\"set\": \"candlelit_vows\", ", "", 1);
        let item_names_unknown = LOOT_JSON.replacen(
            "\"set\": \"candlelit_vows\"",
            "\"set\": \"no_such_set\"",
            1,
        );
        for (name, text) in [
            ("item names another set", item_says_other),
            ("piece does not name the set", item_without_set),
            ("item names an unknown set", item_names_unknown),
        ] {
            assert_ne!(text, LOOT_JSON, "{name}: the edit did not apply");
            let loot = LootCatalog::parse(&text).expect("still valid loot");
            assert!(SetCatalog::parse(SETS_JSON, &loot).is_err(), "{name}");
        }
    }

    #[test]
    fn a_full_major_set_changes_the_sheet_the_realm_builds() {
        // The online path: the persisted loadout -> `sheet_of` -> stats, with set bonuses.
        crate::loot::ensure_items_registered();
        let set = shipped().sets.into_iter().find(|set| set.id == "emberwake_regalia").expect("set");
        let slots = [
            EquipSlot::Head,
            EquipSlot::Shoulder,
            EquipSlot::Back,
            EquipSlot::Wrist,
            EquipSlot::Hands,
        ];
        let mut loadout = crate::loadout::Loadout::default();
        for (slot, piece) in slots.iter().zip(&set.pieces) {
            loadout.equipment.push(crate::loadout::EquippedItem {
                slot: slot.key().to_owned(),
                item: piece.clone(),
                quality: 100,
                sharpening: 0,
                broken: false,
                gems: Vec::new(),
            });
        }
        let sheet = crate::loadout::sheet_of(Hero::FireMage, &loadout);
        let worn = active_sets(sheet.equipment());
        assert_eq!(worn.get("emberwake_regalia").expect("worn").equipped, 5);
        // Piece stats alone, then the 2-piece (+6 crit, +4 haste) and 4-piece (+6 haste).
        let pieces: f32 = set
            .pieces
            .iter()
            .map(|piece| {
                ItemStack::one(item_by_key(piece).expect("item").id)
                    .stats()
                    .get(engine_game::character::Stat::Haste)
            })
            .sum();
        assert!((sheet.gear_stats().get(engine_game::character::Stat::Haste) - (pieces + 10.0)).abs() < 1e-3);
        assert!(crate::loadout::stats_of(&sheet).iter().any(|(name, _)| name.contains("aste")));
    }
}
