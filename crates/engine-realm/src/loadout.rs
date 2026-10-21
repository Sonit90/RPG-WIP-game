//! What a persistent character wears and carries, and the gear score that follows from it.
//!
//! ```text
//! persistence DB ─► Loadout (names) ─► CharacterSheet (engine-game items) ─► gear score, stats
//! ```
//!
//! A [`Loadout`] is the stored form: slot keys and item keys, exactly the vocabulary
//! `character.cfg` already uses, so the offline save and the online save describe a character
//! the same way. [`sheet_of`] is the only way back into gameplay types, and it checks every
//! entry on the way in — a stored row naming a removed item, or an item in a slot it does not
//! fit, becomes an empty slot rather than an illegal character.
//!
//! The gear score is **derived**, never stored and never accepted from a client: the
//! persistence API computes it from the loadout it owns with [`gear_score`], and the realm
//! module reads it from the API. Nothing a client sends can change it.

use std::collections::BTreeMap;

use engine_game::character::MailItem as SimMail;
use engine_game::character::{
    CharacterSheet, EquipSlot, GEM_SOCKETS_MAX, INVENTORY_SLOTS, Inventory, ItemFamily, ItemId,
    ItemProps, ItemSlot, ItemStack, MailReason, MailSender, MoveError, QUALITY_DEFAULT, item,
    item_by_key,
};
use engine_game::hero::Hero;
use serde::{Deserialize, Serialize};

use crate::error::RealmError;
use crate::loot::ensure_items_registered;

/// The quality of a stored item that has none: rows saved before quality existed.
fn default_quality() -> u8 {
    QUALITY_DEFAULT
}

fn is_default_quality(quality: &u8) -> bool {
    *quality == QUALITY_DEFAULT
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The numeric gem ids of stored gem keys, in socket order. An empty or unknown key is an
/// empty socket.
pub fn gem_ids_of(keys: &[String]) -> [u16; GEM_SOCKETS_MAX] {
    let mut ids = [0; GEM_SOCKETS_MAX];
    for (id, key) in ids.iter_mut().zip(keys) {
        ensure_items_registered();
        if let Some(def) = item_by_key(key).filter(|def| def.family == ItemFamily::Gem) {
            *id = def.id.0;
        }
    }
    ids
}

/// The stored keys of gem ids, trailing empty sockets trimmed (empty sockets in the middle are
/// the empty string).
pub fn gem_keys_of(ids: [u16; GEM_SOCKETS_MAX]) -> Vec<String> {
    let mut keys: Vec<String> = ids
        .into_iter()
        .map(|id| {
            if id == 0 {
                String::new()
            } else {
                item(ItemId(id)).map_or_else(String::new, |def| def.key.to_owned())
            }
        })
        .collect();
    while keys.last().is_some_and(String::is_empty) {
        keys.pop();
    }
    keys
}

/// `props` with the gems an item of `key` cannot hold removed: a forged row cannot socket an
/// item that has no socket, nor more gems than it has.
fn fit_sockets(key: &str, mut props: ItemProps) -> ItemProps {
    let sockets = item_by_key(key).map_or(0, |def| usize::from(def.sockets));
    for gem in props.gems.iter_mut().skip(sockets) {
        *gem = 0;
    }
    props
}

/// One worn item, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquippedItem {
    /// The slot's stable key (`"head"`, `"ring1"` …).
    pub slot: String,
    /// The item's stable key.
    pub item: String,
    /// Rolled quality, per cent of the base stats. Absent in older rows: 100.
    #[serde(
        default = "default_quality",
        skip_serializing_if = "is_default_quality"
    )]
    pub quality: u8,
    /// Sharpening level. Absent in older rows: 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[serde(rename = "enhancement", alias = "sharpening")]
    pub sharpening: u8,
    /// Whether it broke. Absent in older rows: false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub broken: bool,
    /// The key of the gem in each socket, `""` for an empty one. Absent in older rows: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gems: Vec<String>,
}

impl EquippedItem {
    /// A worn item of default props.
    #[must_use]
    pub fn new(slot: &str, item: &str) -> Self {
        Self {
            slot: slot.to_owned(),
            item: item.to_owned(),
            quality: QUALITY_DEFAULT,
            ..Self::default()
        }
    }

    /// Its instance properties, clamped into the legal ranges.
    #[must_use]
    pub fn props(&self) -> ItemProps {
        ItemProps::new(self.quality, self.sharpening, self.broken)
            .with_gems(gem_ids_of(&self.gems))
    }

    fn set_props(&mut self, props: ItemProps) {
        (self.quality, self.sharpening, self.broken) =
            (props.quality, props.sharpening, props.broken);
        self.gems = gem_keys_of(props.gems);
    }
}

/// One carried stack, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BagItem {
    /// The bag cell.
    pub index: u32,
    /// The item's stable key.
    pub item: String,
    /// How many.
    pub quantity: u16,
    /// Rolled quality, per cent of the base stats. Absent in older rows: 100.
    #[serde(
        default = "default_quality",
        skip_serializing_if = "is_default_quality"
    )]
    pub quality: u8,
    /// Sharpening level. Absent in older rows: 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[serde(rename = "enhancement", alias = "sharpening")]
    pub sharpening: u8,
    /// Whether it broke. Absent in older rows: false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub broken: bool,
    /// The key of the gem in each socket, `""` for an empty one. Absent in older rows: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gems: Vec<String>,
}

impl BagItem {
    /// A carried stack of default props.
    #[must_use]
    pub fn new(index: u32, item: &str, quantity: u16) -> Self {
        Self {
            index,
            item: item.to_owned(),
            quantity,
            quality: QUALITY_DEFAULT,
            ..Self::default()
        }
    }

    /// Its instance properties, clamped into the legal ranges.
    #[must_use]
    pub fn props(&self) -> ItemProps {
        ItemProps::new(self.quality, self.sharpening, self.broken)
            .with_gems(gem_ids_of(&self.gems))
    }

    fn set_props(&mut self, props: ItemProps) {
        (self.quality, self.sharpening, self.broken) =
            (props.quality, props.sharpening, props.broken);
        self.gems = gem_keys_of(props.gems);
    }
}

