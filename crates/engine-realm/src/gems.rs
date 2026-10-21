//! Gems: the catalog (`assets/catalog/gems.json`) and the three hub rules that use it.
//!
//! ```text
//! gems.json ──► GemCatalog::parse (serde, validated)
//!                  ├─► items_for_registration ──► gem_<kind>_<level> stackable items
//!                  └─► drop_rules             ──► engine_game::loot::GemDrops
//!
//! socket(loadout, item, socket, gem_slot)      one gem from the bag fills an empty socket
//! unsocket(loadout, item, socket)              pay gold, the gem goes back to the bag
//! upgrade(loadout, gem_slot, draw)             consume N same gems + gold, roll, keep one on failure
//! ```
//!
//! A gem is an inventory item that stacks; a socketed gem is stored on the piece as its key
//! (`EquippedItem::gems`), so a rebuilt catalog never renumbers anyone's gems. What a socketed
//! gem grants is the gem item's own stat, added to the wearer **unscaled** by
//! `CharacterSheet::gear_stats` (like a set bonus: quality and sharpening do not touch it).
//!
//! The rules are pure: the persistence API runs each inside one locked read-modify-write of the
//! character row, under the hub's request-id idempotency, and supplies the upgrade's roll from
//! the server's own random source. A refused call changes nothing and charges nothing, and no
//! refusal depends on the roll (a full bag refuses an upgrade whether or not it would succeed),
//! so a refusal never leaks the outcome of a try.

use std::fmt;

use engine_game::character::{
    CatalogItem, GEM_SOCKETS_MAX, ItemFamily, ItemIcon, ItemProps, ItemQuality, Stat, item_by_key,
};
use engine_game::loot::{DropSource, GemDrops, GemSourceRule};
use serde::Deserialize;

use crate::catalog::{CATALOG_VERSION, CatalogError, Economy};
use crate::loadout::{Loadout, SlotRef};
use crate::loot::ensure_items_registered;

/// The gem catalog as written.
pub(crate) const GEMS_JSON: &str = include_str!("../../../assets/catalog/gems.json");

/// The resource every gem service is paid in.
pub const GOLD: &str = "gold";

/// The most stacked gems one upgrade may consume. A sanity bound on the data.
const MAX_CONSUME: u16 = 10;

/// One gem kind: a stat and what each level of it is worth.
#[derive(Debug, Clone, PartialEq)]
pub struct GemKind {
    /// Stable id, part of the item key (`gem_<id>_<level>`).
    pub id: String,
    /// Display name of the stone (`"Ruby"`).
    pub name: String,
    /// The stat it grants.
    pub stat: Stat,
    /// The stat value per level, index 0 is level 1. Strictly increasing.
    pub values: Vec<f32>,
}

/// What one upgrade step costs and risks: from level `n` to `n + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct UpgradeStep {
    /// Gold paid for every attempt, success or not.
    pub gold: u64,
    /// The chance the attempt succeeds, in `(0, 1]`.
    pub success_chance: f64,
}

/// What a failed upgrade gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    /// One of the consumed gems comes back; the rest are lost.
    KeepOne,
}

