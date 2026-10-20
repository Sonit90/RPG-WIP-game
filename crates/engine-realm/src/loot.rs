//! The loot catalog: dungeon items, drop tables and the numbers that govern a roll.
//!
//! ```text
//! assets/catalog/loot.json ──► LootCatalog (serde, validated)
//!                                 ├─► items ──► engine_game::character::register_catalog_items
//!                                 └─► rules ──► engine_game::loot::LootRules ──► Simulation
//! ```
//!
//! The file is compiled into every program that reads it, like the other catalogs, so the
//! client, the realm module and the persistence API cannot disagree about which items exist
//! or what a boss drops. `engine-game` keeps no serde: this module is where the file's format
//! is read and where it is turned into the plain data the simulation rolls with.
//!
//! # Item ids
//!
//! An item's *key* (`"ashgraves_seal"`) is permanent and is what persistence stores. Its
//! numeric [`engine_game::character::ItemId`] is its position in the file, assigned at
//! registration, so every program derives the same ids from the same bytes. **Append new
//! items; never reorder or remove.**
//!
//! # Registration
//!
//! [`ensure_items_registered`] is cheap after the first call and is made by everything here
//! that resolves a key, so a program that never builds a [`crate::catalog::Catalog`] (the
//! persistence API computing a gear score) still sees the catalog's items.

use std::collections::BTreeMap;
use std::sync::{Once, OnceLock};

use engine_game::character::{
    CatalogItem, ITEMS, ItemFamily, ItemIcon, ItemQuality, QUALITY_MAX, QUALITY_MIN, Stat,
    item_by_key, loot_pool, register_catalog_items,
};
use engine_game::enemy::EnemyKind;
use engine_game::loot::{
    BossTable, CurrencyRules, EpicRules, GoldRules, LootRules, LootTable, MobLootRules,
    QualityBucket, QualityRules, WeightedItem,
};
use engine_game::trinket::{TrinketActive, TrinketEffect};
use serde::Deserialize;

use crate::catalog::{CATALOG_VERSION, CatalogError, Economy};

/// The loot catalog as written.
pub(crate) const LOOT_JSON: &str = include_str!("../../../assets/catalog/loot.json");

/// One dungeon item as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LootItem {
    /// Stable key. What a loadout stores.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The slot family it is worn in: `"head"`, `"ring"` ...
    pub slot: String,
    /// `"uncommon"`, `"rare"` or `"epic"`.
    pub rarity: String,
    /// Its item level.
    pub item_level: u16,
    /// Base stats at quality 100, by stat key.
    pub stats: BTreeMap<String, f32>,
    /// The icon family; the slot's if absent.
    #[serde(default)]
    pub icon: Option<String>,
    /// The button a trinket gives while worn. Quality and enhancement do not scale it.
    #[serde(default)]
    pub active: Option<LootActive>,
}

/// A trinket's active as written: `{ "effect": "group_absorb", "cooldown_s": 120, ... }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LootActive {
    /// `"group_damage_reduction"`, `"mass_dispel"`, `"group_cooldown_reduction"` or
    /// `"group_absorb"`.
    pub effect: String,
    /// Seconds before it can be used again.
    pub cooldown_s: f32,
    /// How far from the user it reaches, in metres.
    pub radius_m: f32,
    /// Seconds an effect it leaves lasts; zero for an instant one.
    #[serde(default)]
    pub duration_s: f32,
    /// Its strength as a fraction; unused by Mass Dispel.
    #[serde(default)]
    pub magnitude: f32,
}

impl LootActive {
    fn parsed(&self) -> Option<TrinketActive> {
        Some(TrinketActive {
            effect: TrinketEffect::from_key(&self.effect)?,
            cooldown: self.cooldown_s,
            radius: self.radius_m,
            duration: self.duration_s,
            magnitude: self.magnitude,
        })
    }
}

/// One row of a drop table as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LootEntry {
    /// The item's key.
    pub item: String,
    /// Relative weight within its table.
    pub weight: f32,
}

/// What normal monsters drop as equipment, as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MobTable {
    /// The chance, per player per kill, of one drop.
    #[serde(default = "default_equipment_chance")]
    pub equipment_chance: f32,
    /// What can drop.
    pub entries: Vec<LootEntry>,
}