/// One piece of mail: an item waiting to be taken into the bag.
///
/// Mail is how loot that could not be handed over in the dungeon (the bag was full, or the
/// chest was never opened) still reaches the character.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailItem {
    /// Stable within the character; never reused.
    pub id: u64,
    /// The attached item's stable key.
    pub item: String,
    /// The attachment's quality, per cent of the base stats. Absent in older rows: 100.
    #[serde(
        default = "default_quality",
        skip_serializing_if = "is_default_quality"
    )]
    pub quality: u8,
    /// The attachment's sharpening level. Absent in older rows: 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[serde(rename = "enhancement", alias = "sharpening")]
    pub sharpening: u8,
    /// Whether the attachment is broken. Absent in older rows: false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub broken: bool,
    /// The key of the gem in each of the attachment's sockets. Absent in older rows: none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gems: Vec<String>,
    /// Who it is from.
    pub sender: String,
    /// The subject line.
    pub subject: String,
    /// Why it was mailed: [`MAIL_BAG_FULL`] or [`MAIL_UNCLAIMED`].
    pub reason: String,
    /// When it arrived, unix seconds.
    pub received_at: i64,
    /// Whether the player has opened it.
    pub read: bool,
}

impl MailItem {
    /// The attachment's instance properties, clamped into the legal ranges.
    #[must_use]
    pub fn props(&self) -> ItemProps {
        ItemProps::new(self.quality, self.sharpening, self.broken)
            .with_gems(gem_ids_of(&self.gems))
    }
}

fn mail_subject(item_name: &str) -> String {
    format!("{item_name} from the dungeon chest")
}

fn removed_slot_subject(item_name: &str) -> String {
    format!("{item_name} from a removed slot")
}

fn dungeon_drop_subject(item_name: &str) -> String {
    format!("{item_name} dropped in the dungeon")
}

/// Mailed because the bag had no room when the item was claimed.
pub const MAIL_BAG_FULL: &str = "bag_full";
/// Mailed because the chest was never opened.
pub const MAIL_UNCLAIMED: &str = "unclaimed";
/// Mailed because the item was worn in an equipment slot that no longer exists and the bag
/// had no room for it.
pub const MAIL_SLOT_REMOVED: &str = "slot_removed";
/// Mailed because a monster dropped it in the dungeon.
pub const MAIL_DUNGEON_DROP: &str = "dungeon_drop";
/// Who dungeon loot mail is from.
pub const MAIL_SENDER_CHEST: &str = "Dungeon Chest";
/// Who a monster's drop is from.
pub const MAIL_SENDER_DUNGEON: &str = "Dungeon";
/// Who housekeeping mail is from.
pub const MAIL_SENDER_SYSTEM: &str = "System";

/// The version [`Loadout`] is written with. Reading accepts every older one through [`migrate`].
///
/// 0 and 1 are the P1 shape (rows that never said); 2 is the same shape stamped, the starting
/// point for the field additions that follow; 3 adds gems (`gems` on worn, carried and mailed
/// items): each bump adds an arm to [`migrate`], with a test over a captured row of the version
/// before.
pub const CURRENT_VERSION: u32 = 3;

/// Everything a character wears and carries, in stored form.
///
/// Reading goes through [`migrate`] (every deserialization of a `Loadout` does), so an old
/// row comes out as the current shape; writing always stamps [`CURRENT_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct Loadout {
    /// The stored shape's version; see [`CURRENT_VERSION`]. Rows saved before versioning say 0.
    #[serde(default)]
    pub version: u32,
    /// What is worn.
    pub equipment: Vec<EquippedItem>,
    /// What is carried.
    pub inventory: Vec<BagItem>,
    /// What is waiting in the mailbox. Absent in rows saved before mail existed.
    #[serde(default)]
    pub mail: Vec<MailItem>,
    /// Reagents held, by resource id (see the economy catalog). Absent in older rows.
    #[serde(default)]
    pub resources: BTreeMap<String, u64>,
}

impl Default for Loadout {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            equipment: Vec::new(),
            inventory: Vec::new(),
            mail: Vec::new(),
            resources: BTreeMap::new(),
        }
    }
}

/// A stored loadout of any version known so far, before [`migrate`] lifts it.
#[derive(Deserialize)]
struct StoredLoadout {
    #[serde(default)]
    equipment: Vec<EquippedItem>,
    #[serde(default)]
    inventory: Vec<BagItem>,
    #[serde(default)]
    mail: Vec<MailItem>,
    #[serde(default)]
    resources: BTreeMap<String, u64>,
}

/// Why a stored loadout could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadoutError {
    /// Written by a newer build than this one: reading it would silently drop what it added.
    NewerThanKnown(u64),
    /// Not a loadout.
    Malformed(String),
}

impl std::fmt::Display for LoadoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerThanKnown(version) => write!(
                f,
                "loadout version {version} is newer than this build reads ({CURRENT_VERSION})"
            ),
            Self::Malformed(reason) => write!(f, "malformed loadout: {reason}"),
        }
    }
}

impl std::error::Error for LoadoutError {}

impl TryFrom<serde_json::Value> for Loadout {
    type Error = LoadoutError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        migrate(value)
    }
}

/// Reads a stored loadout of any version and returns it in the current shape.
///
/// Versions 0 and 1 are the P1 shape and differ from the current one only in the stamp. A
/// version this build does not know is refused rather than read with fields dropped.
///
/// # Errors
///
/// The value is not a loadout, or carries a version newer than [`CURRENT_VERSION`].
pub fn migrate(value: serde_json::Value) -> Result<Loadout, LoadoutError> {
    let version = match value.get("version") {
        None | Some(serde_json::Value::Null) => 0,
        Some(found) => found
            .as_u64()
            .ok_or_else(|| LoadoutError::Malformed("version is not a number".into()))?,
    };
    if version > u64::from(CURRENT_VERSION) {
        return Err(LoadoutError::NewerThanKnown(version));
    }
    // 0 and 1: the P1 shape. 2 differs from 3 only in lacking `gems`, which every item row
    // defaults to none, so an old save loads with empty sockets and nothing else changes.
    // Later versions add an arm here that lifts the one before.
    let stored: StoredLoadout =
        serde_json::from_value(value).map_err(|e| LoadoutError::Malformed(e.to_string()))?;
    Ok(Loadout {
        version: CURRENT_VERSION,
        equipment: stored.equipment,
        inventory: stored.inventory,
        mail: stored.mail,
        resources: stored.resources,
    })
}