/// The upgrade rule.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpgradeRule {
    /// How many gems of one kind and level an attempt consumes.
    pub consume: u16,
    /// What a failure returns.
    pub on_fail: OnFail,
    /// Per level being upgraded (`steps[0]` is level 1 to 2): `max_level - 1` rows.
    pub steps: Vec<UpgradeStep>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct KindRecord {
    id: String,
    name: String,
    stat: String,
    values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct SourceRecord {
    source: String,
    chance: f32,
    level_weights: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct DropsRecord {
    #[serde(default)]
    key_bonus_per_level: f32,
    #[serde(default = "one")]
    max_multiplier: f32,
    sources: Vec<SourceRecord>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct UnsocketRecord {
    gold: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct GemFile {
    version: u32,
    max_level: u8,
    level_names: Vec<String>,
    kinds: Vec<KindRecord>,
    drops: DropsRecord,
    upgrade: UpgradeRule,
    unsocket: UnsocketRecord,
}

/// The whole gem catalog, parsed and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct GemCatalog {
    /// The highest gem level.
    pub max_level: u8,
    /// The adjective of each level (`"Chipped"`).
    pub level_names: Vec<String>,
    /// Every kind, in file order.
    pub kinds: Vec<GemKind>,
    /// The upgrade rule.
    pub upgrade: UpgradeRule,
    /// Gold to take a gem out, per gem level.
    pub unsocket_gold: Vec<u64>,
    drops: DropsRecord,
}

fn err(message: impl Into<String>) -> CatalogError {
    CatalogError(format!("gems.json: {}", message.into()))
}

/// The item key of the gem of `kind` at `level`.
#[must_use]
pub fn gem_key(kind: &str, level: u8) -> String {
    format!("gem_{kind}_{level}")
}

impl GemCatalog {
    /// Reads and validates `text`.
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version, a bad level count, a kind with an unknown stat or
    /// values that do not strictly increase, a duplicate kind, an upgrade table that is not
    /// `max_level - 1` rows or has a chance outside `(0, 1]`, an unsocket table that is not
    /// `max_level` rows, or a drop source that is unknown, repeated or has a chance outside
    /// `(0, 1]`.
    pub fn parse(text: &str) -> Result<Self, CatalogError> {
        let file: GemFile = serde_json::from_str(text).map_err(|error| err(error.to_string()))?;
        if file.version != CATALOG_VERSION {
            return Err(err(format!(
                "this build reads catalog version {CATALOG_VERSION}"
            )));
        }
        let levels = usize::from(file.max_level);
        if !(2..=9).contains(&file.max_level) {
            return Err(err("max_level must be between 2 and 9"));
        }
        if file.level_names.len() != levels {
            return Err(err("level_names must name every level"));
        }
        if file.kinds.is_empty() {
            return Err(err("there are no kinds"));
        }
        let mut kinds: Vec<GemKind> = Vec::with_capacity(file.kinds.len());
        for record in &file.kinds {
            let id = record.id.trim();
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(err(format!(
                    "kind id {:?} must be lower snake_case",
                    record.id
                )));
            }
            if kinds.iter().any(|kind| kind.id == id) {
                return Err(err(format!("kind {id:?} is listed twice")));
            }
            let stat = Stat::from_key(&record.stat)
                .ok_or_else(|| err(format!("kind {id:?} has unknown stat {:?}", record.stat)))?;
            if record.values.len() != levels {
                return Err(err(format!("kind {id:?} needs {levels} values")));
            }
            let increasing = record.values.windows(2).all(|pair| pair[0] < pair[1]);
            if !increasing || record.values.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                return Err(err(format!(
                    "kind {id:?} values must be positive and strictly increase with level"
                )));
            }
            kinds.push(GemKind {
                id: id.to_owned(),
                name: record.name.clone(),
                stat,
                values: record.values.clone(),
            });
        }
        let upgrade = &file.upgrade;
        if !(2..=MAX_CONSUME).contains(&upgrade.consume) {
            return Err(err(format!(
                "upgrade.consume must be between 2 and {MAX_CONSUME}"
            )));
        }
        if upgrade.steps.len() != levels - 1 {
            return Err(err(format!(
                "upgrade.steps needs {} rows (one per level below the maximum)",
                levels - 1
            )));
        }
        for step in &upgrade.steps {
            if !(step.success_chance.is_finite()
                && step.success_chance > 0.0
                && step.success_chance <= 1.0)
            {
                return Err(err("upgrade success_chance must be in (0, 1]"));
            }
            if step.gold == 0 {
                return Err(err("an upgrade must cost gold"));
            }
        }
        if file.unsocket.gold.len() != levels || file.unsocket.gold.contains(&0) {
            return Err(err(format!(
                "unsocket.gold needs {levels} non-zero rows (one per gem level)"
            )));
        }
        let drops = &file.drops;
        if !drops.key_bonus_per_level.is_finite()
            || drops.key_bonus_per_level < 0.0
            || !drops.max_multiplier.is_finite()
            || drops.max_multiplier < 1.0
        {
            return Err(err(
                "drops key_bonus_per_level must be >= 0 and max_multiplier >= 1",
            ));
        }
        let mut seen: Vec<DropSource> = Vec::new();
        for record in &drops.sources {
            let source = DropSource::from_key(&record.source)
                .ok_or_else(|| err(format!("unknown drop source {:?}", record.source)))?;
            if seen.contains(&source) {
                return Err(err(format!(
                    "drop source {:?} is listed twice",
                    record.source
                )));
            }
            seen.push(source);
            if !(record.chance.is_finite() && record.chance > 0.0 && record.chance <= 1.0) {
                return Err(err(format!(
                    "drop source {:?} chance must be in (0, 1]",
                    record.source
                )));
            }
            let weights = &record.level_weights;
            if weights.is_empty()
                || weights.len() > levels
                || weights.iter().any(|w| !w.is_finite() || *w < 0.0)
                || weights.iter().sum::<f32>() <= 0.0
            {
                return Err(err(format!(
                    "drop source {:?} needs 1..={levels} non-negative level weights with a positive sum",
                    record.source
                )));
            }
        }
        Ok(Self {
            max_level: file.max_level,
            level_names: file.level_names,
            kinds,
            upgrade: file.upgrade,
            unsocket_gold: file.unsocket.gold,
            drops: file.drops,
        })
    }

    /// The kind with this id.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<&GemKind> {
        self.kinds.iter().find(|kind| kind.id == id)
    }

    /// Which kind and level an item key is, if it is one of this catalog's gems.
    #[must_use]
    pub fn parse_key(&self, key: &str) -> Option<(&GemKind, u8)> {
        let rest = key.strip_prefix("gem_")?;
        let (kind, level) = rest.rsplit_once('_')?;
        let level: u8 = level.parse().ok()?;
        if !(1..=self.max_level).contains(&level) {
            return None;
        }
        self.kind(kind).map(|kind| (kind, level))
    }

    /// What a gem of `level` of `kind` is worth, in its stat.
    #[must_use]
    pub fn value(&self, kind: &GemKind, level: u8) -> f32 {
        kind.values
            .get(usize::from(level).saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
    }

    /// The display name of the gem (`"Perfect Ruby"`).
    #[must_use]
    pub fn display_name(&self, kind: &GemKind, level: u8) -> String {
        let level_name = self
            .level_names
            .get(usize::from(level).saturating_sub(1))
            .map_or("", String::as_str);
        format!("{level_name} {}", kind.name)
    }

    /// The upgrade step that takes a gem of `level` to the next, if there is a next.
    #[must_use]
    pub fn step(&self, level: u8) -> Option<&UpgradeStep> {
        if level == 0 || level >= self.max_level {
            return None;
        }
        self.upgrade.steps.get(usize::from(level) - 1)
    }

    /// The gold to take a gem of `level` out of a socket.
    #[must_use]
    pub fn unsocket_cost(&self, level: u8) -> u64 {
        self.unsocket_gold
            .get(usize::from(level).saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    /// The gem items as `engine-game` registers them: every kind at every level, kind-major.
    #[must_use]
    pub fn items_for_registration(&self) -> Vec<CatalogItem> {
        let qualities = [
            ItemQuality::Common,
            ItemQuality::Uncommon,
            ItemQuality::Rare,
            ItemQuality::Epic,
            ItemQuality::Legendary,
        ];
        let mut items = Vec::with_capacity(self.kinds.len() * usize::from(self.max_level));
        for kind in &self.kinds {
            for level in 1..=self.max_level {
                items.push(CatalogItem {
                    key: gem_key(&kind.id, level),
                    name: self.display_name(kind, level),
                    family: ItemFamily::Gem,
                    quality: qualities[(usize::from(level) - 1).min(qualities.len() - 1)],
                    item_level: u16::from(level),
                    icon: ItemIcon::Trinket,
                    stats: vec![(kind.stat, self.value(kind, level))],
                    active: None,
                    set: None,
                    sockets: 0,
                    gem_level: level,
                });
            }
        }
        items
    }

    /// The drop rules as the simulation rolls them. **The items must be registered** (the gem
    /// items are looked up by key); a gem that is not gets no entry.
    #[must_use]
    pub fn drop_rules(&self) -> GemDrops {
        let items: Vec<Vec<_>> = self
            .kinds
            .iter()
            .map(|kind| {
                (1..=self.max_level)
                    .filter_map(|level| item_by_key(&gem_key(&kind.id, level)).map(|def| def.id))
                    .collect()
            })
            .collect();
        if items
            .iter()
            .any(|levels| levels.len() != usize::from(self.max_level))
        {
            return GemDrops::none();
        }
        GemDrops {
            sources: self
                .drops
                .sources
                .iter()
                .filter_map(|record| {
                    Some(GemSourceRule {
                        source: DropSource::from_key(&record.source)?,
                        chance: record.chance,
                        level_weights: record.level_weights.clone(),
                    })
                })
                .collect(),
            key_bonus_per_level: self.drops.key_bonus_per_level,
            max_multiplier: self.drops.max_multiplier,
            items,
        }
    }
}

/// The shipped gem catalog, parsed once. `None` only if the shipped file is broken, which
/// `cargo test` catches.
#[must_use]
pub fn builtin() -> Option<&'static GemCatalog> {
    static CATALOG: std::sync::OnceLock<Option<GemCatalog>> = std::sync::OnceLock::new();
    CATALOG
        .get_or_init(|| match GemCatalog::parse(GEMS_JSON) {
            Ok(catalog) => Some(catalog),
            Err(error) => {
                log::error!("{error}; no gems are registered");
                None
            }
        })
        .as_ref()
}

/// A number in `[0, 1)` from 64 random bits (the top 53, exactly representable).
fn unit(bits: u64) -> f64 {
    (bits >> 11) as f64 / (1_u64 << 53) as f64
}

/// Why a gem service was refused. Nothing was paid or changed in any of these cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GemError {
    /// Gems are not available (the catalog failed to load).
    Disabled,
    /// The item address names nothing.
    EmptySlot,
    /// The item has no sockets (it is not equipment, or has none).
    NoSockets,
    /// The socket index is past the item's sockets.
    NoSuchSocket,
    /// The socket already holds a gem.
    SocketOccupied,
    /// The socket holds no gem.
    SocketEmpty,
    /// The bag cell is empty or does not hold a gem.
    NotAGem,
    /// The gem is already at the maximum level.
    MaxLevel,
    /// The stack is smaller than an upgrade consumes: `(have, need)`.
    NotEnoughGems(u16, u16),
    /// The bag has no room for the result.
    BagFull,
    /// The character cannot pay; carries the resource's display name.
    Insufficient(String),
}

impl fmt::Display for GemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("Gems are not available."),
            Self::EmptySlot => f.write_str("There is no item there."),
            Self::NoSockets => f.write_str("That item has no sockets."),
            Self::NoSuchSocket => f.write_str("That item has no such socket."),
            Self::SocketOccupied => f.write_str("That socket already holds a gem."),
            Self::SocketEmpty => f.write_str("That socket is empty."),
            Self::NotAGem => f.write_str("That is not a gem."),
            Self::MaxLevel => f.write_str("That gem is already at the highest level."),
            Self::NotEnoughGems(have, need) => {
                write!(f, "You need {need} of that gem and have {have}.")
            }
            Self::BagFull => f.write_str("Your bag is full."),
            Self::Insufficient(resource) => write!(f, "You do not have enough {resource}."),
        }
    }
}