fn default_equipment_chance() -> f32 {
    0.002
}

/// What finishing a dungeon pays in equipment, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct EquipmentRewards {
    /// Independent rolls from the boss table, per player present.
    #[serde(default = "default_equipment_per_completion")]
    pub equipment_per_completion: u32,
}

fn default_equipment_per_completion() -> u32 {
    2
}

/// One quality band as written.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct QualityBand {
    /// Lowest quality in the band.
    pub min: u8,
    /// Highest quality in the band.
    pub max: u8,
    /// Relative weight at key level 1.
    pub weight: f32,
}

/// The quality distribution as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QualityDistribution {
    /// The bands: contiguous, sorted, covering `50..=110`.
    pub buckets: Vec<QualityBand>,
    /// Bands whose `min` is at least this are boosted by the key level.
    #[serde(default = "default_high_from")]
    pub high_bucket_from: u8,
    /// Boosted bands' weights are multiplied by `1 + this x (key level - 1)`.
    #[serde(default)]
    pub high_bucket_weight_per_key_level: f32,
}

fn default_high_from() -> u8 {
    95
}

/// The epic chance as written.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct EpicChance {
    /// At key level 1.
    pub base_chance: f32,
    /// Added per key level above the first.
    #[serde(default)]
    pub per_key_level: f32,
    /// The ceiling.
    pub max_chance: f32,
}

/// The modifier-reroll currency as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CurrencyDrops {
    /// The resource's id in the economy catalog.
    pub resource: String,
    /// The chance, per player per ordinary kill.
    pub per_mob_chance: f32,
    /// The chance for an elite.
    #[serde(default)]
    pub elite_chance: f32,
    /// How many one drop is worth.
    #[serde(default = "default_currency_amount")]
    pub amount: u32,
}

fn default_currency_amount() -> u32 {
    1
}

/// Gold as written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GoldDrops {
    /// The resource's id in the economy catalog.
    pub resource: String,
    /// The least a kill pays.
    pub per_mob_min: u32,
    /// The most a kill pays.
    pub per_mob_max: u32,
    /// What the boss pays.
    pub boss_amount: u32,
}

/// The whole loot catalog.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct LootCatalog {
    version: u32,
    /// Every dungeon item, in file order (which fixes their numeric ids).
    pub items: Vec<LootItem>,
    /// Boss tables, by the boss's stable id (its enemy archetype id).
    pub boss_tables: BTreeMap<String, Vec<LootEntry>>,
    /// What ordinary monsters drop.
    pub mob_table: MobTable,
    /// What a completion pays in equipment.
    pub rewards: EquipmentRewards,
    /// How quality is drawn.
    pub quality: QualityDistribution,
    /// How often a roll is epic.
    pub epic: EpicChance,
    /// The modifier-reroll currency.
    pub currency: CurrencyDrops,
    /// Gold.
    pub gold: GoldDrops,
}

fn err(message: impl Into<String>) -> CatalogError {
    CatalogError(format!("loot.json: {}", message.into()))
}

impl LootCatalog {
    /// Reads and validates the document's own consistency: formats, ranges and references
    /// between its parts. Resource ids are checked against an economy by
    /// [`LootCatalog::validate_resources`].
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version, a duplicate or built-in-clashing item key, an
    /// unknown slot, rarity or stat, a table naming an unknown item or a boss that does not
    /// exist, an empty table, a quality distribution that does not cover `50..=110`, or a
    /// chance outside `0..=1`.
    pub fn parse(text: &str) -> Result<Self, CatalogError> {
        let catalog: Self = serde_json::from_str(text).map_err(|error| err(error.to_string()))?;
        if catalog.version != CATALOG_VERSION {
            return Err(err(format!(
                "this build reads catalog version {CATALOG_VERSION}"
            )));
        }
        catalog.validate()?;
        Ok(catalog)
    }