/// Where an item is, in stored form: a slot key or a bag cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotRef {
    /// A worn slot, by key.
    Equipment {
        /// The slot's key.
        slot: String,
    },
    /// A bag cell.
    Inventory {
        /// The cell index.
        index: u32,
    },
}

impl SlotRef {
    /// The gameplay address this names, if it names one.
    #[must_use]
    pub fn resolve(&self) -> Option<ItemSlot> {
        match self {
            Self::Equipment { slot } => EquipSlot::from_key(slot).map(ItemSlot::Equipment),
            Self::Inventory { index } => usize::try_from(*index).ok().map(ItemSlot::Inventory),
        }
    }
}

/// What a new character starts with: nothing worn, nothing carried.
#[must_use]
pub fn starting_loadout(hero: Hero) -> Loadout {
    loadout_of(&CharacterSheet::starting(hero))
}

/// The stored form of a character sheet.
#[must_use]
pub fn loadout_of(sheet: &CharacterSheet) -> Loadout {
    ensure_items_registered();
    let mut loadout = Loadout::default();
    for (slot, stack) in sheet.equipment().worn() {
        if let Some(def) = stack.def() {
            let mut entry = EquippedItem::new(slot.key(), def.key);
            entry.set_props(stack.props);
            loadout.equipment.push(entry);
        }
    }
    for (index, stack) in sheet.inventory().contents() {
        if let (Some(def), Ok(index)) = (stack.def(), u32::try_from(index)) {
            let mut entry = BagItem::new(index, def.key, stack.quantity);
            entry.set_props(stack.props);
            loadout.inventory.push(entry);
        }
    }
    for letter in sheet.mailbox().letters() {
        let Some(def) = engine_game::character::item(letter.item) else {
            continue;
        };
        loadout.mail.push(MailItem {
            id: letter.id,
            item: def.key.to_owned(),
            quality: letter.props.quality,
            sharpening: letter.props.sharpening,
            broken: letter.props.broken,
            gems: gem_keys_of(letter.props.gems),
            sender: letter.sender.name().to_owned(),
            subject: match letter.reason {
                MailReason::SlotRemoved => removed_slot_subject(def.name),
                MailReason::DungeonDrop => dungeon_drop_subject(def.name),
                _ => mail_subject(def.name),
            },
            reason: match letter.reason {
                MailReason::BagFull => MAIL_BAG_FULL,
                MailReason::Unclaimed => MAIL_UNCLAIMED,
                MailReason::SlotRemoved => MAIL_SLOT_REMOVED,
                MailReason::DungeonDrop => MAIL_DUNGEON_DROP,
            }
            .to_owned(),
            received_at: letter.received_at as i64,
            read: letter.read,
        });
    }
    loadout
}

/// A legal character sheet from a stored loadout. Entries that cannot be honoured are
/// dropped and logged, never trusted.
#[must_use]
pub fn sheet_of(hero: Hero, loadout: &Loadout) -> CharacterSheet {
    ensure_items_registered();
    // Rows saved while there were four trinket slots still name `trinket3` / `trinket4`.
    // Rehome them first so the item reaches the bag (or the mailbox) rather than vanishing.
    let migrated;
    let loadout = if has_removed_slots(loadout) {
        let mut copy = loadout.clone();
        migrate_removed_slots(&mut copy, 0);
        migrated = copy;
        &migrated
    } else {
        loadout
    };
    let mut sheet = CharacterSheet::bare(hero);
    for entry in &loadout.equipment {
        let (Some(slot), Some(def)) = (EquipSlot::from_key(&entry.slot), item_by_key(&entry.item))
        else {
            log::warn!("loadout: dropped {:?} in {:?}", entry.item, entry.slot);
            continue;
        };
        if sheet
            .equipment_mut()
            .equip(
                slot,
                ItemStack::with_props(def.id, fit_sockets(def.key, entry.props())),
            )
            .is_err()
        {
            log::warn!("loadout: {} does not fit {}", def.key, slot.key());
        }
    }
    let mut inventory = Inventory::empty();
    for entry in &loadout.inventory {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!("loadout: dropped unknown item {:?}", entry.item);
            continue;
        };
        let Ok(index) = usize::try_from(entry.index) else {
            continue;
        };
        if index >= inventory.len() || inventory.get(index).is_some() {
            log::warn!("loadout: bag cell {index} is unusable; dropped {}", def.key);
            continue;
        }
        let quantity = entry.quantity.clamp(1, def.max_stack.max(1));
        let mut stack = ItemStack::new(def.id, quantity);
        stack.props = fit_sockets(def.key, entry.props());
        inventory.put(index, Some(stack));
    }
    *sheet.inventory_mut() = inventory;
    for letter in &loadout.mail {
        let Some(def) = item_by_key(&letter.item) else {
            log::warn!(
                "loadout: dropped mail {} with unknown item {:?}",
                letter.id,
                letter.item
            );
            continue;
        };
        sheet.mailbox_mut().restore(SimMail {
            id: letter.id,
            item: def.id,
            props: fit_sockets(&letter.item, letter.props()),
            sender: match letter.sender.as_str() {
                MAIL_SENDER_SYSTEM => MailSender::System,
                MAIL_SENDER_DUNGEON => MailSender::Dungeon,
                _ => MailSender::DungeonChest,
            },
            reason: match letter.reason.as_str() {
                MAIL_BAG_FULL => MailReason::BagFull,
                MAIL_SLOT_REMOVED => MailReason::SlotRemoved,
                MAIL_DUNGEON_DROP => MailReason::DungeonDrop,
                _ => MailReason::Unclaimed,
            },
            received_at: letter.received_at as f64,
            read: letter.read,
        });
    }
    sheet
}