impl std::error::Error for GemError {}

/// Where an item is stored: its worn entry or its bag entry.
fn item_entry<'a>(
    loadout: &'a mut Loadout,
    slot: &SlotRef,
) -> Result<(&'a str, &'a mut Vec<String>), GemError> {
    match slot {
        SlotRef::Equipment { slot } => loadout
            .equipment
            .iter_mut()
            .find(|entry| entry.slot == *slot)
            .map(|entry| (entry.item.as_str(), &mut entry.gems)),
        SlotRef::Inventory { index } => loadout
            .inventory
            .iter_mut()
            .find(|entry| entry.index == *index)
            .map(|entry| (entry.item.as_str(), &mut entry.gems)),
    }
    .ok_or(GemError::EmptySlot)
}

/// The sockets of the item stored under `key`.
fn sockets_of(key: &str) -> usize {
    item_by_key(key).map_or(0, |def| usize::from(def.sockets).min(GEM_SOCKETS_MAX))
}

/// The gem key in socket `socket` of a stored gems list, if any (the empty string is empty).
fn gem_at(gems: &[String], socket: usize) -> Option<&str> {
    gems.get(socket)
        .map(String::as_str)
        .filter(|key| !key.is_empty())
}

fn gold_name(economy: &Economy) -> String {
    economy
        .resource(GOLD)
        .map_or_else(|| GOLD.to_owned(), |resource| resource.name.clone())
}