    fn validate(&self) -> Result<(), CatalogError> {
        for (index, row) in self.items.iter().enumerate() {
            if row.id.trim().is_empty() {
                return Err(err("an item has no id"));
            }
            if self.items[..index].iter().any(|other| other.id == row.id) {
                return Err(err(format!("item {:?} is listed twice", row.id)));
            }
            if ITEMS.iter().chain(loot_pool()).any(|def| def.key == row.id) {
                return Err(err(format!(
                    "item {:?} clashes with a built-in item",
                    row.id
                )));
            }
            if ItemFamily::from_key(&row.slot).is_none() {
                return Err(err(format!(
                    "item {:?} has unknown slot {:?}",
                    row.id, row.slot
                )));
            }
            if let Some(icon) = &row.icon
                && ItemIcon::from_key(icon).is_none()
            {
                return Err(err(format!("item {:?} has unknown icon {icon:?}", row.id)));
            }
            if ItemQuality::from_key(&row.rarity).is_none() {
                return Err(err(format!(
                    "item {:?} has unknown rarity {:?}",
                    row.id, row.rarity
                )));
            }
            if row.item_level == 0 {
                return Err(err(format!("item {:?} has item level 0", row.id)));
            }
            if row.stats.is_empty() {
                return Err(err(format!("item {:?} grants nothing", row.id)));
            }
            if let Some(active) = &row.active {
                if row.slot != "trinket" {
                    return Err(err(format!(
                        "item {:?} has an active but is not a trinket",
                        row.id
                    )));
                }
                if !active.parsed().is_some_and(|parsed| parsed.is_valid()) {
                    return Err(err(format!("item {:?} has a bad active", row.id)));
                }
            }
            for (name, value) in &row.stats {
                if Stat::from_key(name).is_none() {
                    return Err(err(format!("item {:?} has unknown stat {name:?}", row.id)));
                }
                if !(value.is_finite() && *value > 0.0) {
                    return Err(err(format!("item {:?} has a bad {name} value", row.id)));
                }
            }
        }

        if self.boss_tables.is_empty() {
            return Err(err("there is no boss table"));
        }
        for (boss, entries) in &self.boss_tables {
            let is_boss = EnemyKind::from_id(boss).is_some_and(EnemyKind::is_boss);
            if !is_boss {
                return Err(err(format!("boss table {boss:?} is not a boss")));
            }
            self.validate_table(&format!("boss table {boss:?}"), entries)?;
        }
        self.validate_table("mob_table", &self.mob_table.entries)?;
        check_chance(
            "mob_table.equipment_chance",
            self.mob_table.equipment_chance,
        )?;

        let mut covered = QUALITY_MIN;
        let mut total = 0.0;
        for band in &self.quality.buckets {
            if band.min != covered || band.max < band.min || band.max > QUALITY_MAX {
                return Err(err(format!(
                    "quality bands must run contiguously from {QUALITY_MIN} to {QUALITY_MAX}; \
                     {}..={} breaks that",
                    band.min, band.max
                )));
            }
            if !(band.weight.is_finite() && band.weight >= 0.0) {
                return Err(err("a quality band has a bad weight"));
            }
            total += band.weight;
            covered = band.max.saturating_add(1);
        }
        if covered != QUALITY_MAX + 1 || total <= 0.0 {
            return Err(err(format!(
                "quality bands must cover {QUALITY_MIN}..={QUALITY_MAX} with some weight"
            )));
        }
        if !(self.quality.high_bucket_weight_per_key_level.is_finite()
            && self.quality.high_bucket_weight_per_key_level >= 0.0)
        {
            return Err(err("high_bucket_weight_per_key_level must not be negative"));
        }

        check_chance("epic.base_chance", self.epic.base_chance)?;
        check_chance("epic.max_chance", self.epic.max_chance)?;
        if self.epic.base_chance > self.epic.max_chance
            || !(self.epic.per_key_level.is_finite() && self.epic.per_key_level >= 0.0)
        {
            return Err(err(
                "epic chance must start at or below its maximum and not shrink",
            ));
        }

        check_chance("currency.per_mob_chance", self.currency.per_mob_chance)?;
        check_chance("currency.elite_chance", self.currency.elite_chance)?;
        if self.gold.per_mob_min > self.gold.per_mob_max {
            return Err(err("gold.per_mob_min is above per_mob_max"));
        }
        Ok(())
    }