/// Gear score: ten times the summed item level of everything worn, each scaled by its item's
/// props like its stats are.
///
/// Summed rather than averaged over the slots, so an empty slot costs what the item in it
/// would have been worth and a character with nothing worn is 0 rather than a fraction. Ten
/// times so a single item level is a visible step in a three-digit number. An item of default
/// props counts exactly its item level; a 110 per cent piece counts a tenth more and a broken
/// one 15 per cent less.
#[must_use]
pub fn gear_score_of(sheet: &CharacterSheet) -> u32 {
    let levels: f32 = sheet
        .equipment()
        .worn()
        .map(|(_, stack)| stack.effective_item_level())
        .sum();
    (levels * 10.0).round() as u32
}

/// The gear score of a stored loadout, after the same validation [`sheet_of`] applies — so an
/// illegal row cannot raise it.
#[must_use]
pub fn gear_score(hero: Hero, loadout: &Loadout) -> u32 {
    gear_score_of(&sheet_of(hero, loadout))
}

/// Moves an item, server-side. Returns the new loadout or why nothing changed.
///
/// # Errors
///
/// A persistence error with a player-facing sentence when an address is unknown, empty, the
/// same, or the item does not fit where it was dropped.
pub fn move_item(
    hero: Hero,
    loadout: &Loadout,
    from: &SlotRef,
    to: &SlotRef,
) -> Result<Loadout, RealmError> {
    let (Some(from), Some(to)) = (from.resolve(), to.resolve()) else {
        return Err(RealmError::protocol("That slot does not exist."));
    };
    // Rehome anything in a removed slot first, so the mail it may create is kept below.
    let mut loadout = loadout.clone();
    migrate_removed_slots(&mut loadout, 0);
    let loadout = &loadout;
    let mut sheet = sheet_of(hero, loadout);
    sheet.move_item(from, to).map_err(|error| {
        RealmError::persistence(match error {
            MoveError::Empty => "There is nothing to move.",
            MoveError::SameSlot => "The item is already there.",
            MoveError::NoSuchSlot => "That slot does not exist.",
            MoveError::Incompatible => "That item does not go there.",
        })
    })?;
    let mut moved = loadout_of(&sheet);
    // Moving gear never touches the mailbox or the resources.
    moved.mail.clone_from(&loadout.mail);
    moved.resources.clone_from(&loadout.resources);
    Ok(moved)
}

/// Why an item could not be put in the bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BagError {
    /// Every cell is taken.
    Full,
    /// No such item.
    UnknownItem,
}

/// Puts one `key` item in the first free bag cell and returns the cell. Used by the
/// persistence API to deliver a chest drop; the key is validated against the item tables.
///
/// # Errors
///
/// [`BagError::UnknownItem`] for a key no item has; [`BagError::Full`] when all
/// [`INVENTORY_SLOTS`] cells are taken (the loadout is left as it was).
pub fn add_to_bag(loadout: &mut Loadout, key: &str) -> Result<u32, BagError> {
    add_stack_to_bag(loadout, key, ItemProps::STANDARD)
}

/// [`add_to_bag`] for an item with these props: what a rolled chest or monster drop is
/// delivered with.
///
/// # Errors
///
/// As [`add_to_bag`].
pub fn add_stack_to_bag(
    loadout: &mut Loadout,
    key: &str,
    props: ItemProps,
) -> Result<u32, BagError> {
    ensure_items_registered();
    let def = item_by_key(key).ok_or(BagError::UnknownItem)?;
    // A stackable item joins a stack of the same item and props that has room.
    if def.max_stack > 1
        && let Some(entry) = loadout.inventory.iter_mut().find(|entry| {
            entry.item == def.key && entry.props() == props && entry.quantity < def.max_stack
        })
    {
        entry.quantity += 1;
        return Ok(entry.index);
    }
    let free = (0..INVENTORY_SLOTS)
        .filter_map(|cell| u32::try_from(cell).ok())
        .find(|cell| loadout.inventory.iter().all(|entry| entry.index != *cell))
        .ok_or(BagError::Full)?;
    let mut entry = BagItem::new(free, def.key, 1);
    entry.set_props(props);
    loadout.inventory.push(entry);
    loadout.inventory.sort_by_key(|entry| entry.index);
    Ok(free)
}

/// Why mail could not be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeMailError {
    /// There is no such mail.
    NotFound,
    /// The bag has no free cell; the mail stays.
    BagFull,
}

/// Puts a `key` item in the mailbox and returns the mail's id.
///
/// # Errors
///
/// [`BagError::UnknownItem`] for a key no item has.
pub fn add_to_mail(
    loadout: &mut Loadout,
    key: &str,
    reason: &str,
    received_at: i64,
) -> Result<u64, BagError> {
    add_stack_to_mail(loadout, key, ItemProps::STANDARD, reason, received_at)
}

/// [`add_to_mail`] for an item with these props.
///
/// # Errors
///
/// As [`add_to_mail`].
pub fn add_stack_to_mail(
    loadout: &mut Loadout,
    key: &str,
    props: ItemProps,
    reason: &str,
    received_at: i64,
) -> Result<u64, BagError> {
    ensure_items_registered();
    let def = item_by_key(key).ok_or(BagError::UnknownItem)?;
    let (sender, subject) = if reason == MAIL_DUNGEON_DROP {
        (MAIL_SENDER_DUNGEON, dungeon_drop_subject(def.name))
    } else {
        (MAIL_SENDER_CHEST, mail_subject(def.name))
    };
    Ok(push_mail(
        loadout,
        def.key,
        props,
        sender,
        subject,
        reason,
        received_at,
    ))
}

fn push_mail(
    loadout: &mut Loadout,
    key: &str,
    props: ItemProps,
    sender: &str,
    subject: String,
    reason: &str,
    received_at: i64,
) -> u64 {
    let id = loadout
        .mail
        .iter()
        .map(|m| m.id)
        .max()
        .map_or(1, |id| id + 1);
    loadout.mail.push(MailItem {
        id,
        item: key.to_owned(),
        quality: props.quality,
        sharpening: props.sharpening,
        broken: props.broken,
        gems: gem_keys_of(props.gems),
        sender: sender.to_owned(),
        subject,
        reason: reason.to_owned(),
        received_at,
        read: false,
    });
    id
}

/// Whether any worn entry names an equipment slot that no longer exists.
#[must_use]
pub fn has_removed_slots(loadout: &Loadout) -> bool {
    loadout
        .equipment
        .iter()
        .any(|entry| EquipSlot::REMOVED_KEYS.contains(&entry.slot.as_str()))
}