fn pay(loadout: &mut Loadout, economy: &Economy, amount: u64) -> Result<u64, GemError> {
    let held = loadout.resources.get(GOLD).copied().unwrap_or(0);
    let left = held
        .checked_sub(amount)
        .ok_or_else(|| GemError::Insufficient(gold_name(economy)))?;
    if left == 0 {
        loadout.resources.remove(GOLD);
    } else {
        loadout.resources.insert(GOLD.to_owned(), left);
    }
    Ok(left)
}

/// Removes `count` gems from the bag stack in `cell`; the cell empties at zero.
fn take_from_cell(loadout: &mut Loadout, cell: u32, count: u16) {
    if let Some(position) = loadout.inventory.iter().position(|e| e.index == cell) {
        let entry = &mut loadout.inventory[position];
        entry.quantity = entry.quantity.saturating_sub(count);
        if entry.quantity == 0 {
            loadout.inventory.remove(position);
        }
    }
}

/// What socketing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketOutcome {
    /// The socketed item's key.
    pub item: String,
    /// The socket index.
    pub socket: u8,
    /// The gem's item key.
    pub gem: String,
}

/// **Socket a gem**: one gem from the bag cell `gem_slot` fills the empty `socket` of the item
/// at `item_slot` (worn or in the bag). Free.
///
/// # Errors
///
/// [`GemError::EmptySlot`], [`GemError::NoSockets`], [`GemError::NoSuchSocket`],
/// [`GemError::SocketOccupied`], [`GemError::NotAGem`]. The loadout is unchanged on every error.
pub fn socket(
    loadout: &mut Loadout,
    item_slot: &SlotRef,
    socket: u8,
    gem_slot: u32,
    catalog: &GemCatalog,
) -> Result<SocketOutcome, GemError> {
    ensure_items_registered();
    let socket_index = usize::from(socket);
    let gem = loadout
        .inventory
        .iter()
        .find(|entry| entry.index == gem_slot)
        .map(|entry| entry.item.clone())
        .filter(|key| catalog.parse_key(key).is_some())
        .ok_or(GemError::NotAGem)?;
    let mut changed = loadout.clone();
    let item = {
        // The item may be the bag cell the gem is taken from only if they differ; a gem is
        // never socketable (no sockets), which `NoSockets` reports.
        let (key, gems) = item_entry(&mut changed, item_slot)?;
        let sockets = sockets_of(key);
        if sockets == 0 {
            return Err(GemError::NoSockets);
        }
        if socket_index >= sockets {
            return Err(GemError::NoSuchSocket);
        }
        if gem_at(gems, socket_index).is_some() {
            return Err(GemError::SocketOccupied);
        }
        if gems.len() <= socket_index {
            gems.resize(socket_index + 1, String::new());
        }
        gems[socket_index].clone_from(&gem);
        key.to_owned()
    };
    take_from_cell(&mut changed, gem_slot, 1);
    *loadout = changed;
    Ok(SocketOutcome { item, socket, gem })
}

/// What unsocketing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsocketOutcome {
    /// The item's key.
    pub item: String,
    /// The socket index.
    pub socket: u8,
    /// The gem that came back to the bag.
    pub gem: String,
    /// The gold paid.
    pub cost: u64,
    /// The gold left.
    pub gold_left: u64,
}

/// What taking the gem out of `socket` of the item at `item_slot` would cost: the gem's key and
/// the gold.
///
/// # Errors
///
/// [`GemError::EmptySlot`], [`GemError::NoSuchSocket`], [`GemError::SocketEmpty`].
pub fn unsocket_quote(
    loadout: &Loadout,
    item_slot: &SlotRef,
    socket: u8,
    catalog: &GemCatalog,
) -> Result<(String, u64), GemError> {
    let mut copy = loadout.clone();
    let (key, gems) = item_entry(&mut copy, item_slot)?;
    if usize::from(socket) >= sockets_of(key) {
        return Err(GemError::NoSuchSocket);
    }
    let gem = gem_at(gems, usize::from(socket)).ok_or(GemError::SocketEmpty)?;
    let level = catalog.parse_key(gem).map_or(1, |(_, level)| level);
    Ok((gem.to_owned(), catalog.unsocket_cost(level)))
}

/// **Take a gem out**: pay gold, the gem returns to the bag (joining a stack when it can).
///
/// # Errors
///
/// [`GemError::EmptySlot`], [`GemError::NoSuchSocket`], [`GemError::SocketEmpty`],
/// [`GemError::BagFull`] (checked before anything is paid), [`GemError::Insufficient`]. The
/// loadout is unchanged on every error.
pub fn unsocket(
    loadout: &mut Loadout,
    item_slot: &SlotRef,
    socket: u8,
    catalog: &GemCatalog,
    economy: &Economy,
) -> Result<UnsocketOutcome, GemError> {
    ensure_items_registered();
    let (gem, cost) = unsocket_quote(loadout, item_slot, socket, catalog)?;
    let mut changed = loadout.clone();
    let item = {
        let (key, gems) = item_entry(&mut changed, item_slot)?;
        gems[usize::from(socket)] = String::new();
        while gems.last().is_some_and(String::is_empty) {
            gems.pop();
        }
        key.to_owned()
    };
    crate::loadout::add_stack_to_bag(&mut changed, &gem, ItemProps::STANDARD)
        .map_err(|_| GemError::BagFull)?;
    let gold_left = pay(&mut changed, economy, cost)?;
    *loadout = changed;
    Ok(UnsocketOutcome {
        item,
        socket,
        gem,
        cost,
        gold_left,
    })
}

/// What an upgrade would cost and risk, for a panel.
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeQuote {
    /// The gem being upgraded.
    pub gem: String,
    /// The gem it becomes on success.
    pub result: String,
    /// The gem's level.
    pub level: u8,
    /// How many are consumed.
    pub consume: u16,
    /// The gold.
    pub gold: u64,
    /// The success chance.
    pub success_chance: f64,
}