    fn validate_table(&self, what: &str, entries: &[LootEntry]) -> Result<(), CatalogError> {
        if entries.is_empty() {
            return Err(err(format!("{what} is empty")));
        }
        for entry in entries {
            if !self.items.iter().any(|row| row.id == entry.item) {
                return Err(err(format!("{what} names unknown item {:?}", entry.item)));
            }
            if !(entry.weight.is_finite() && entry.weight > 0.0) {
                return Err(err(format!("{what} gives {:?} no weight", entry.item)));
            }
        }
        Ok(())
    }

    /// Checks that the resources the drops name exist in `economy`.
    ///
    /// # Errors
    ///
    /// A drop naming a resource the economy does not define.
    pub fn validate_resources(&self, economy: &Economy) -> Result<(), CatalogError> {
        for id in [&self.currency.resource, &self.gold.resource] {
            if economy.resource(id).is_none() {
                return Err(err(format!("drops unknown resource {id:?}")));
            }
        }
        Ok(())
    }

    /// The items as `engine-game` registers them, in file order.
    #[must_use]
    pub fn items_for_registration(&self) -> Vec<CatalogItem> {
        self.items
            .iter()
            .filter_map(|row| {
                let family = ItemFamily::from_key(&row.slot)?;
                Some(CatalogItem {
                    key: row.id.clone(),
                    name: row.name.clone(),
                    family,
                    quality: ItemQuality::from_key(&row.rarity)?,
                    item_level: row.item_level,
                    icon: row
                        .icon
                        .as_deref()
                        .and_then(ItemIcon::from_key)
                        .unwrap_or_else(|| ItemIcon::of_family(family)),
                    stats: row
                        .stats
                        .iter()
                        .filter_map(|(name, value)| Some((Stat::from_key(name)?, *value)))
                        .collect(),
                    active: row.active.as_ref().and_then(LootActive::parsed),
                })
            })
            .collect()
    }

    /// Registers the items with `engine-game` (idempotent; the first registration in the
    /// process wins).
    pub fn register_items(&self) {
        register_catalog_items(self.items_for_registration());
    }

    /// The plain-data rules the simulation rolls with. **Registers the items first**, because
    /// the tables name numeric ids.
    ///
    /// The currency and gold resource ids are leaked to `'static` (events carry them by
    /// reference); use [`builtin_rules`], which does this once, rather than calling this in a
    /// loop.
    ///
    /// # Errors
    ///
    /// A table entry whose item did not register (another catalog got there first).
    pub fn rules(&self) -> Result<LootRules, CatalogError> {
        self.register_items();
        let table = |entries: &[LootEntry]| -> Result<LootTable, CatalogError> {
            entries
                .iter()
                .map(|entry| {
                    item_by_key(&entry.item)
                        .map(|def| WeightedItem {
                            item: def.id,
                            weight: entry.weight,
                        })
                        .ok_or_else(|| err(format!("item {:?} is not registered", entry.item)))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|entries| LootTable { entries })
        };
        let leak = |text: &str| -> &'static str { Box::leak(text.to_owned().into_boxed_str()) };
        Ok(LootRules {
            equipment_per_completion: self.rewards.equipment_per_completion,
            quality: QualityRules {
                buckets: self
                    .quality
                    .buckets
                    .iter()
                    .map(|band| QualityBucket {
                        min: band.min,
                        max: band.max,
                        weight: band.weight,
                    })
                    .collect(),
                high_from: self.quality.high_bucket_from,
                high_bucket_weight_per_key_level: self.quality.high_bucket_weight_per_key_level,
            },
            epic: EpicRules {
                base_chance: self.epic.base_chance,
                per_key_level: self.epic.per_key_level,
                max_chance: self.epic.max_chance,
            },
            boss_tables: self
                .boss_tables
                .iter()
                .map(|(boss, entries)| {
                    Ok(BossTable {
                        boss: boss.clone(),
                        table: table(entries)?,
                    })
                })
                .collect::<Result<_, CatalogError>>()?,
            mob: MobLootRules {
                equipment_chance: self.mob_table.equipment_chance,
                table: table(&self.mob_table.entries)?,
            },
            currency: CurrencyRules {
                resource: leak(&self.currency.resource),
                per_mob_chance: self.currency.per_mob_chance,
                elite_chance: self.currency.elite_chance,
                amount: self.currency.amount,
            },
            gold: GoldRules {
                resource: leak(&self.gold.resource),
                per_mob_min: self.gold.per_mob_min,
                per_mob_max: self.gold.per_mob_max,
                boss_amount: self.gold.boss_amount,
            },
        })
    }
}

fn check_chance(what: &str, value: f32) -> Result<(), CatalogError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(err(format!("{what} must be between 0 and 1")))
    }
}