/// **Rehomes every item worn in a removed slot** (`trinket3`, `trinket4`): into the first free
/// bag cell, or into the mailbox ([`MAIL_SLOT_REMOVED`], from the System) when the bag is full.
/// Items whose key no item has are dropped, as everywhere else. `received_at` is the unix time
/// stamped on any letter. Returns how many items moved.
pub fn migrate_removed_slots(loadout: &mut Loadout, received_at: i64) -> usize {
    ensure_items_registered();
    let (removed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut loadout.equipment)
        .into_iter()
        .partition(|entry| EquipSlot::REMOVED_KEYS.contains(&entry.slot.as_str()));
    loadout.equipment = kept;
    let mut moved = 0;
    for entry in removed {
        let Some(def) = item_by_key(&entry.item) else {
            log::warn!(
                "loadout: dropped unknown item {:?} in {:?}",
                entry.item,
                entry.slot
            );
            continue;
        };
        let props = entry.props();
        if add_stack_to_bag(loadout, def.key, props).is_err() {
            push_mail(
                loadout,
                def.key,
                props,
                MAIL_SENDER_SYSTEM,
                removed_slot_subject(def.name),
                MAIL_SLOT_REMOVED,
                received_at,
            );
        }
        moved += 1;
    }
    moved
}

/// Moves mail `mail_id`'s attachment into the first free bag cell and deletes the mail.
/// Returns the cell.
///
/// # Errors
///
/// [`TakeMailError::NotFound`] or [`TakeMailError::BagFull`]; the loadout is unchanged.
pub fn take_mail(loadout: &mut Loadout, mail_id: u64) -> Result<u32, TakeMailError> {
    let index = loadout
        .mail
        .iter()
        .position(|m| m.id == mail_id)
        .ok_or(TakeMailError::NotFound)?;
    let key = loadout.mail[index].item.clone();
    let letter = &loadout.mail[index];
    let props = letter.props();
    match add_stack_to_bag(loadout, &key, props) {
        Ok(cell) => {
            loadout.mail.remove(index);
            Ok(cell)
        }
        Err(BagError::Full) => Err(TakeMailError::BagFull),
        // An item that no longer exists cannot be taken; the mail is dropped, not stuck.
        Err(BagError::UnknownItem) => {
            loadout.mail.remove(index);
            Err(TakeMailError::NotFound)
        }
    }
}

/// Marks mail read. `false` if there is no such mail.
pub fn mark_mail_read(loadout: &mut Loadout, mail_id: u64) -> bool {
    loadout
        .mail
        .iter_mut()
        .find(|m| m.id == mail_id)
        .map(|m| m.read = true)
        .is_some()
}