/// Looks up the gem stack in bag cell `gem_slot` and what upgrading it would take.
///
/// # Errors
///
/// [`GemError::NotAGem`], [`GemError::MaxLevel`]; the stack size is not checked.
pub fn upgrade_quote(
    loadout: &Loadout,
    gem_slot: u32,
    catalog: &GemCatalog,
) -> Result<UpgradeQuote, GemError> {
    let entry = loadout
        .inventory
        .iter()
        .find(|entry| entry.index == gem_slot)
        .ok_or(GemError::NotAGem)?;
    let (kind, level) = catalog.parse_key(&entry.item).ok_or(GemError::NotAGem)?;
    let step = catalog.step(level).ok_or(GemError::MaxLevel)?;
    Ok(UpgradeQuote {
        gem: entry.item.clone(),
        result: gem_key(&kind.id, level + 1),
        level,
        consume: catalog.upgrade.consume,
        gold: step.gold,
        success_chance: step.success_chance,
    })
}

/// What an upgrade did.
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeOutcome {
    /// The gem that was upgraded.
    pub gem: String,
    /// The gem it would have become.
    pub result: String,
    /// Whether it worked.
    pub success: bool,
    /// How many gems were consumed.
    pub consumed: u16,
    /// How many of them came back (zero on success).
    pub returned: u16,
    /// The gold paid.
    pub cost: u64,
    /// The gold left.
    pub gold_left: u64,
}