/// Registers the shipped catalog's items with `engine-game`, once per process. Cheap after
/// the first call.
///
/// Everything in this crate that resolves an item key calls it, so a program that only ever
/// loads a [`Loadout`](crate::loadout::Loadout) sees the same items as one that built a
/// [`Catalog`](crate::catalog::Catalog).
pub fn ensure_items_registered() {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| match LootCatalog::parse(LOOT_JSON) {
        Ok(catalog) => catalog.register_items(),
        Err(error) => log::error!("{error}; no catalog items are registered"),
    });
}

/// The shipped loot rules, built once. `None` only if the shipped file is broken, which
/// `cargo test` catches before anything ships.
#[must_use]
pub fn builtin_rules() -> Option<&'static LootRules> {
    static RULES: OnceLock<Option<LootRules>> = OnceLock::new();
    RULES
        .get_or_init(|| {
            let rules = LootCatalog::parse(LOOT_JSON).and_then(|catalog| catalog.rules());
            if let Err(error) = &rules {
                log::error!("{error}; the simulation will use the legacy chest");
            }
            rules.ok()
        })
        .as_ref()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    reason = "test code asserts on known-good values"
)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use engine_game::character::{ItemId, ItemProps, item};
    use engine_game::enemy::MonsterRole;
    use engine_game::firemage::Rng;
    use engine_game::loot::{
        epic_chance_at, quality_odds, roll_equipment, roll_mob_currency, roll_quality,
    };
    use engine_game::trinket::TrinketEffect;

    const CRYPT_SCENE: &str = include_str!("../../../assets/scenes/graveyard_crypt.json");

    fn rules() -> &'static LootRules {
        builtin_rules().expect("the shipped loot rules build")
    }

    fn stat_total(id: ItemId) -> f32 {
        item(id)
            .expect("registered")
            .stats
            .entries()
            .map(|(_, value)| value)
            .sum()
    }

    #[test]
    fn the_shipped_loot_catalog_reads_and_its_items_register_in_file_order() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let loot = catalog.loot();
        assert!(loot.items.len() >= 20, "only {} items", loot.items.len());
        let epics = loot.items.iter().filter(|row| row.rarity == "epic").count();
        assert!((4..=24).contains(&epics), "{epics} epics");
        for (index, row) in loot.items.iter().enumerate() {
            let def = item_by_key(&row.id).expect("registered");
            assert_eq!(
                usize::from(def.id.0 - engine_game::character::CATALOG_ID_BASE),
                index,
                "{} is out of file order",
                row.id
            );
            assert!(!def.grants_nothing());
            assert_eq!(def.item_level, row.item_level);
        }
        // Nearly every family has something to drop, so no slot is starved.
        let families: std::collections::HashSet<_> =
            loot.items.iter().map(|row| row.slot.as_str()).collect();
        assert!(families.len() >= 10, "{families:?}");
    }

    #[test]
    fn the_abbess_drops_the_epics_for_the_slots_the_crypt_leaves_bare() {
        let rules = rules();
        let table = rules
            .boss_table("drowned_abbess")
            .expect("the Abbess has a table");
        let slots: std::collections::HashSet<_> = table
            .entries
            .iter()
            .filter_map(|entry| item(entry.item))
            .filter(|def| def.quality == ItemQuality::Epic)
            .map(|def| def.family)
            .collect();
        // Head, chest, necklace, ring and trinkets were already covered; these are new.
        assert!(slots.len() >= 6, "{slots:?}");
        for key in [
            "abbess_tidewrap_mantle",
            "drowned_pilgrims_greaves",
            "gloves_of_the_salt_psalm",
            "tideworn_sabatons",
            "abbess_signet_of_the_deep",
        ] {
            let def = item_by_key(key).expect("registered");
            assert_eq!(def.quality, ItemQuality::Epic);
            assert_eq!(def.item_level, 12);
            assert!(stat_total(def.id) >= 19.0, "{key}");
            assert!(
                table.entries.iter().any(|entry| entry.item == def.id),
                "{key}"
            );
        }
    }

    #[test]
    fn the_overseer_drops_the_epics_for_the_slots_nobody_else_covers() {
        let rules = rules();
        let table = rules
            .boss_table("foundry_overseer")
            .expect("the Overseer has a table");
        // Back and wrist had no epic before this dungeon; two armoured pieces serve tanks.
        let slots: std::collections::HashSet<_> = table
            .entries
            .iter()
            .filter_map(|entry| item(entry.item))
            .filter(|def| def.quality == ItemQuality::Epic)
            .map(|def| def.family)
            .collect();
        assert!(slots.len() >= 5, "{slots:?}");
        for key in [
            "foundry_cinder_cloak",
            "slagproof_bracers",
            "overseers_ironclad_helm",
            "furnace_wardens_cuirass",
            "emberlink_chain",
        ] {
            let def = item_by_key(key).expect("registered");
            assert_eq!(def.quality, ItemQuality::Epic);
            assert_eq!(def.item_level, 13);
            assert!(stat_total(def.id) >= 19.0, "{key}");
            assert!(table.entries.iter().any(|entry| entry.item == def.id), "{key}");
        }
        // Not stronger than the best of the crypt's epics by more than an item level's worth.
        let best_crypt = stat_total(item_by_key("stewards_funerary_raiment").expect("registered").id);
        for key in ["furnace_wardens_cuirass", "overseers_ironclad_helm"] {
            let def = item_by_key(key).expect("registered");
            assert!(stat_total(def.id) <= best_crypt * 1.2, "{key}");
        }
    }

    #[test]
    fn epics_are_stronger_than_rares() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let mean = |rarity: &str| {
            let rows: Vec<_> = catalog
                .loot()
                .items
                .iter()
                .filter(|row| row.rarity == rarity)
                .collect();
            rows.iter()
                .map(|row| stat_total(item_by_key(&row.id).expect("registered").id))
                .sum::<f32>()
                / rows.len() as f32
        };
        let (rare, epic) = (mean("rare"), mean("epic"));
        assert!(epic > rare * 1.3, "epic {epic} vs rare {rare}");
    }

    #[test]
    fn the_boss_has_a_table_and_the_tables_hold_both_rarities() {
        let rules = rules();
        let table = rules
            .boss_table("lord_ashgrave")
            .expect("the Hollow Steward has a table");
        let of = |entries: &[engine_game::loot::WeightedItem], quality| {
            entries
                .iter()
                .filter(|entry| item(entry.item).is_some_and(|def| def.quality == quality))
                .count()
        };
        assert!(of(&table.entries, ItemQuality::Epic) >= 4);
        assert!(of(&table.entries, ItemQuality::Rare) >= 10);
        assert!(of(&rules.mob.table.entries, ItemQuality::Epic) >= 1);
        assert_eq!(rules.equipment_per_completion, 2);
        assert!((rules.mob.equipment_chance - 0.002).abs() < 1e-9);
    }

    #[test]
    fn the_quality_distribution_matches_the_documented_shape() {
        let rules = rules();
        let odds = quality_odds(rules, 1);
        assert!(odds.mean < 80.0, "mean {}", odds.mean);
        assert!((odds.jackpot - 0.0005).abs() < 0.0001, "{}", odds.jackpot);
        assert!(quality_odds(rules, 20).at_least_100 > odds.at_least_100 * 1.5);
        let mut rng = Rng::new(2024);
        let n = 10_000_u32;
        let rolls: Vec<u8> = (0..n).map(|_| roll_quality(rules, 1, &mut rng)).collect();
        assert!(rolls.iter().all(|q| (50..=110).contains(q)));
        let mean = rolls.iter().map(|q| f32::from(*q)).sum::<f32>() / n as f32;
        assert!((mean - odds.mean).abs() < 1.5, "{mean} vs {}", odds.mean);
        assert!(
            rolls.iter().filter(|q| **q == 110).count() < 30,
            "110 is not rare"
        );
    }

    #[test]
    fn the_epic_rate_matches_the_catalog_and_rises_with_the_key() {
        let rules = rules();
        assert!((epic_chance_at(rules, 1) - 0.025).abs() < 1e-6);
        assert!((epic_chance_at(rules, 11) - 0.045).abs() < 1e-6);
        assert!((epic_chance_at(rules, 20) - 0.063).abs() < 1e-6);
        assert!((epic_chance_at(rules, 100) - 0.08).abs() < 1e-6);
        let table = rules.boss_table("lord_ashgrave").expect("the table");
        let rate = |level: u32| {
            let mut rng = Rng::new(77);
            let n = 20_000_u32;
            (0..n)
                .filter(|_| {
                    let (id, _) = roll_equipment(table, rules, level, &mut rng).expect("items");
                    item(id).expect("registered").quality == ItemQuality::Epic
                })
                .count() as f32
                / n as f32
        };
        let (low, high) = (rate(1), rate(20));
        assert!((low - 0.025).abs() < 0.006, "{low}");
        assert!((high - 0.063).abs() < 0.008, "{high}");
        assert!(high > low);
    }

    #[test]
    fn the_same_seed_rolls_the_same_loot_from_the_shipped_tables() {
        let rules = rules();
        let table = rules.boss_table("lord_ashgrave").expect("the table");
        let run = |seed| {
            let mut rng = Rng::new(seed);
            (0..50)
                .map(|_| roll_equipment(table, rules, 5, &mut rng))
                .collect::<Vec<Option<(ItemId, ItemProps)>>>()
        };
        assert_eq!(run(9), run(9));
        assert_ne!(run(9), run(10));
    }

    /// The monsters a full clear of the crypt kills, as `(ordinary, elite)` counts: the
    /// scene's placements other than the boss, plus the boss's adds.
    fn crypt_kills() -> (u32, u32) {
        let scene: serde_json::Value = serde_json::from_str(CRYPT_SCENE).expect("the scene reads");
        let adds = u32::try_from(scene["dungeon"]["add_count"].as_u64().expect("adds"))
            .expect("a small number");
        let (mut ordinary, mut elite) = (adds, 0);
        for monster in scene["monsters"].as_array().expect("monsters") {
            let id = monster["monster"].as_str().expect("an id");
            let kind = EnemyKind::from_id(id).expect("a known monster");
            if kind.is_boss() {
                continue;
            }
            if kind.archetype().role == MonsterRole::Elite {
                elite += 1;
            } else {
                ordinary += 1;
            }
        }
        (ordinary, elite)
    }

    #[test]
    fn a_crypt_run_drops_about_one_and_a_quarter_aether_shards_per_player() {
        let rules = rules();
        let (ordinary, elite) = crypt_kills();
        assert!(
            ordinary >= 36,
            "the crypt places 36 ordinary monsters, found {ordinary}"
        );
        // Expected value: the closed form, from the catalog's own numbers.
        let expected = f64::from(ordinary) * f64::from(rules.currency.per_mob_chance)
            + f64::from(elite) * f64::from(rules.currency.elite_chance);
        assert!(
            (1.0..=1.5).contains(&expected),
            "expected {expected} per run"
        );
        // And the sampled mean agrees with it.
        let mut rng = Rng::new(31);
        let runs = 4_000_u32;
        let mut shards = 0_u64;
        for _ in 0..runs {
            for _ in 0..ordinary {
                shards += u64::from(
                    roll_mob_currency(rules, false, 0, &mut rng).map_or(0, |drop| drop.amount),
                );
            }
            for _ in 0..elite {
                shards += u64::from(
                    roll_mob_currency(rules, true, 0, &mut rng).map_or(0, |drop| drop.amount),
                );
            }
        }
        let mean = shards as f64 / f64::from(runs);
        assert!(
            (mean - expected).abs() < 0.08,
            "sampled {mean}, expected {expected}"
        );
    }

    #[test]
    fn a_broken_loot_document_is_refused() {
        let edit = |from: &str, to: &str| {
            assert!(
                LOOT_JSON.contains(from),
                "{from} is not in the shipped file"
            );
            LOOT_JSON.replacen(from, to, 1)
        };
        for bad in [
            // An unknown item in a table.
            edit("\"item\": \"ashgraves_seal\"", "\"item\": \"moon_cheese\""),
            // An unknown slot, rarity and stat.
            edit("\"slot\": \"head\"", "\"slot\": \"tail\""),
            edit("\"rarity\": \"epic\"", "\"rarity\": \"mythic\""),
            edit("\"stamina\": 6", "\"stomach\": 6"),
            // A hole in the quality bands.
            edit("\"min\": 60", "\"min\": 61"),
            // A table for something that is not a boss.
            edit("\"lord_ashgrave\"", "\"skeleton_warrior\""),
            // A chance above one.
            edit("\"base_chance\": 0.025", "\"base_chance\": 1.5"),
            // The wrong version.
            edit("\"version\": 1", "\"version\": 2"),
        ] {
            assert!(LootCatalog::parse(&bad).is_err());
        }
        // An item that is already built in.
        let clash = edit(
            "\"id\": \"gravewardens_cowl\"",
            "\"id\": \"simple_cloth_hood\"",
        );
        assert!(LootCatalog::parse(&clash).is_err());
        // A duplicate.
        let twice = edit(
            "\"id\": \"gate_wardens_helm\"",
            "\"id\": \"gravewardens_cowl\"",
        );
        assert!(LootCatalog::parse(&twice).is_err());
    }

    #[test]
    fn the_four_active_trinkets_carry_the_specified_numbers_and_sit_in_the_boss_table() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let rules = rules();
        let table = rules.boss_table("lord_ashgrave").expect("the table");
        let expected = [
            (
                "wardens_rallying_bell",
                TrinketEffect::GroupDamageReduction,
                120.0,
                30.0,
                8.0,
                0.15,
            ),
            (
                "censer_of_cleansing_ash",
                TrinketEffect::MassDispel,
                90.0,
                12.0,
                0.0,
                0.0,
            ),
            (
                "hourglass_of_the_patient_dead",
                TrinketEffect::GroupCooldownReduction,
                120.0,
                30.0,
                0.0,
                0.10,
            ),
            (
                "aegis_of_the_last_vigil",
                TrinketEffect::GroupAbsorb,
                120.0,
                30.0,
                10.0,
                0.20,
            ),
        ];
        assert_eq!(
            catalog
                .loot()
                .items
                .iter()
                .filter(|row| row.active.is_some())
                .count(),
            4
        );
        for (key, effect, cooldown, radius, duration, magnitude) in expected {
            let def = item_by_key(key).expect("registered");
            let active = def.active.expect("has an active");
            assert_eq!(
                (active.effect, active.cooldown, active.radius),
                (effect, cooldown, radius)
            );
            assert_eq!((active.duration, active.magnitude), (duration, magnitude));
            assert!(
                table.entries.iter().any(|entry| entry.item == def.id),
                "{key}"
            );
            // Quality scales the passive stats only.
            let weak = engine_game::character::ItemProps::rolled(60);
            assert!(def.stats_for(weak).get(Stat::Stamina) <= def.stats.get(Stat::Stamina));
        }
        let bad = LOOT_JSON.replacen("\"cooldown_s\": 120", "\"cooldown_s\": 0", 1);
        assert!(LootCatalog::parse(&bad).is_err());
        let off_slot = LOOT_JSON.replacen("\"effect\": \"mass_dispel\"", "\"effect\": \"moon\"", 1);
        assert!(LootCatalog::parse(&off_slot).is_err());
    }

    #[test]
    fn the_drops_must_name_real_resources() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let bad = LOOT_JSON.replacen("\"aether_shard\"", "\"moon_cheese\"", 1);
        let loot = LootCatalog::parse(&bad).expect("it is internally consistent");
        assert!(loot.validate_resources(catalog.economy()).is_err());
    }
}