/// A character's derived stats as `(key, value)` pairs, for a snapshot or a tooltip.
#[must_use]
pub fn stats_of(sheet: &CharacterSheet) -> Vec<(String, f32)> {
    let stats = sheet.stats();
    engine_game::character::Stat::ALL
        .into_iter()
        .map(|stat| (stat.key().to_owned(), stats.get(stat)))
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_game::character::{ItemFamily, starting_item};

    /// A loadout with the white kit new heroes used to start with: four pieces worn.
    fn kit_loadout() -> Loadout {
        let mut loadout = Loadout::default();
        for slot in [
            EquipSlot::Chest,
            EquipSlot::Legs,
            EquipSlot::Wrist,
            EquipSlot::Feet,
        ] {
            let def = starting_item(slot.family()).expect("a white piece");
            loadout
                .equipment
                .push(EquippedItem::new(slot.key(), def.key));
        }
        loadout
    }

    /// A P1 row as the database holds it: no `version`, qualities and sharpenings on worn,
    /// carried and mailed items (defaults omitted), mail, resources. Captured, not generated.
    const P1_LOADOUT: &str = r#"{
        "equipment": [
            {"slot": "ring1", "item": "ashgraves_seal", "quality": 106, "enhancement": 3},
            {"slot": "chest", "item": "ashgraves_seal", "broken": true}
        ],
        "inventory": [
            {"index": 0, "item": "ashgraves_seal", "quantity": 1, "quality": 92},
            {"index": 4, "item": "ashgraves_seal", "quantity": 2}
        ],
        "mail": [
            {"id": 7, "item": "ashgraves_seal", "quality": 110, "enhancement": 1,
             "sender": "System", "subject": "Dungeon drop", "reason": "dungeon_drop",
             "received_at": 1760000000, "read": false}
        ],
        "resources": {"gold": 120, "crypt_dust": 35}
    }"#;

    #[test]
    fn sharpening_is_written_as_the_stable_enhancement_key_and_either_name_loads() {
        let loadout: Loadout = serde_json::from_str(P1_LOADOUT).expect("P1 row");
        let written = serde_json::to_value(&loadout).expect("serialize");
        let ring = &written["equipment"][0];
        assert_eq!(ring["enhancement"], 3, "the persisted key does not change");
        assert!(ring.get("sharpening").is_none());
        let renamed = P1_LOADOUT.replace("\"enhancement\"", "\"sharpening\"");
        let alias: Loadout = serde_json::from_str(&renamed).expect("alias loads");
        assert_eq!(alias, loadout);
    }

    #[test]
    fn a_p1_row_loads_unchanged_and_is_stamped_current() {
        let value: serde_json::Value = serde_json::from_str(P1_LOADOUT).expect("json");
        let loadout = migrate(value).expect("a P1 row");
        assert_eq!(loadout.version, CURRENT_VERSION);
        assert_eq!(loadout.equipment.len(), 2);
        assert_eq!(loadout.equipment[0].slot, "ring1");
        assert_eq!(
            (
                loadout.equipment[0].quality,
                loadout.equipment[0].sharpening
            ),
            (106, 3)
        );
        assert!(loadout.equipment[1].broken);
        assert_eq!(loadout.equipment[1].quality, QUALITY_DEFAULT);
        assert_eq!(loadout.inventory[0].quality, 92);
        assert_eq!(loadout.inventory[1].quantity, 2);
        assert_eq!(loadout.mail[0].id, 7);
        assert_eq!(
            (loadout.mail[0].quality, loadout.mail[0].sharpening),
            (110, 1)
        );
        assert_eq!(loadout.resources.get("gold"), Some(&120));
        assert_eq!(loadout.resources.get("crypt_dust"), Some(&35));
        // Deserializing a `Loadout` directly is the same path.
        let direct: Loadout = serde_json::from_str(P1_LOADOUT).expect("direct");
        assert_eq!(direct, loadout);
    }

    #[test]
    fn versions_zero_and_one_read_alike_and_a_write_says_current() {
        let mut one: serde_json::Value = serde_json::from_str(P1_LOADOUT).expect("json");
        let zero = migrate(one.clone()).expect("v0");
        one["version"] = 1.into();
        assert_eq!(migrate(one).expect("v1"), zero);
        let written = serde_json::to_value(&zero).expect("serializes");
        assert_eq!(written["version"], CURRENT_VERSION);
        // And what was written reads back identical.
        assert_eq!(migrate(written).expect("current"), zero);
    }

    #[test]
    fn a_version_two_row_loads_with_empty_sockets_and_keeps_everything() {
        let v2 = serde_json::json!({
            "version": 2,
            "equipment": [{"slot": "ring1", "item": "ashgraves_seal", "quality": 104, "enhancement": 3}],
            "inventory": [{"index": 4, "item": "gravewardens_cowl", "quantity": 1}],
            "mail": [],
            "resources": {"gold": 77}
        });
        let loadout = migrate(v2).expect("v2 reads");
        assert_eq!(loadout.version, CURRENT_VERSION);
        assert_eq!(loadout.equipment[0].quality, 104);
        assert_eq!(loadout.equipment[0].sharpening, 3);
        assert!(loadout.equipment[0].gems.is_empty());
        assert_eq!(loadout.resources.get("gold"), Some(&77));
        assert_eq!(loadout.inventory.len(), 1);
        let props = loadout.equipment[0].props();
        assert_eq!(props.gems, [0, 0]);
    }

    #[test]
    fn socketed_gems_round_trip_through_json_and_the_sheet() {
        let mut loadout = Loadout::default();
        let mut ring = EquippedItem::new("ring1", "ashgraves_seal");
        ring.gems = vec!["gem_haste_2".to_owned()];
        loadout.equipment.push(ring);
        let written = serde_json::to_value(&loadout).expect("serializes");
        assert_eq!(written["equipment"][0]["gems"][0], "gem_haste_2");
        assert_eq!(migrate(written).expect("reads"), loadout);
        let sheet = sheet_of(Hero::Warrior, &loadout);
        assert_eq!(loadout_of(&sheet), loadout);
        // Without gems the key is not written at all.
        let plain = serde_json::to_value(Loadout::default()).expect("serializes");
        assert!(plain.to_string().find("gems").is_none());
    }

    #[test]
    fn a_newer_or_broken_row_is_refused_not_trimmed() {
        let newer = serde_json::json!({"version": CURRENT_VERSION + 1, "equipment": []});
        assert_eq!(
            migrate(newer),
            Err(LoadoutError::NewerThanKnown(u64::from(CURRENT_VERSION) + 1))
        );
        assert!(matches!(
            migrate(serde_json::json!({"version": "two"})),
            Err(LoadoutError::Malformed(_))
        ));
        assert!(matches!(
            migrate(serde_json::json!({"equipment": "head"})),
            Err(LoadoutError::Malformed(_))
        ));
    }

    #[test]
    fn starting_loadout_is_empty_and_round_trips() {
        for hero in Hero::ALL {
            let loadout = starting_loadout(hero);
            assert_eq!(loadout, Loadout::default());
            assert_eq!(loadout_of(&sheet_of(hero, &loadout)), loadout);
        }
    }

    #[test]
    fn starting_gear_score_is_zero() {
        for hero in Hero::ALL {
            assert_eq!(gear_score(hero, &starting_loadout(hero)), 0);
        }
        assert_eq!(gear_score(Hero::Warrior, &kit_loadout()), 40);
    }

    fn trinket_key() -> &'static str {
        starting_item(ItemFamily::Trinket).expect("a trinket").key
    }

    fn old_loadout_with_four_trinkets() -> Loadout {
        let mut loadout = kit_loadout();
        for slot in ["trinket1", "trinket2", "trinket3", "trinket4"] {
            loadout
                .equipment
                .push(EquippedItem::new(slot, trinket_key()));
        }
        loadout
    }

    #[test]
    fn items_in_removed_slots_move_to_the_first_free_bag_cells() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        old.inventory.push(BagItem::new(0, trinket_key(), 1));
        let sheet = sheet_of(hero, &old);
        assert!(sheet.equipment().get(EquipSlot::Trinket1).is_some());
        assert!(sheet.equipment().get(EquipSlot::Trinket2).is_some());
        // Cell 0 was taken, so the two displaced trinkets land in 1 and 2.
        assert_eq!(sheet.inventory().count(), 3);
        assert!(sheet.inventory().get(1).is_some() && sheet.inventory().get(2).is_some());
        assert!(sheet.mailbox().is_empty());
        // And the stored form no longer names the removed slots.
        let stored = loadout_of(&sheet);
        assert!(!has_removed_slots(&stored));
    }

    #[test]
    fn items_in_removed_slots_are_mailed_when_the_bag_is_full() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        let key = engine_game::character::loot_pool()[0].key;
        while add_to_bag(&mut old, key).is_ok() {}
        let sheet = sheet_of(hero, &old);
        assert_eq!(sheet.inventory().count(), INVENTORY_SLOTS);
        assert_eq!(sheet.mailbox().len(), 2);
        for letter in sheet.mailbox().letters() {
            assert_eq!(letter.reason, MailReason::SlotRemoved);
            assert_eq!(letter.sender, MailSender::System);
        }
        let stored = loadout_of(&sheet);
        assert_eq!(stored.mail.len(), 2);
        assert_eq!(stored.mail[0].reason, MAIL_SLOT_REMOVED);
        // Round trips through the stored form.
        assert_eq!(sheet_of(hero, &stored).mailbox().len(), 2);
    }

    #[test]
    fn migrating_a_stored_loadout_keeps_the_mail_across_a_move() {
        let hero = Hero::Warrior;
        let mut old = old_loadout_with_four_trinkets();
        let key = engine_game::character::loot_pool()[0].key;
        while add_to_bag(&mut old, key).is_ok() {}
        let from = SlotRef::Inventory { index: 0 };
        let to = SlotRef::Inventory { index: 1 };
        let moved = move_item(hero, &old, &from, &to).expect("two full cells swap");
        assert!(!has_removed_slots(&moved));
        assert_eq!(moved.mail.len(), 2, "the rehomed items were lost");
    }

    #[test]
    fn migrating_in_place_reports_how_many_items_moved() {
        let mut old = old_loadout_with_four_trinkets();
        assert!(has_removed_slots(&old));
        assert_eq!(migrate_removed_slots(&mut old, 77), 2);
        assert!(!has_removed_slots(&old));
        assert_eq!(old.inventory.len(), 2);
        assert_eq!(migrate_removed_slots(&mut old, 77), 0);
    }

    #[test]
    fn forged_rows_cannot_raise_the_gear_score() {
        let hero = Hero::Warrior;
        let mut loadout = kit_loadout();
        let honest = gear_score(hero, &loadout);
        // An item that does not exist, and a real item in a slot it does not fit.
        loadout
            .equipment
            .push(EquippedItem::new("trinket1", "legendary_sword_of_lies"));
        let chest = loadout
            .equipment
            .iter()
            .find(|entry| entry.slot == "chest")
            .map(|entry| entry.item.clone())
            .expect("the starting set has a chest");
        loadout.equipment.push(EquippedItem::new("ring1", &chest));
        assert_eq!(gear_score(hero, &loadout), honest);
    }

    #[test]
    fn loot_goes_to_the_first_free_cell_until_the_bag_is_full() {
        let key = engine_game::character::loot_pool()[0].key;
        let mut loadout = Loadout::default();
        loadout.inventory.push(BagItem::new(1, key, 1));
        // Cell 0 is free, cell 1 is taken.
        assert_eq!(add_to_bag(&mut loadout, key), Ok(0));
        assert_eq!(add_to_bag(&mut loadout, key), Ok(2));
        assert_eq!(
            add_to_bag(&mut loadout, "no_such_item"),
            Err(BagError::UnknownItem)
        );
        while add_to_bag(&mut loadout, key).is_ok() {}
        assert_eq!(add_to_bag(&mut loadout, key), Err(BagError::Full));
        assert_eq!(loadout.inventory.len(), INVENTORY_SLOTS);
        // The legal sheet keeps every one of them.
        let sheet = sheet_of(Hero::Warrior, &loadout);
        assert_eq!(sheet.inventory().count(), INVENTORY_SLOTS);
    }

    #[test]
    fn mail_is_taken_into_the_first_free_cell_and_waits_when_the_bag_is_full() {
        let key = engine_game::character::loot_pool()[0].key;
        let mut loadout = Loadout::default();
        let first = add_to_mail(&mut loadout, key, MAIL_BAG_FULL, 100).expect("a real item");
        let second = add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 101).expect("a real item");
        assert_ne!(first, second);
        assert_eq!(
            add_to_mail(&mut loadout, "nope", MAIL_UNCLAIMED, 0),
            Err(BagError::UnknownItem)
        );
        assert!(mark_mail_read(&mut loadout, first));
        assert!(!mark_mail_read(&mut loadout, 99));
        assert_eq!(take_mail(&mut loadout, first), Ok(0));
        assert_eq!(take_mail(&mut loadout, first), Err(TakeMailError::NotFound));
        while add_to_bag(&mut loadout, key).is_ok() {}
        assert_eq!(take_mail(&mut loadout, second), Err(TakeMailError::BagFull));
        assert_eq!(loadout.mail.len(), 1, "full bag leaves the mail");
        // Old rows without a mailbox still load.
        let old: Loadout =
            serde_json::from_str(r#"{"equipment":[],"inventory":[]}"#).expect("loads");
        assert!(old.mail.is_empty());
    }

    #[test]
    fn mail_survives_the_trip_through_a_sheet() {
        let hero = Hero::Warrior;
        let mut loadout = starting_loadout(hero);
        let key = engine_game::character::loot_pool()[0].key;
        let first = add_to_mail(&mut loadout, key, MAIL_BAG_FULL, 7).expect("a real item");
        let second = add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 8).expect("a real item");
        let mut sheet = sheet_of(hero, &loadout);
        assert_eq!(sheet.mailbox().len(), 2);
        let back = loadout_of(&sheet);
        assert_eq!(back.mail.len(), 2);
        assert_eq!(back.mail[0].id, first);
        assert_eq!(back.mail[1].reason, MAIL_UNCLAIMED);
        // New letters never reuse an id.
        let fresh = sheet.mailbox_mut().send(
            engine_game::character::item_by_key(key).expect("real").id,
            MailSender::DungeonChest,
            MailReason::BagFull,
            9.0,
        );
        assert!(fresh > second);
    }

    #[test]
    fn moving_gear_keeps_the_mail() {
        let hero = Hero::Warrior;
        let mut loadout = kit_loadout();
        let key = engine_game::character::loot_pool()[0].key;
        add_to_mail(&mut loadout, key, MAIL_UNCLAIMED, 5).expect("a real item");
        let from = SlotRef::Equipment {
            slot: loadout.equipment[0].slot.clone(),
        };
        let moved = move_item(hero, &loadout, &from, &SlotRef::Inventory { index: 31 })
            .expect("fits the bag");
        assert_eq!(moved.mail, loadout.mail);
    }

    #[test]
    fn moves_are_validated() {
        let hero = Hero::FireMage;
        let loadout = kit_loadout();
        let head = SlotRef::Equipment {
            slot: "head".into(),
        };
        let cell = SlotRef::Inventory { index: 31 };
        // Something worn moves to an empty bag cell, and back.
        let worn = loadout.equipment[0].slot.clone();
        let from = SlotRef::Equipment { slot: worn };
        let moved = move_item(hero, &loadout, &from, &cell).expect("a worn item fits the bag");
        assert_eq!(moved.equipment.len(), loadout.equipment.len() - 1);
        // Nothing to move from an empty cell; nonsense addresses are refused.
        assert!(move_item(hero, &loadout, &SlotRef::Inventory { index: 30 }, &head).is_err());
        assert!(
            move_item(
                hero,
                &loadout,
                &SlotRef::Equipment {
                    slot: "tail".into()
                },
                &cell
            )
            .is_err()
        );
        assert!(move_item(hero, &loadout, &SlotRef::Inventory { index: 999 }, &cell).is_err());
    }

    /// A row exactly as it was stored before item properties existed.
    const OLD_ROW: &str = r#"{
        "equipment": [{"slot": "ring1", "item": "reinforced_iron_band"}],
        "inventory": [{"index": 3, "item": "head_haste_critical_strike", "quantity": 1}],
        "mail": [{"id": 1, "item": "head_haste_critical_strike", "sender": "Dungeon Chest",
                  "subject": "x", "reason": "bag_full", "received_at": 10, "read": false}],
        "resources": {"crypt_dust": 4}
    }"#;

    #[test]
    fn rows_stored_before_item_properties_still_load_as_legacy_items() {
        let old: Loadout = serde_json::from_str(OLD_ROW).expect("an old row deserializes");
        assert_eq!(old.equipment[0].props(), ItemProps::STANDARD);
        assert_eq!(old.inventory[0].props(), ItemProps::STANDARD);
        assert_eq!(old.mail[0].quality, 100);
        let sheet = sheet_of(Hero::Warrior, &old);
        let ring = sheet
            .equipment()
            .get(EquipSlot::Ring1)
            .expect("the ring is worn");
        assert_eq!(ring.props, ItemProps::STANDARD);
        assert!((ring.stats().get(engine_game::character::Stat::Stamina) - 10.0).abs() < 1e-5);
        assert_eq!(
            sheet.mailbox().letters()[0].props,
            ItemProps::STANDARD,
            "the letter lost its defaults"
        );
        // And writing it back changes nothing about it: defaults are not written at all.
        let back = serde_json::to_value(loadout_of(&sheet)).expect("serializes");
        assert!(back["equipment"][0].get("quality").is_none());
        assert!(back["inventory"][0].get("broken").is_none());
        assert_eq!(loadout_of(&sheet).equipment, old.equipment);
    }

    #[test]
    fn properties_survive_the_trip_through_a_sheet_and_through_json() {
        let hero = Hero::Warrior;
        let mut loadout = Loadout::default();
        let mut worn = EquippedItem::new("ring1", "reinforced_iron_band");
        (worn.quality, worn.sharpening) = (107, 6);
        loadout.equipment.push(worn);
        let mut carried = BagItem::new(2, engine_game::character::loot_pool()[0].key, 1);
        (carried.quality, carried.broken) = (93, true);
        loadout.inventory.push(carried);
        add_stack_to_mail(
            &mut loadout,
            "reinforced_iron_band",
            ItemProps::new(110, 2, false),
            MAIL_UNCLAIMED,
            9,
        )
        .expect("a real item");

        let json = serde_json::to_string(&loadout).expect("serializes");
        let read: Loadout = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(read, loadout);

        let sheet = sheet_of(hero, &read);
        assert_eq!(
            sheet.equipment().get(EquipSlot::Ring1).map(|s| s.props),
            Some(ItemProps::new(107, 6, false))
        );
        assert_eq!(
            sheet.inventory().get(2).map(|s| s.props),
            Some(ItemProps::new(93, 0, true))
        );
        assert_eq!(
            sheet.mailbox().letters()[0].props,
            ItemProps::new(110, 2, false)
        );
        assert_eq!(loadout_of(&sheet), read, "the stored form round trips");
        // Taking the mail keeps the props.
        let mut mailed = read;
        let cell = take_mail(&mut mailed, 1).expect("room");
        let taken = mailed
            .inventory
            .iter()
            .find(|entry| entry.index == cell)
            .expect("in the bag");
        assert_eq!(taken.props(), ItemProps::new(110, 2, false));
    }

    #[test]
    fn a_forged_row_cannot_exceed_the_legal_properties() {
        let hero = Hero::Warrior;
        let mut loadout = Loadout::default();
        let mut worn = EquippedItem::new("ring1", "reinforced_iron_band");
        (worn.quality, worn.sharpening) = (255, 250);
        loadout.equipment.push(worn);
        let props = sheet_of(hero, &loadout)
            .equipment()
            .get(EquipSlot::Ring1)
            .expect("worn")
            .props;
        assert_eq!(props.quality, engine_game::character::QUALITY_MAX);
        assert!(props.sharpening <= engine_game::character::SHARPENING_CEILING);
        // Item level 10 at the most the rules allow: 110 plus 20 per cent.
        assert!(gear_score(hero, &loadout) <= 130);
    }

    #[test]
    fn gear_score_follows_the_items_properties() {
        let hero = Hero::Warrior;
        let plain = |quality, sharpening, broken| {
            let mut loadout = Loadout::default();
            let mut worn = EquippedItem::new("ring1", "reinforced_iron_band");
            (worn.quality, worn.sharpening, worn.broken) = (quality, sharpening, broken);
            loadout.equipment.push(worn);
            gear_score(hero, &loadout)
        };
        // Item level 10, so 100 at default props.
        assert_eq!(plain(100, 0, false), 100);
        assert_eq!(plain(110, 0, false), 110);
        assert_eq!(plain(100, 10, false), 110);
        assert_eq!(plain(100, 0, true), 85);
        assert_eq!(plain(50, 0, false), 50);
    }

    #[test]
    fn catalog_items_are_wearable_and_carry_their_stats() {
        let hero = Hero::FireMage;
        let mut loadout = Loadout::default();
        loadout
            .equipment
            .push(EquippedItem::new("head", "crown_of_the_hollow_steward"));
        let sheet = sheet_of(hero, &loadout);
        let crown = sheet
            .equipment()
            .get(EquipSlot::Head)
            .expect("a catalog helm fits the head");
        assert!(crown.def().is_some_and(|def| !def.grants_nothing()));
        assert!(gear_score(hero, &loadout) >= 120);
        // A ring does not fit a head, even a catalog one.
        let mut wrong = Loadout::default();
        wrong
            .equipment
            .push(EquippedItem::new("head", "ashgraves_seal"));
        assert_eq!(gear_score(hero, &wrong), 0);
    }
}