/// **Upgrade a gem**: `consume` gems of one kind and level from the stack in `gem_slot` and the
/// gold go; on success one gem of the next level arrives, on failure one consumed gem comes back
/// (`keep_one`). The price is paid either way.
///
/// `draw` is the caller's random source (64 bits per call, called once); the server feeds it
/// from the operating system, tests from a seeded generator. A refused call draws nothing.
///
/// # Errors
///
/// [`GemError::NotAGem`], [`GemError::MaxLevel`], [`GemError::NotEnoughGems`],
/// [`GemError::BagFull`] (when the bag could not take the next-level gem, whatever the roll),
/// [`GemError::Insufficient`]. The loadout is unchanged on every error.
pub fn upgrade(
    loadout: &mut Loadout,
    gem_slot: u32,
    draw: &mut dyn FnMut() -> u64,
    catalog: &GemCatalog,
    economy: &Economy,
) -> Result<UpgradeOutcome, GemError> {
    ensure_items_registered();
    let quote = upgrade_quote(loadout, gem_slot, catalog)?;
    let have = loadout
        .inventory
        .iter()
        .find(|entry| entry.index == gem_slot)
        .map_or(0, |entry| entry.quantity);
    if have < quote.consume {
        return Err(GemError::NotEnoughGems(have, quote.consume));
    }
    let mut changed = loadout.clone();
    take_from_cell(&mut changed, gem_slot, quote.consume);
    // Room for the next level is required whatever the roll, so a refusal never tells the
    // player how a try would have gone.
    let mut with_result = changed.clone();
    crate::loadout::add_stack_to_bag(&mut with_result, &quote.result, ItemProps::STANDARD)
        .map_err(|_| GemError::BagFull)?;
    let gold_left = pay(&mut changed, economy, quote.gold)?;
    let success = unit(draw()) < quote.success_chance;
    let (arrives, returned) = if success {
        (quote.result.as_str(), 0)
    } else {
        match catalog.upgrade.on_fail {
            OnFail::KeepOne => (quote.gem.as_str(), 1),
        }
    };
    // The kept gem returns to the stack it came from or, if that emptied, a free cell; there is
    // room for it because the result had room.
    crate::loadout::add_stack_to_bag(&mut changed, arrives, ItemProps::STANDARD)
        .map_err(|_| GemError::BagFull)?;
    *loadout = changed;
    Ok(UpgradeOutcome {
        gem: quote.gem,
        result: quote.result,
        success,
        consumed: quote.consume,
        returned,
        cost: quote.gold,
        gold_left,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::economy::add_resources;
    use crate::loadout::{BagItem, EquippedItem, sheet_of};
    use engine_game::character::{EquipSlot, INVENTORY_SLOTS, Stat};
    use engine_game::hero::Hero;

    fn gems() -> &'static GemCatalog {
        builtin().expect("the shipped gem catalog parses")
    }

    fn economy() -> Economy {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .economy()
            .clone()
    }

    /// A one-socket epic ring and a two-socket epic helm.
    const RING: &str = "ashgraves_seal";
    const HELM: &str = "crown_of_the_hollow_steward";

    fn worn(gold: u64) -> Loadout {
        ensure_items_registered();
        let mut loadout = Loadout::default();
        loadout.equipment.push(EquippedItem::new("ring1", RING));
        loadout.equipment.push(EquippedItem::new("head", HELM));
        if gold > 0 {
            add_resources(&mut loadout, &[(GOLD.into(), gold)]);
        }
        loadout
    }

    fn put(loadout: &mut Loadout, cell: u32, key: &str, quantity: u16) {
        loadout.inventory.push(BagItem::new(cell, key, quantity));
        loadout.inventory.sort_by_key(|entry| entry.index);
    }

    fn fill_bag(loadout: &mut Loadout) {
        for cell in 0..u32::try_from(INVENTORY_SLOTS).expect("small") {
            put(loadout, cell, "gravewardens_cowl", 1);
        }
    }

    fn ring() -> SlotRef {
        SlotRef::Equipment {
            slot: "ring1".into(),
        }
    }

    fn head() -> SlotRef {
        SlotRef::Equipment {
            slot: "head".into(),
        }
    }

    fn gold(loadout: &Loadout) -> u64 {
        loadout.resources.get(GOLD).copied().unwrap_or(0)
    }

    fn count(loadout: &Loadout, key: &str) -> u32 {
        loadout
            .inventory
            .iter()
            .filter(|entry| entry.item == key)
            .map(|entry| u32::from(entry.quantity))
            .sum()
    }

    fn succeed() -> u64 {
        0
    }

    fn fail() -> u64 {
        u64::MAX
    }

    #[test]
    fn the_shipped_catalog_is_valid_and_its_tables_have_the_right_shape() {
        let catalog = gems();
        assert_eq!(catalog.max_level, 5);
        assert_eq!(catalog.kinds.len(), 7);
        for kind in &catalog.kinds {
            assert_eq!(kind.values.len(), 5);
            assert!(kind.values.windows(2).all(|pair| pair[0] < pair[1]));
        }
        assert_eq!(
            catalog.upgrade.steps.len(),
            usize::from(catalog.max_level) - 1
        );
        assert_eq!(catalog.unsocket_gold.len(), usize::from(catalog.max_level));
        for step in &catalog.upgrade.steps {
            assert!(step.success_chance > 0.0 && step.success_chance <= 1.0);
        }
        // Harder and dearer as the gem gets better.
        let steps = &catalog.upgrade.steps;
        assert!(
            steps
                .windows(2)
                .all(|p| p[0].success_chance >= p[1].success_chance)
        );
        assert!(steps.windows(2).all(|p| p[0].gold < p[1].gold));
        let stats: Vec<_> = catalog.kinds.iter().map(|kind| kind.stat).collect();
        for stat in [
            Stat::MainStat,
            Stat::Stamina,
            Stat::CriticalStrike,
            Stat::Haste,
            Stat::Versatility,
            Stat::Armor,
            Stat::Spirit,
        ] {
            assert!(stats.contains(&stat));
        }
    }

    #[test]
    fn gem_items_register_as_stackable_with_the_gems_stat() {
        ensure_items_registered();
        for kind in &gems().kinds {
            for level in 1..=gems().max_level {
                let def = item_by_key(&gem_key(&kind.id, level)).expect("registered");
                assert_eq!(def.family, ItemFamily::Gem);
                assert_eq!(def.max_stack, engine_game::character::GEM_STACK_MAX);
                assert_eq!(def.gem_level, level);
                assert_eq!(def.stats.get(kind.stat), gems().value(kind, level));
                let parsed = gems().parse_key(def.key).map(|(k, l)| (k.id.clone(), l));
                assert_eq!(parsed, Some((kind.id.clone(), level)));
            }
        }
        assert!(gems().parse_key("gem_nonsense_1").is_none());
        assert!(gems().parse_key("gem_haste_6").is_none());
        assert!(gems().parse_key("ashgraves_seal").is_none());
    }

    #[test]
    fn epics_have_sockets_and_rares_do_not() {
        ensure_items_registered();
        assert_eq!(item_by_key(RING).expect("ring").sockets, 1);
        assert_eq!(item_by_key(HELM).expect("helm").sockets, 2);
        assert_eq!(item_by_key("gravewardens_cowl").expect("rare").sockets, 0);
    }

    #[test]
    fn a_broken_catalog_is_refused() {
        let edit = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value: serde_json::Value = serde_json::from_str(GEMS_JSON).expect("json");
            change(&mut value);
            GemCatalog::parse(&value.to_string())
        };
        assert!(edit(&|_| {}).is_ok());
        // Values that do not increase.
        assert!(edit(&|v| v["kinds"][0]["values"][2] = 0.5.into()).is_err());
        // A chance of zero, or above one.
        assert!(edit(&|v| v["upgrade"]["steps"][1]["success_chance"] = 0.0.into()).is_err());
        assert!(edit(&|v| v["upgrade"]["steps"][1]["success_chance"] = 1.1.into()).is_err());
        // A missing row.
        assert!(
            edit(&|v| {
                v["upgrade"]["steps"].as_array_mut().expect("steps").pop();
            })
            .is_err()
        );
        assert!(
            edit(&|v| {
                v["unsocket"]["gold"].as_array_mut().expect("gold").pop();
            })
            .is_err()
        );
        // An unknown stat, a repeated kind, an unknown or repeated drop source.
        assert!(edit(&|v| v["kinds"][0]["stat"] = "luck".into()).is_err());
        assert!(edit(&|v| v["kinds"][1]["id"] = "main_stat".into()).is_err());
        assert!(edit(&|v| v["drops"]["sources"][0]["source"] = "pond".into()).is_err());
        assert!(edit(&|v| v["drops"]["sources"][1]["source"] = "mob".into()).is_err());
        assert!(edit(&|v| v["drops"]["sources"][0]["chance"] = 0.0.into()).is_err());
        assert!(edit(&|v| v["upgrade"]["consume"] = 1.into()).is_err());
    }

    #[test]
    fn socketing_consumes_one_gem_and_fills_the_socket() {
        let mut loadout = worn(0);
        put(&mut loadout, 0, "gem_haste_2", 3);
        let outcome = socket(&mut loadout, &ring(), 0, 0, gems()).expect("socketed");
        assert_eq!(
            (outcome.item.as_str(), outcome.gem.as_str()),
            (RING, "gem_haste_2")
        );
        assert_eq!(loadout.equipment[0].gems, vec!["gem_haste_2".to_owned()]);
        assert_eq!(count(&loadout, "gem_haste_2"), 2);
        // The last gem of a stack empties the cell.
        let mut single = worn(0);
        put(&mut single, 4, "gem_haste_1", 1);
        socket(&mut single, &ring(), 0, 4, gems()).expect("socketed");
        assert!(single.inventory.is_empty());
    }

    #[test]
    fn socketing_refuses_without_changing_anything() {
        let mut loadout = worn(0);
        put(&mut loadout, 0, "gem_haste_1", 2);
        put(&mut loadout, 1, "gravewardens_cowl", 1);
        let chest = SlotRef::Equipment {
            slot: "chest".into(),
        };
        let cases = [
            (ring(), 1, 0, GemError::NoSuchSocket),
            (ring(), 0, 1, GemError::NotAGem),
            (ring(), 0, 9, GemError::NotAGem),
            (chest, 0, 0, GemError::EmptySlot),
            (SlotRef::Inventory { index: 1 }, 0, 0, GemError::NoSockets),
            (SlotRef::Inventory { index: 0 }, 0, 0, GemError::NoSockets),
            (head(), 2, 0, GemError::NoSuchSocket),
        ];
        for (item, socket_index, gem_slot, expected) in cases {
            let before = loadout.clone();
            assert_eq!(
                socket(&mut loadout, &item, socket_index, gem_slot, gems()),
                Err(expected)
            );
            assert_eq!(loadout, before);
        }
        socket(&mut loadout, &ring(), 0, 0, gems()).expect("first fits");
        let before = loadout.clone();
        assert_eq!(
            socket(&mut loadout, &ring(), 0, 0, gems()),
            Err(GemError::SocketOccupied)
        );
        assert_eq!(loadout, before);
        // The second socket of the helm can be filled first; the first stays empty.
        socket(&mut loadout, &head(), 1, 0, gems()).expect("second socket");
        assert_eq!(
            loadout.equipment[1].gems,
            vec![String::new(), "gem_haste_1".to_owned()]
        );
    }

    #[test]
    fn unsocketing_costs_gold_and_returns_the_gem_to_a_stack() {
        let mut loadout = worn(100);
        loadout.equipment[0].gems = vec!["gem_haste_3".to_owned()];
        put(&mut loadout, 5, "gem_haste_3", 2);
        let outcome = unsocket(&mut loadout, &ring(), 0, gems(), &economy()).expect("unsocketed");
        assert_eq!(outcome.cost, gems().unsocket_cost(3));
        assert_eq!(gold(&loadout), 100 - outcome.cost);
        assert_eq!(outcome.gold_left, gold(&loadout));
        assert!(loadout.equipment[0].gems.is_empty());
        assert_eq!(loadout.inventory.len(), 1, "joined the existing stack");
        assert_eq!(count(&loadout, "gem_haste_3"), 3);
    }

    #[test]
    fn unsocketing_refuses_a_poor_character_an_empty_socket_and_a_full_bag() {
        let economy = economy();
        let mut broke = worn(1);
        broke.equipment[0].gems = vec!["gem_haste_5".to_owned()];
        let before = broke.clone();
        assert!(matches!(
            unsocket(&mut broke, &ring(), 0, gems(), &economy),
            Err(GemError::Insufficient(_))
        ));
        assert_eq!(broke, before);

        let mut loadout = worn(1000);
        assert_eq!(
            unsocket(&mut loadout, &ring(), 0, gems(), &economy),
            Err(GemError::SocketEmpty)
        );
        assert_eq!(
            unsocket(&mut loadout, &ring(), 1, gems(), &economy),
            Err(GemError::NoSuchSocket)
        );

        // A bag with every cell taken by something that cannot take the gem.
        loadout.equipment[0].gems = vec!["gem_haste_1".to_owned()];
        fill_bag(&mut loadout);
        let before = loadout.clone();
        assert_eq!(
            unsocket(&mut loadout, &ring(), 0, gems(), &economy),
            Err(GemError::BagFull)
        );
        assert_eq!(loadout, before);
        // Room in an existing stack is room enough.
        loadout.inventory[0] = BagItem::new(0, "gem_haste_1", 4);
        unsocket(&mut loadout, &ring(), 0, gems(), &economy).expect("joins the stack");
        assert_eq!(loadout.inventory[0].quantity, 5);
    }

    #[test]
    fn a_successful_upgrade_consumes_n_and_yields_the_next_level() {
        let mut loadout = worn(1000);
        put(&mut loadout, 2, "gem_haste_1", 5);
        let outcome = upgrade(&mut loadout, 2, &mut succeed, gems(), &economy()).expect("rolled");
        assert!(outcome.success);
        assert_eq!((outcome.consumed, outcome.returned), (2, 0));
        assert_eq!(outcome.result, "gem_haste_2");
        assert_eq!(count(&loadout, "gem_haste_1"), 3);
        assert_eq!(count(&loadout, "gem_haste_2"), 1);
        assert_eq!(gold(&loadout), 1000 - gems().upgrade.steps[0].gold);
        assert_eq!(outcome.gold_left, gold(&loadout));
    }

    #[test]
    fn a_failed_upgrade_keeps_one_and_still_charges() {
        let mut loadout = worn(1000);
        put(&mut loadout, 2, "gem_haste_3", 2);
        let outcome = upgrade(&mut loadout, 2, &mut fail, gems(), &economy()).expect("rolled");
        assert!(!outcome.success);
        assert_eq!((outcome.consumed, outcome.returned), (2, 1));
        assert_eq!(count(&loadout, "gem_haste_3"), 1);
        assert_eq!(count(&loadout, "gem_haste_4"), 0);
        assert_eq!(gold(&loadout), 1000 - gems().upgrade.steps[2].gold);
    }

    #[test]
    fn an_upgrade_refuses_too_few_gems_too_little_gold_a_max_gem_and_a_full_bag() {
        let economy = economy();
        let mut draws = 0;
        let mut draw = || {
            draws += 1;
            0
        };
        let mut loadout = worn(1000);
        put(&mut loadout, 0, "gem_haste_1", 1);
        let before = loadout.clone();
        assert_eq!(
            upgrade(&mut loadout, 0, &mut draw, gems(), &economy),
            Err(GemError::NotEnoughGems(1, 2))
        );
        assert_eq!(
            upgrade(&mut loadout, 9, &mut draw, gems(), &economy),
            Err(GemError::NotAGem)
        );
        assert_eq!(loadout, before);

        let mut poor = worn(1);
        put(&mut poor, 0, "gem_haste_1", 2);
        let before = poor.clone();
        assert!(matches!(
            upgrade(&mut poor, 0, &mut draw, gems(), &economy),
            Err(GemError::Insufficient(_))
        ));
        assert_eq!(poor, before);

        let mut top = worn(1000);
        put(&mut top, 0, "gem_haste_5", 9);
        assert_eq!(
            upgrade(&mut top, 0, &mut draw, gems(), &economy),
            Err(GemError::MaxLevel)
        );
        assert_eq!(draws, 0, "a refusal draws nothing");

        // A bag with no room for the result refuses whatever the roll would have been.
        for roll in [0, u64::MAX] {
            let mut full = worn(1000);
            fill_bag(&mut full);
            full.inventory[3] = BagItem::new(3, "gem_haste_1", 3);
            let before = full.clone();
            assert_eq!(
                upgrade(&mut full, 3, &mut || roll, gems(), &economy),
                Err(GemError::BagFull)
            );
            assert_eq!(full, before);
        }
    }

    #[test]
    fn upgrade_success_follows_the_published_chance() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut draw = || {
            // splitmix64
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let (mut wins, trials) = (0_u32, 4000_u32);
        let chance = gems().upgrade.steps[1].success_chance;
        let economy = economy();
        for _ in 0..trials {
            let mut loadout = worn(1000);
            put(&mut loadout, 0, "gem_haste_2", 2);
            let outcome = upgrade(&mut loadout, 0, &mut draw, gems(), &economy).expect("rolled");
            wins += u32::from(outcome.success);
        }
        let seen = f64::from(wins) / f64::from(trials);
        assert!((seen - chance).abs() < 0.04, "{seen} vs {chance}");
    }

    #[test]
    fn socketed_gems_reach_the_sheet_unscaled_by_quality_and_sharpening() {
        let base = |quality: u8, sharpening: u8| {
            let mut loadout = worn(0);
            loadout.equipment[0].quality = quality;
            loadout.equipment[0].sharpening = sharpening;
            loadout
        };
        let stats = |loadout: &Loadout| sheet_of(Hero::FireMage, loadout).gear_stats();
        let kind = gems().kind("haste").expect("kind");
        let value = gems().value(kind, 4);
        for (quality, sharpening) in [(100, 0), (60, 0), (110, 9)] {
            let mut socketed = base(quality, sharpening);
            socketed.equipment[0].gems = vec!["gem_haste_4".to_owned()];
            let gained = stats(&socketed).get(Stat::Haste)
                - stats(&base(quality, sharpening)).get(Stat::Haste);
            assert!(
                (gained - value).abs() < 1e-4,
                "{quality}/{sharpening}: {gained} vs {value}"
            );
        }
        // Main stat gems resolve to the hero's own stat, like a piece's.
        let mut loadout = base(100, 0);
        loadout.equipment[1].gems = vec!["gem_main_stat_5".to_owned(); 2];
        let plain = stats(&base(100, 0));
        let with = stats(&loadout);
        let main = gems().kind("main_stat").expect("kind");
        let gained = with.get(Stat::Intellect) - plain.get(Stat::Intellect);
        assert!((gained - 2.0 * gems().value(main, 5)).abs() < 1e-4);
        assert_eq!(with.get(Stat::MainStat), 0.0);
    }

    #[test]
    fn a_forged_socket_on_an_item_without_one_grants_nothing() {
        let mut loadout = Loadout::default();
        let mut cowl = EquippedItem::new("head", "gravewardens_cowl");
        cowl.gems = vec!["gem_haste_5".to_owned()];
        loadout.equipment.push(cowl);
        let mut plain = loadout.clone();
        plain.equipment[0].gems.clear();
        assert_eq!(
            sheet_of(Hero::Warrior, &loadout).gear_stats(),
            sheet_of(Hero::Warrior, &plain).gear_stats()
        );
        // An unknown or non-gem key is an empty socket.
        let mut odd = Loadout::default();
        let mut worn_ring = EquippedItem::new("ring1", RING);
        worn_ring.gems = vec!["no_such_gem".to_owned()];
        odd.equipment.push(worn_ring);
        let stack = sheet_of(Hero::Warrior, &odd)
            .equipment()
            .get(EquipSlot::Ring1)
            .expect("worn");
        assert_eq!(stack.props.gems, [0, 0]);
    }

    #[test]
    fn the_drop_rules_cover_every_kind_and_level() {
        ensure_items_registered();
        let rules = gems().drop_rules();
        assert_eq!(rules.items.len(), gems().kinds.len());
        assert!(rules.items.iter().all(|levels| levels.len() == 5));
        assert_eq!(rules.sources.len(), 4);
        for source in [
            DropSource::Mob,
            DropSource::Elite,
            DropSource::Boss,
            DropSource::Chest,
        ] {
            assert!(rules.chance_at(source, 1, 0) > 0.0);
        }
        // The key level raises the chance, up to the cap.
        assert!(rules.chance_at(DropSource::Boss, 10, 0) > rules.chance_at(DropSource::Boss, 1, 0));
        assert!(
            rules.chance_at(DropSource::Mob, 1000, 0)
                <= 2.0 * rules.chance_at(DropSource::Mob, 1, 0) + 1e-6
        );
    }
}
