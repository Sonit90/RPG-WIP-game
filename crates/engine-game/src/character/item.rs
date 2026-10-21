//! What an item is, where it can go, and the one table every item is a row of.
//!
//! ```text
//! ItemFamily ──► which EquipSlots accept it
//!      ▲
//!      │
//! ItemDef  id · name · family · quality · item level · icon · StatBlock
//!      │
//!      ├─► ITEMS      the authored starting table. The interface never names a row of it.
//!      ├─► loot_pool  the 110 generated Uncommon pieces bosses drop
//!      ├─► catalog    items registered at runtime from the data catalog (`assets/catalog`)
//!      └─► ItemStack  an id, a quantity and the instance's [`ItemProps`]: what a slot holds
//! ```
//!
//! # Why the table is static and the slot holds an id
//!
//! An equipment slot and an inventory cell hold an [`ItemStack`] — an id, a quantity and three
//! small numbers of instance state ([`ItemProps`]) — rather than an item. Everything that
//! makes an item what it is lives in [`ITEMS`], read through [`item`]. So a saved character is
//! a list of ids, moving an item copies a handful of bytes, and correcting a typo in an item
//! name changes nothing anywhere else.
//!
//! # Instance properties
//!
//! Two copies of the same ring are not the same ring: each rolled its own quality (50 to 110
//! per cent of the base stats), may have been sharpened, and may have broken. [`ItemProps`]
//! carries that, and [`ItemProps::multiplier`] is the one function every stat read goes
//! through. Stacks only merge when their props are equal ([`ItemStack::can_merge`]).
//!
//! # Why a family, and not just a slot
//!
//! A ring is not a "Ring 1"; it is a ring, and there are two slots that take one. Trinkets
//! have two. Putting the restriction on the *item's* family and asking
//! [`ItemFamily::accepts`] means the equip rule is one comparison, and adding a second
//! back slot later would be a change to one match arm rather than to every item that could
//! go in it.

use std::sync::{LazyLock, OnceLock};

use super::stat::{Stat, StatBlock};
use crate::trinket::TrinketActive;

/// One of the thirteen places a character wears something.
///
/// The roster is Head, Shoulder, Chest, Back, Hands, Legs, Feet, Wrist, Necklace, two Rings
/// and two Trinkets. Saves from when there were four trinkets still name `trinket3` and
/// `trinket4`; [`EquipSlot::REMOVED_KEYS`] lists them so a loader can rehome those items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipSlot {
    /// Helm.
    Head,
    /// Shoulders.
    Shoulder,
    /// Chestpiece.
    Chest,
    /// Cloak.
    Back,
    /// Gloves.
    Hands,
    /// Leggings.
    Legs,
    /// Boots.
    Feet,
    /// Bracers.
    Wrist,
    /// Neck.
    Necklace,
    /// The first ring.
    Ring1,
    /// The second ring.
    Ring2,
    /// The first trinket.
    Trinket1,
    /// The second trinket.
    Trinket2,
}

impl EquipSlot {
    /// Every slot, in the order the sheet lists and saves them.
    pub const ALL: [Self; 13] = [
        Self::Head,
        Self::Shoulder,
        Self::Chest,
        Self::Back,
        Self::Hands,
        Self::Legs,
        Self::Feet,
        Self::Wrist,
        Self::Necklace,
        Self::Ring1,
        Self::Ring2,
        Self::Trinket1,
        Self::Trinket2,
    ];

    /// Saved keys of slots that no longer exist. An item found under one of these moves to the
    /// bag (or the mailbox) on load instead of being dropped.
    pub const REMOVED_KEYS: [&'static str; 2] = ["trinket3", "trinket4"];

    /// How many equipment slots a character has.
    pub const COUNT: usize = Self::ALL.len();

    /// Dense index into an [`super::equipment::Equipment`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The slot at `index`, if there is one.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// Uppercase display name. The UI font has no lowercase glyphs.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::Shoulder => "SHOULDER",
            Self::Chest => "CHEST",
            Self::Back => "BACK",
            Self::Hands => "HANDS",
            Self::Legs => "LEGS",
            Self::Feet => "FEET",
            Self::Wrist => "WRIST",
            Self::Necklace => "NECKLACE",
            Self::Ring1 => "RING 1",
            Self::Ring2 => "RING 2",
            Self::Trinket1 => "TRINKET 1",
            Self::Trinket2 => "TRINKET 2",
        }
    }

    /// The name a saved file uses. Stable, and deliberately not the display name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Shoulder => "shoulder",
            Self::Chest => "chest",
            Self::Back => "back",
            Self::Hands => "hands",
            Self::Legs => "legs",
            Self::Feet => "feet",
            Self::Wrist => "wrist",
            Self::Necklace => "necklace",
            Self::Ring1 => "ring1",
            Self::Ring2 => "ring2",
            Self::Trinket1 => "trinket1",
            Self::Trinket2 => "trinket2",
        }
    }

    /// The slot with this saved key, if it is one this build knows.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.key() == key)
    }

    /// Which family of item this slot takes.
    #[must_use]
    pub fn family(self) -> ItemFamily {
        match self {
            Self::Head => ItemFamily::Head,
            Self::Shoulder => ItemFamily::Shoulder,
            Self::Chest => ItemFamily::Chest,
            Self::Back => ItemFamily::Back,
            Self::Hands => ItemFamily::Hands,
            Self::Legs => ItemFamily::Legs,
            Self::Feet => ItemFamily::Feet,
            Self::Wrist => ItemFamily::Wrist,
            Self::Necklace => ItemFamily::Necklace,
            Self::Ring1 | Self::Ring2 => ItemFamily::Ring,
            Self::Trinket1 | Self::Trinket2 => ItemFamily::Trinket,
        }
    }
}

/// What kind of thing an item is, which is what decides where it may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemFamily {
    /// A helm. Goes on the head.
    Head,
    /// Shoulders.
    Shoulder,
    /// A chestpiece.
    Chest,
    /// A cloak.
    Back,
    /// Gloves.
    Hands,
    /// Leggings.
    Legs,
    /// Boots.
    Feet,
    /// Bracers.
    Wrist,
    /// An amulet.
    Necklace,
    /// A ring. Either ring slot takes one.
    Ring,
    /// A trinket. Either trinket slot takes one.
    Trinket,
}

impl ItemFamily {
    /// Every family, in slot order.
    pub const ALL: [Self; 11] = [
        Self::Head,
        Self::Shoulder,
        Self::Chest,
        Self::Back,
        Self::Hands,
        Self::Legs,
        Self::Feet,
        Self::Wrist,
        Self::Necklace,
        Self::Ring,
        Self::Trinket,
    ];

    /// Whether an item of this family may be worn in `slot`.
    ///
    /// **The equipment restriction, in one place.** Every equip, swap and drop asks this and
    /// nothing else, so there is no path by which a cloak reaches a ring finger.
    #[must_use]
    pub fn accepts(self, slot: EquipSlot) -> bool {
        slot.family() == self
    }

    /// Every slot this family may be worn in, in sheet order.
    pub fn slots(self) -> impl Iterator<Item = EquipSlot> {
        EquipSlot::ALL
            .into_iter()
            .filter(move |slot| self.accepts(*slot))
    }

    /// Uppercase display name, for a tooltip.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Head => "HEAD",
            Self::Shoulder => "SHOULDER",
            Self::Chest => "CHEST",
            Self::Back => "BACK",
            Self::Hands => "HANDS",
            Self::Legs => "LEGS",
            Self::Feet => "FEET",
            Self::Wrist => "WRIST",
            Self::Necklace => "NECK",
            Self::Ring => "FINGER",
            Self::Trinket => "TRINKET",
        }
    }
}

impl ItemFamily {
    /// The family authored under `key` (`"head"`, `"ring"` ...), if this build has one.
    ///
    /// How the data catalog names a family. The same fragments the generated loot keys use.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .zip(LOOT_FAMILY_KEYS)
            .find_map(|(family, name)| (name == key).then_some(family))
    }
}

/// How good an item is. Decides the colour its name is drawn in.
///
/// The colours themselves are the interface's, not gameplay's: this enum carries the rank
/// and the name, and `engine-presentation` maps a rank to a colour exactly once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemQuality {
    /// Grey. Vendor trash.
    Poor,
    /// White. What every character starts in.
    #[default]
    Common,
    /// Green.
    Uncommon,
    /// Blue.
    Rare,
    /// Purple.
    Epic,
    /// Orange.
    Legendary,
}

impl ItemQuality {
    /// Every quality, worst first.
    pub const ALL: [Self; 6] = [
        Self::Poor,
        Self::Common,
        Self::Uncommon,
        Self::Rare,
        Self::Epic,
        Self::Legendary,
    ];

    /// The quality authored under `key` (`"rare"`), if this build has one.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|quality| quality.name().eq_ignore_ascii_case(key))
    }

    /// Uppercase display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Poor => "POOR",
            Self::Common => "COMMON",
            Self::Uncommon => "UNCOMMON",
            Self::Rare => "RARE",
            Self::Epic => "EPIC",
            Self::Legendary => "LEGENDARY",
        }
    }
}

/// Which picture an item is drawn with.
///
/// Named here and resolved to the interface's own icon enum one layer up, for the reason
/// this crate exists: gameplay names the art it wants and never learns there is a texture.
/// One icon per family so far, because the starting set has one item per family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemIcon {
    /// A helm.
    Helm,
    /// Shoulders.
    Shoulders,
    /// A chestpiece.
    Chestpiece,
    /// A cloak.
    Cloak,
    /// Gloves.
    Gloves,
    /// Leggings.
    Leggings,
    /// Boots.
    Boots,
    /// Bracers.
    Bracers,
    /// An amulet.
    Amulet,
    /// A ring.
    Ring,
    /// A trinket.
    Trinket,
    /// A trinket with an active: the one that lessens the damage the party takes.
    TrinketWard,
    /// A trinket with an active: the one that strips effects in an area.
    TrinketDispel,
    /// A trinket with an active: the one that hastens the party's cooldowns.
    TrinketHaste,
    /// A trinket with an active: the one that puts up a shield.
    TrinketAbsorb,
}

impl ItemIcon {
    /// Every icon, in family order, then the trinkets that have a picture of their own.
    pub const ALL: [Self; 15] = [
        Self::Helm,
        Self::Shoulders,
        Self::Chestpiece,
        Self::Cloak,
        Self::Gloves,
        Self::Leggings,
        Self::Boots,
        Self::Bracers,
        Self::Amulet,
        Self::Ring,
        Self::Trinket,
        Self::TrinketWard,
        Self::TrinketDispel,
        Self::TrinketHaste,
        Self::TrinketAbsorb,
    ];

    /// The icon authored under `key`, which is the key of the family it belongs to
    /// (`"head"` is the helm), or one of the active trinkets' own names (`"trinket_ward"`).
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        ItemFamily::from_key(key)
            .map(Self::of_family)
            .or(match key {
                "trinket_ward" => Some(Self::TrinketWard),
                "trinket_dispel" => Some(Self::TrinketDispel),
                "trinket_haste" => Some(Self::TrinketHaste),
                "trinket_absorb" => Some(Self::TrinketAbsorb),
                _ => None,
            })
    }

    /// The icon a bare slot draws when nothing is in it.
    #[must_use]
    pub fn of_family(family: ItemFamily) -> Self {
        match family {
            ItemFamily::Head => Self::Helm,
            ItemFamily::Shoulder => Self::Shoulders,
            ItemFamily::Chest => Self::Chestpiece,
            ItemFamily::Back => Self::Cloak,
            ItemFamily::Hands => Self::Gloves,
            ItemFamily::Legs => Self::Leggings,
            ItemFamily::Feet => Self::Boots,
            ItemFamily::Wrist => Self::Bracers,
            ItemFamily::Necklace => Self::Amulet,
            ItemFamily::Ring => Self::Ring,
            ItemFamily::Trinket => Self::Trinket,
        }
    }
}

/// Which row of [`ITEMS`] an item is.
///
/// An identifier rather than an index, so the table can be reordered and a save written by
/// an older build still resolves. Zero is not a valid item; see [`ItemId::NONE`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub u16);

impl ItemId {
    /// The identifier no item has. What an empty slot would be if a slot were not an
    /// `Option` — kept so a file can say "nothing" as a number.
    pub const NONE: Self = Self(0);

    /// Whether this names an item at all.
    #[must_use]
    pub fn is_some(self) -> bool {
        self != Self::NONE
    }
}

/// The item level a starting white piece is.
///
/// One, so the number on a starting tooltip reads as the floor it is rather than as a value
/// somebody picked.
pub const STARTING_ITEM_LEVEL: u16 = 1;

/// One row of the item table: everything an item is.
///
/// Stats default to zero across the board, which is the whole starting set: the four white
/// pieces every character wears grant nothing, and the tooltip says so.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemDef {
    /// Its identifier. Unique across the table; a test asserts it.
    pub id: ItemId,
    /// Uppercase display name.
    pub name: &'static str,
    /// The name a saved file uses. Stable across renames of `name`.
    pub key: &'static str,
    /// What kind of thing it is, which is what decides where it may go.
    pub family: ItemFamily,
    /// How good it is.
    pub quality: ItemQuality,
    /// Its item level.
    pub item_level: u16,
    /// Which picture it draws with.
    pub icon: ItemIcon,
    /// What it grants. Added into the character's stats when it is worn, and nowhere else.
    pub stats: StatBlock,
    /// How many of it fit in one inventory cell. One for equipment; more for a consumable.
    pub max_stack: u16,
    /// The button it gives while worn, if it is a trinket with one. Quality and sharpening
    /// do not scale it: see [`crate::trinket`].
    pub active: Option<TrinketActive>,
    /// The equipment set this piece belongs to, by id (see [`super::sets`]). `None` for an item
    /// that is part of no set. Set bonuses are not scaled by quality or enhancement, so this is
    /// identity only.
    pub set: Option<&'static str>,
}

impl ItemDef {
    /// Whether this item grants no combat stats at all.
    #[must_use]
    pub fn grants_nothing(&self) -> bool {
        self.stats.is_empty()
    }

    /// Whether it may be worn in `slot`.
    #[must_use]
    pub fn fits(&self, slot: EquipSlot) -> bool {
        self.family.accepts(slot)
    }

    /// What a copy with `props` grants: the stats scaled by [`ItemProps::multiplier`].
    #[must_use]
    pub fn stats_for(&self, props: ItemProps) -> StatBlock {
        let mut stats = self.stats;
        let factor = props.multiplier();
        for stat in Stat::STORED {
            stats.scale(stat, factor);
        }
        stats
    }
}

/// A white piece of the given family: the starting-gear constructor.
///
/// Every field but the identity is the same for all of them, which is the point — a starting
/// item is defined by granting nothing, and writing seven explicit zeroes eleven times would
/// invite one of them to stop being zero.
const fn white(
    id: u16,
    name: &'static str,
    key: &'static str,
    family: ItemFamily,
    icon: ItemIcon,
) -> ItemDef {
    ItemDef {
        id: ItemId(id),
        name,
        key,
        family,
        quality: ItemQuality::Common,
        item_level: STARTING_ITEM_LEVEL,
        icon,
        stats: StatBlock::zero(),
        max_stack: 1,
        active: None,
        set: None,
    }
}

/// **The authored items**: the starting set and the odd one-off.
///
/// The interface never names a row of this: it is handed an [`ItemStack`] and looks the row
/// up. The generated boss-loot pieces are not here; they come from [`loot_pool`], and
/// [`item`] / [`item_by_key`] look in both.
///
/// Everything in it grants zero secondary stats, deliberately: a character in starting gear
/// has none, and the cheapest way to guarantee that is for there to be no authored item that
/// grants one. Secondary stats come only from the loot pool.
pub const ITEMS: [ItemDef; 12] = [
    white(
        1,
        "SIMPLE CLOTH CHESTPIECE",
        "simple_cloth_chestpiece",
        ItemFamily::Chest,
        ItemIcon::Chestpiece,
    ),
    white(
        2,
        "SIMPLE CLOTH LEGGINGS",
        "simple_cloth_leggings",
        ItemFamily::Legs,
        ItemIcon::Leggings,
    ),
    white(
        3,
        "SIMPLE CLOTH BRACERS",
        "simple_cloth_bracers",
        ItemFamily::Wrist,
        ItemIcon::Bracers,
    ),
    white(
        4,
        "SIMPLE CLOTH BOOTS",
        "simple_cloth_boots",
        ItemFamily::Feet,
        ItemIcon::Boots,
    ),
    white(
        5,
        "SIMPLE CLOTH HOOD",
        "simple_cloth_hood",
        ItemFamily::Head,
        ItemIcon::Helm,
    ),
    white(
        6,
        "SIMPLE CLOTH MANTLE",
        "simple_cloth_mantle",
        ItemFamily::Shoulder,
        ItemIcon::Shoulders,
    ),
    white(
        7,
        "SIMPLE CLOTH GLOVES",
        "simple_cloth_gloves",
        ItemFamily::Hands,
        ItemIcon::Gloves,
    ),
    white(
        8,
        "WORN TRAVELLERS CLOAK",
        "worn_travellers_cloak",
        ItemFamily::Back,
        ItemIcon::Cloak,
    ),
    white(
        9,
        "TARNISHED PENDANT",
        "tarnished_pendant",
        ItemFamily::Necklace,
        ItemIcon::Amulet,
    ),
    white(
        10,
        "PLAIN COPPER BAND",
        "plain_copper_band",
        ItemFamily::Ring,
        ItemIcon::Ring,
    ),
    white(
        11,
        "DULL GLASS FIGURINE",
        "dull_glass_figurine",
        ItemFamily::Trinket,
        ItemIcon::Trinket,
    ),
    // The one item in the table that grants anything, and the only one no character starts
    // with. It exists so that "equipping ten Stamina raises maximum health by a hundred" is
    // a fact the test suite can assert against the live simulation rather than a property of
    // arithmetic nothing has ever run. A Stamina ring rather than a crit ring, deliberately:
    // every secondary stat is specified to be zero everywhere, and this keeps that true.
    ItemDef {
        id: ItemId(12),
        name: "REINFORCED IRON BAND",
        key: "reinforced_iron_band",
        family: ItemFamily::Ring,
        quality: ItemQuality::Uncommon,
        item_level: 10,
        icon: ItemIcon::Ring,
        stats: StatBlock::of_const(Stat::Stamina, 10.0),
        max_stack: 1,
        active: None,
        set: None,
    },
];

/// The row `id` names, if this build has one.
///
/// `None` rather than a panic: an identifier out of a saved file is data, and data from
/// disk is allowed to be wrong. Everything downstream treats an unknown item as an empty
/// slot.
#[must_use]
pub fn item(id: ItemId) -> Option<&'static ItemDef> {
    if id.0 >= CATALOG_ID_BASE {
        // Registered at runtime, in file order: the row is the id's offset.
        return CATALOG
            .get()
            .and_then(|rows| rows.get(usize::from(id.0 - CATALOG_ID_BASE)));
    }
    if id.0 >= LOOT_ID_BASE {
        // The generated pool is laid out family-major with a fixed id stride, so the row is
        // arithmetic rather than a search. Checked against the row's own id, which also
        // rejects the unused ids in each stride.
        let offset = usize::from(id.0 - LOOT_ID_BASE);
        let row = offset / LOOT_ID_STRIDE * LOOT_PAIRS + offset % LOOT_ID_STRIDE;
        return loot_pool()
            .get(row)
            .filter(|def| def.id == id && offset % LOOT_ID_STRIDE < LOOT_PAIRS);
    }
    ITEMS.iter().find(|def| def.id == id)
}

/// The row saved under `key`, if this build has one.
///
/// Keys are what online persistence stores, so they are permanent - see [`loot_pool`].
#[must_use]
pub fn item_by_key(key: &str) -> Option<&'static ItemDef> {
    ITEMS
        .iter()
        .chain(loot_pool())
        .chain(CATALOG.get().into_iter().flatten())
        .find(|def| def.key == key)
}

/// The lowest quality an item rolls, in per cent of its base stats.
pub const QUALITY_MIN: u8 = 50;

/// The highest quality an item rolls, in per cent of its base stats. A jackpot.
pub const QUALITY_MAX: u8 = 110;

/// The quality of an item that never rolled one: the starting set, legacy rows and anything
/// stored before quality existed. Exactly its base stats.
pub const QUALITY_DEFAULT: u8 = 100;

/// What one sharpening level adds, in per cent of the base stats.
///
/// A constant rather than a catalog value because it is part of how a stack's stats are
/// *read*, and engine-game cannot read the catalog; the economy catalog states the same
/// number (`sharpening.step_pct`) and refuses to load if the two disagree.
pub const SHARPEN_STEP_PCT: u8 = 1;

/// The share of its stats an item loses when it breaks: a broken item grants
/// `quality x (1 - BROKEN_PENALTY)` per cent and loses its sharpening.
pub const BROKEN_PENALTY: f32 = 0.15;

/// The highest sharpening level a stored value is trusted to hold. The catalog's
/// `sharpening.max_level` is the real limit; this only bounds a hand-edited row.
pub const SHARPENING_CEILING: u8 = 20;

/// The sharpening level from which a copy is shown as highly sharpened, until the loot catalog's
/// `sharpening_drops.highlight_from` says otherwise (see [`set_sharpen_highlight_from`]).
pub const SHARPEN_HIGHLIGHT_DEFAULT: u8 = 5;

static SHARPEN_HIGHLIGHT_FROM: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(SHARPEN_HIGHLIGHT_DEFAULT);

/// Sets the level from which a copy counts as highly sharpened. Called once when the loot
/// catalog is loaded; the interface reads it through [`ItemProps::is_highly_sharpened`].
pub fn set_sharpen_highlight_from(level: u8) {
    SHARPEN_HIGHLIGHT_FROM.store(level, std::sync::atomic::Ordering::Relaxed);
}

/// The level from which a copy counts as highly sharpened.
#[must_use]
pub fn sharpen_highlight_from() -> u8 {
    SHARPEN_HIGHLIGHT_FROM.load(std::sync::atomic::Ordering::Relaxed)
}

/// What makes one copy of an item different from another.
///
/// **Naming.** The mechanic is *sharpening* (P1 called it enhancement). Persisted and wire names
/// stay stable so no database migration and no realm schema change is needed: stored and
/// wire structs keep the key `"enhancement"` (`#[serde(rename = "enhancement", alias =
/// "sharpening")]`), the `pending_loot` column and the realm table keep `enhancement`, and the
/// economy catalog accepts either `"sharpening"` or `"enhancement"`.
///
/// **One stat formula.** `multiplier = (quality + sharpening) / 100` ([`ItemProps::total_pct`]).
/// An epic is a different item *row* (own table stats), never a multiplier; quality and
/// sharpening add; the broken penalty is the only multiplicative factor and replaces the
/// sharpening (a broken item has none).
///
/// `Copy` and three bytes of payload, so it rides inside every [`ItemStack`] for free. The
/// default is the legacy item: quality 100, not sharpened, not broken, which is exactly the
/// item's base stats - so every item that predates this type is unchanged by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemProps {
    /// Rolled quality, `QUALITY_MIN..=QUALITY_MAX` per cent of the base stats.
    pub quality: u8,
    /// Sharpening level; each adds [`SHARPEN_STEP_PCT`] per cent. Zero while broken.
    pub sharpening: u8,
    /// Whether a failed sharpening broke it.
    pub broken: bool,
}

impl Default for ItemProps {
    fn default() -> Self {
        Self::STANDARD
    }
}

impl ItemProps {
    /// Quality 100, not sharpened, not broken: the item as the table states it.
    pub const STANDARD: Self = Self {
        quality: QUALITY_DEFAULT,
        sharpening: 0,
        broken: false,
    };

    /// Props from stored or rolled numbers, clamped into the legal ranges: a forged row can
    /// be wrong but cannot be stronger than the rules allow. A broken item has no sharpening.
    #[must_use]
    pub fn new(quality: u8, sharpening: u8, broken: bool) -> Self {
        Self {
            quality: quality.clamp(QUALITY_MIN, QUALITY_MAX),
            sharpening: if broken {
                0
            } else {
                sharpening.min(SHARPENING_CEILING)
            },
            broken,
        }
    }

    /// Props of a freshly rolled item: this quality, nothing else.
    #[must_use]
    pub fn rolled(quality: u8) -> Self {
        Self::new(quality, 0, false)
    }

    /// Whether these are the legacy defaults.
    #[must_use]
    pub fn is_standard(self) -> bool {
        self == Self::STANDARD
    }

    /// "Base Quality": the rolled quality, in per cent.
    #[must_use]
    pub fn base_quality_pct(self) -> f32 {
        f32::from(self.quality)
    }

    /// "Sharpening": what the sharpening levels add, in per cent. Zero when broken.
    #[must_use]
    pub fn sharpening_pct(self) -> f32 {
        if self.broken {
            0.0
        } else {
            f32::from(self.sharpening) * f32::from(SHARPEN_STEP_PCT)
        }
    }

    /// Whether the copy is sharpened far enough to be marked in the interface.
    #[must_use]
    pub fn is_highly_sharpened(self) -> bool {
        !self.broken && self.sharpening > 0 && self.sharpening >= sharpen_highlight_from()
    }

    /// "Effective Quality": the share of the base stats the item grants, in per cent.
    ///
    /// `quality + sharpening x step` normally; `quality x (1 - BROKEN_PENALTY)` once broken.
    #[must_use]
    pub fn total_pct(self) -> f32 {
        if self.broken {
            self.base_quality_pct() * (1.0 - BROKEN_PENALTY)
        } else {
            self.base_quality_pct() + self.sharpening_pct()
        }
    }

    /// **What the base stats are multiplied by.** The one function every stat read goes through.
    #[must_use]
    pub fn multiplier(self) -> f32 {
        self.total_pct() / 100.0
    }
}

/// What a slot holds: which item, how many, and this copy's [`ItemProps`].
///
/// `Copy`, so moving one between slots is an assignment. Everything else about the item is a
/// table lookup away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    /// Which item.
    pub item: ItemId,
    /// How many. At least one; a stack of zero is an empty slot, which is `None`.
    pub quantity: u16,
    /// Quality, sharpening and broken state of this copy.
    pub props: ItemProps,
}

impl ItemStack {
    /// One of `item`, with the default props.
    #[must_use]
    pub fn one(item: ItemId) -> Self {
        Self {
            item,
            quantity: 1,
            props: ItemProps::STANDARD,
        }
    }

    /// One of `item` with these props.
    #[must_use]
    pub fn with_props(item: ItemId, props: ItemProps) -> Self {
        Self {
            item,
            quantity: 1,
            props,
        }
    }

    /// `quantity` of `item`, clamped to at least one and to what the item stacks to.
    #[must_use]
    pub fn new(item: ItemId, quantity: u16) -> Self {
        let cap = def_of(item).map_or(1, |def| def.max_stack.max(1));
        Self {
            item,
            quantity: quantity.clamp(1, cap),
            props: ItemProps::STANDARD,
        }
    }

    /// The row this stack is of, if this build knows it.
    #[must_use]
    pub fn def(self) -> Option<&'static ItemDef> {
        item(self.item)
    }

    /// What this stack grants when it is worn: the row's stats scaled by
    /// [`ItemProps::multiplier`]. Nothing, if the item is unknown.
    #[must_use]
    pub fn stats(self) -> StatBlock {
        self.def()
            .map_or(StatBlock::zero(), |def| def.stats_for(self.props))
    }

    /// Whether it may be worn in `slot`. An unknown item fits nowhere.
    #[must_use]
    pub fn fits(self, slot: EquipSlot) -> bool {
        self.def().is_some_and(|def| def.fits(slot))
    }

    /// Whether `other` may join this stack: the same item with equal props.
    #[must_use]
    pub fn can_merge(self, other: Self) -> bool {
        self.item == other.item && self.props == other.props
    }

    /// The item level this copy counts for, scaled by its props like its stats are. What the
    /// gear score sums.
    #[must_use]
    pub fn effective_item_level(self) -> f32 {
        self.def().map_or(0.0, |def| {
            f32::from(def.item_level) * self.props.multiplier()
        })
    }
}

/// [`item`] under a name that a local binding called `item` does not shadow. Kept private.
fn def_of(id: ItemId) -> Option<&'static ItemDef> {
    item(id)
}

/// The starting white piece for a family, if the table has one.
///
/// A new hero starts with no items; this is for tests and tools that need a plain piece.
#[must_use]
pub fn starting_item(family: ItemFamily) -> Option<&'static ItemDef> {
    ITEMS
        .iter()
        .find(|def| def.family == family && def.quality == ItemQuality::Common)
}

/// Whether `stat` is one every authored ([`ITEMS`]) item leaves at zero.
///
/// Exists for the test that asserts the starting set grants no secondary stats; a caller in
/// gameplay would be asking the wrong question.
#[must_use]
pub fn no_item_grants(stat: Stat) -> bool {
    ITEMS.iter().all(|def| def.stats.get(stat) == 0.0)
}

/// The item level of every generated loot piece.
pub const LOOT_ITEM_LEVEL: u16 = 5;

/// Stat rating each of a loot piece's two stats grants.
pub const LOOT_STAT_RATING: f32 = 2.0;

/// First identifier of the generated pool. Authored items live below it.
const LOOT_ID_BASE: u16 = 1000;

/// Identifier stride between families: 16 slots per family, ten used.
///
/// A power of two so the ids read cleanly in hex dumps, with headroom for more pair
/// combinations without renumbering anything.
const LOOT_ID_STRIDE: usize = 16;

/// How many two-stat combinations a family has: C(5, 2).
const LOOT_PAIRS: usize = 10;

/// The five stats loot can roll, in the order that fixes pair indices. **Never reorder.**
const LOOT_STATS: [Stat; 5] = [
    Stat::Haste,
    Stat::CriticalStrike,
    Stat::MainStat,
    Stat::Spirit,
    Stat::Versatility,
];

/// Item-name suffix per pair, in pair-index order (lexicographic over [`LOOT_STATS`]).
/// Display only; the stable identity is the id and key.
const LOOT_SUFFIXES: [&str; LOOT_PAIRS] = [
    "THE TEMPEST",     // haste, critical strike
    "SWIFTNESS",       // haste, main stat
    "THE WINDWALKER",  // haste, spirit
    "THE GALE",        // haste, versatility
    "THE EXECUTIONER", // critical strike, main stat
    "THE ORACLE",      // critical strike, spirit
    "THE DUELIST",     // critical strike, versatility
    "THE SAGE",        // main stat, spirit
    "THE VANGUARD",    // main stat, versatility
    "THE WANDERER",    // spirit, versatility
];

/// Base noun per family, in [`ItemFamily::ALL`] order.
const LOOT_BASES: [&str; 11] = [
    "HELM", "MANTLE", "VEST", "CLOAK", "GLOVES", "LEGGINGS", "BOOTS", "BRACERS", "AMULET", "RING",
    "CHARM",
];

/// The key fragment per family, in [`ItemFamily::ALL`] order.
const LOOT_FAMILY_KEYS: [&str; 11] = [
    "head", "shoulder", "chest", "back", "hands", "legs", "feet", "wrist", "necklace", "ring",
    "trinket",
];

/// The generated pool, built once on first use.
///
/// Built at runtime rather than as a `const` because a key such as
/// `head_haste_critical_strike` is a concatenation, and `const` Rust cannot concatenate
/// non-literals. The strings are leaked so the rows can be `&'static str` like every authored
/// row: 110 short strings for the life of the process.
static LOOT: LazyLock<Vec<ItemDef>> = LazyLock::new(|| {
    let mut pool = Vec::with_capacity(ItemFamily::ALL.len() * LOOT_PAIRS);
    for (family_index, family) in ItemFamily::ALL.into_iter().enumerate() {
        let mut pair_index = 0;
        for (a, first) in LOOT_STATS.into_iter().enumerate() {
            for second in LOOT_STATS.into_iter().skip(a + 1) {
                let mut stats = StatBlock::zero();
                stats.set(first, LOOT_STAT_RATING);
                stats.set(second, LOOT_STAT_RATING);
                let name = format!(
                    "{} OF {}",
                    LOOT_BASES[family_index], LOOT_SUFFIXES[pair_index]
                );
                let key = format!(
                    "{}_{}_{}",
                    LOOT_FAMILY_KEYS[family_index],
                    first.key(),
                    second.key()
                );
                // 1000 + 11 * 16 fits u16 with a wide margin.
                let id = LOOT_ID_BASE + (family_index * LOOT_ID_STRIDE + pair_index) as u16;
                pool.push(ItemDef {
                    id: ItemId(id),
                    name: Box::leak(name.into_boxed_str()),
                    key: Box::leak(key.into_boxed_str()),
                    family,
                    quality: ItemQuality::Uncommon,
                    item_level: LOOT_ITEM_LEVEL,
                    icon: ItemIcon::of_family(family),
                    stats,
                    max_stack: 1,
                    active: None,
                    set: None,
                });
                pair_index += 1;
            }
        }
    }
    pool
});

/// **The boss loot pool**: every family times every pair of
/// {Haste, Critical Strike, Main Stat, Spirit, Versatility}, +2 of each - 110 Uncommon items.
///
/// Ids are `1000 + family_index * 16 + pair_index` and keys are
/// `<family>_<stat_a>_<stat_b>`; **both are permanent**, because online persistence stores
/// them. Rows are ordered family-major, then by pair index. No item here has its own model:
/// wearing one changes stats only.
#[must_use]
pub fn loot_pool() -> &'static [ItemDef] {
    &LOOT
}

/// First identifier of the catalog items. Authored items and the generated pool live below it.
pub const CATALOG_ID_BASE: u16 = 2000;

/// One item of the data catalog, as plain owned data: what `engine-realm` reads out of
/// `assets/catalog/loot.json` and hands to [`register_catalog_items`]. This crate keeps no
/// serde, so the file format is the caller's.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogItem {
    /// Its stable key. Permanent: persistence stores it.
    pub key: String,
    /// Display name; uppercased on registration like every other row.
    pub name: String,
    /// What kind of thing it is.
    pub family: ItemFamily,
    /// How good it is.
    pub quality: ItemQuality,
    /// Its item level.
    pub item_level: u16,
    /// Which picture it draws with.
    pub icon: ItemIcon,
    /// Its base stats, at quality 100.
    pub stats: Vec<(Stat, f32)>,
    /// The button it gives while worn, for a trinket that has one.
    pub active: Option<TrinketActive>,
    /// The id of the equipment set it belongs to, if any (`sets.json`).
    pub set: Option<String>,
}

/// The registered catalog rows, in registration order. Set once.
static CATALOG: OnceLock<Vec<ItemDef>> = OnceLock::new();

/// **Registers the data catalog's items**, so [`item`] and [`item_by_key`] find them.
///
/// Ids are assigned `CATALOG_ID_BASE + position`, in the order given, which is file order:
/// the client, the server and the API all read the same file, so they agree on every id with
/// no coordination. Strings are leaked to `'static` like the generated pool's, once.
///
/// Idempotent: the first call wins and returns `true`; any later call changes nothing and
/// returns `false` (logging a warning if it was given a different list, which means two
/// catalogs disagree inside one process). Rows past what an id can number are dropped.
pub fn register_catalog_items(items: Vec<CatalogItem>) -> bool {
    let capacity = usize::from(u16::MAX - CATALOG_ID_BASE);
    let mut installed = false;
    let rows = CATALOG.get_or_init(|| {
        installed = true;
        items
            .iter()
            .take(capacity)
            .enumerate()
            .map(|(index, row)| {
                let mut stats = StatBlock::zero();
                for (stat, value) in &row.stats {
                    stats.add(*stat, *value);
                }
                ItemDef {
                    // Bounded by `capacity` above.
                    id: ItemId(CATALOG_ID_BASE + index as u16),
                    name: Box::leak(row.name.to_uppercase().into_boxed_str()),
                    key: Box::leak(row.key.clone().into_boxed_str()),
                    family: row.family,
                    quality: row.quality,
                    item_level: row.item_level,
                    icon: row.icon,
                    stats,
                    max_stack: 1,
                    active: row.active,
                    set: row
                        .set
                        .as_deref()
                        .map(|id| &*Box::leak(id.to_owned().into_boxed_str())),
                }
            })
            .collect()
    });
    if !installed
        && (rows.len() != items.len().min(capacity)
            || rows.iter().zip(&items).any(|(def, row)| def.key != row.key))
    {
        log::warn!("catalog items registered twice with different contents; keeping the first");
    }
    installed
}

/// The registered catalog rows, in file order. Empty until [`register_catalog_items`] runs.
#[must_use]
pub fn catalog_items() -> &'static [ItemDef] {
    CATALOG.get().map_or(&[], Vec::as_slice)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::character::stat::StatGroup;

    #[test]
    fn there_are_thirteen_slots_and_each_has_a_distinct_key() {
        assert_eq!(EquipSlot::COUNT, 13);
        for (index, slot) in EquipSlot::ALL.into_iter().enumerate() {
            assert_eq!(slot.index(), index);
            assert_eq!(EquipSlot::from_index(index), Some(slot));
            assert_eq!(EquipSlot::from_key(slot.key()), Some(slot));
        }
    }

    #[test]
    fn the_slot_roster_is_the_one_that_was_asked_for() {
        use EquipSlot::{
            Back, Chest, Feet, Hands, Head, Legs, Necklace, Ring1, Ring2, Shoulder, Trinket1,
            Trinket2, Wrist,
        };
        assert_eq!(
            EquipSlot::ALL,
            [
                Head, Shoulder, Chest, Back, Hands, Legs, Feet, Wrist, Necklace, Ring1, Ring2,
                Trinket1, Trinket2,
            ]
        );
    }

    #[test]
    fn a_family_accepts_exactly_the_slots_that_want_it() {
        assert_eq!(
            ItemFamily::Ring.slots().collect::<Vec<_>>(),
            vec![EquipSlot::Ring1, EquipSlot::Ring2]
        );
        assert_eq!(ItemFamily::Trinket.slots().count(), 2);
        assert_eq!(ItemFamily::Chest.slots().count(), 1);

        assert!(ItemFamily::Ring.accepts(EquipSlot::Ring2));
        assert!(!ItemFamily::Ring.accepts(EquipSlot::Necklace));
        assert!(!ItemFamily::Back.accepts(EquipSlot::Chest));
    }

    #[test]
    fn every_family_has_a_slot_and_every_slot_has_a_family() {
        for family in ItemFamily::ALL {
            assert!(family.slots().count() > 0, "{family:?} fits nowhere");
        }
        for slot in EquipSlot::ALL {
            assert!(slot.family().accepts(slot));
        }
    }

    fn everything() -> impl Iterator<Item = &'static ItemDef> {
        ITEMS.iter().chain(loot_pool())
    }

    #[test]
    fn identifiers_names_and_keys_are_unique_and_none_is_zero() {
        for def in everything() {
            assert!(def.id.is_some(), "{} has the reserved identifier", def.name);
            assert_eq!(item(def.id), Some(def));
            assert_eq!(item_by_key(def.key), Some(def));
            assert_eq!(everything().filter(|other| other.id == def.id).count(), 1);
            assert_eq!(everything().filter(|other| other.key == def.key).count(), 1);
            assert_eq!(
                everything().filter(|other| other.name == def.name).count(),
                1
            );
        }
        assert_eq!(item(ItemId::NONE), None);
        // The unused ids in a family's stride, and ids past the pool, resolve to nothing.
        assert_eq!(item(ItemId(LOOT_ID_BASE + 10)), None);
        assert_eq!(item(ItemId(LOOT_ID_BASE + 15)), None);
        assert_eq!(item(ItemId(LOOT_ID_BASE + 11 * 16)), None);
        assert_eq!(item(ItemId(999)), None);
    }

    #[test]
    fn the_loot_pool_is_every_family_times_every_pair_of_five_stats() {
        let pool = loot_pool();
        assert_eq!(pool.len(), 110);
        for family in ItemFamily::ALL {
            let of_family: Vec<_> = pool.iter().filter(|def| def.family == family).collect();
            assert_eq!(of_family.len(), 10, "{family:?}");
            // Ten distinct stat pairs, each +2 of exactly two of the five stats.
            let mut pairs = Vec::new();
            for def in &of_family {
                assert_eq!(def.quality, ItemQuality::Uncommon);
                assert_eq!(def.item_level, LOOT_ITEM_LEVEL);
                assert_eq!(def.icon, ItemIcon::of_family(family));
                assert_eq!(def.max_stack, 1);
                let entries: Vec<_> = def.stats.entries().collect();
                assert_eq!(entries.len(), 2, "{}", def.name);
                assert!(
                    entries
                        .iter()
                        .all(|(stat, value)| { *value == 2.0 && LOOT_STATS.contains(stat) })
                );
                pairs.push((entries[0].0, entries[1].0));
            }
            pairs.sort_by_key(|(a, b)| (a.index(), b.index()));
            pairs.dedup();
            assert_eq!(pairs.len(), 10, "{family:?} repeats a pair");
        }
    }

    #[test]
    fn loot_ids_and_keys_are_the_documented_permanent_ones() {
        let helm = item(ItemId(1000)).expect("first loot row");
        assert_eq!(helm.key, "head_haste_critical_strike");
        assert_eq!(helm.name, "HELM OF THE TEMPEST");
        assert_eq!(item_by_key("head_haste_critical_strike"), Some(helm));
        let last = item(ItemId(1000 + 10 * 16 + 9)).expect("last loot row");
        assert_eq!(last.key, "trinket_spirit_versatility");
        assert_eq!(last.family, ItemFamily::Trinket);
        // Wearable by its family's slots.
        assert!(helm.fits(EquipSlot::Head));
        assert!(!helm.fits(EquipSlot::Chest));
    }

    #[test]
    fn no_authored_item_grants_a_secondary_stat() {
        for stat in Stat::ALL {
            if stat.group() == StatGroup::Secondary {
                assert!(
                    no_item_grants(stat),
                    "{} is granted by an item",
                    stat.name()
                );
            }
        }
    }

    #[test]
    fn the_starting_set_grants_nothing_at_all() {
        for family in ItemFamily::ALL {
            let def = starting_item(family).expect("a white piece per family");
            assert!(def.grants_nothing(), "{} grants stats", def.name);
            assert_eq!(def.quality, ItemQuality::Common);
            assert_eq!(def.item_level, STARTING_ITEM_LEVEL);
        }
    }

    #[test]
    fn the_one_item_that_grants_anything_grants_a_primary_stat_and_is_not_starting_gear() {
        let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
        assert_eq!(ring.stats.get(Stat::Stamina), 10.0);
        assert_ne!(ring.quality, ItemQuality::Common, "it is not a white piece");
        assert_ne!(
            starting_item(ItemFamily::Ring).map(|def| def.id),
            Some(ring.id),
            "the stamina ring became starting gear"
        );
    }

    #[test]
    fn every_family_has_a_starting_piece_with_the_right_icon() {
        for family in ItemFamily::ALL {
            let def = starting_item(family).expect("a white piece per family");
            assert_eq!(def.family, family);
            assert_eq!(def.icon, ItemIcon::of_family(family));
        }
    }

    #[test]
    fn a_stack_knows_what_it_is_and_where_it_fits() {
        let ring = starting_item(ItemFamily::Ring).expect("a ring");
        let stack = ItemStack::one(ring.id);
        assert_eq!(stack.def(), Some(ring));
        assert!(stack.fits(EquipSlot::Ring1));
        assert!(!stack.fits(EquipSlot::Head));
        assert!(stack.stats().is_empty());

        // An item this build does not have fits nowhere and grants nothing.
        let unknown = ItemStack::one(ItemId(9999));
        assert_eq!(unknown.def(), None);
        assert!(EquipSlot::ALL.into_iter().all(|slot| !unknown.fits(slot)));
    }

    #[test]
    fn a_quantity_is_clamped_to_what_the_item_stacks_to() {
        let ring = starting_item(ItemFamily::Ring).expect("a ring");
        assert_eq!(ItemStack::new(ring.id, 0).quantity, 1);
        assert_eq!(ItemStack::new(ring.id, 40).quantity, ring.max_stack);
    }

    #[test]
    fn legacy_props_are_the_items_own_stats() {
        let props = ItemProps::default();
        assert_eq!(props, ItemProps::STANDARD);
        assert!(props.is_standard());
        assert_eq!(props.multiplier(), 1.0);
        let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
        let stack = ItemStack::one(ring.id);
        assert_eq!(stack.props, ItemProps::STANDARD);
        assert_eq!(stack.stats().get(Stat::Stamina), 10.0);
    }

    #[test]
    fn quality_and_sharpening_scale_stats_by_the_one_formula() {
        let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
        let stamina = |props| {
            ItemStack::with_props(ring.id, props)
                .stats()
                .get(Stat::Stamina)
        };
        assert!((stamina(ItemProps::rolled(106)) - 10.6).abs() < 1e-4);
        assert!((stamina(ItemProps::rolled(50)) - 5.0).abs() < 1e-4);
        // +1 per cent per level, exactly: a +7 on a 106 is 113 per cent.
        let props = ItemProps::new(106, 7, false);
        assert_eq!(props.base_quality_pct(), 106.0);
        assert_eq!(props.sharpening_pct(), 7.0);
        assert_eq!(props.total_pct(), 113.0);
        assert!((stamina(props) - 11.3).abs() < 1e-4);
        // One level more adds exactly one per cent of the base stats.
        let step = stamina(ItemProps::new(100, 1, false)) - stamina(ItemProps::new(100, 0, false));
        assert!((step - 0.1).abs() < 1e-5);
    }

    #[test]
    fn a_broken_item_grants_eighty_five_per_cent_of_its_quality_and_no_sharpening() {
        let broken = ItemProps::new(100, 9, true);
        assert_eq!(broken.sharpening, 0, "breaking drops the sharpening");
        assert!((broken.total_pct() - 85.0).abs() < 1e-4);
        assert_eq!(broken.sharpening_pct(), 0.0);
        let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
        let stamina = ItemStack::with_props(ring.id, broken)
            .stats()
            .get(Stat::Stamina);
        assert!((stamina - 8.5).abs() < 1e-4);
        // Quality counts: a 110 that broke is 93.5 per cent.
        assert!((ItemProps::new(110, 0, true).total_pct() - 93.5).abs() < 1e-4);
    }

    #[test]
    fn props_are_clamped_into_the_legal_ranges() {
        assert_eq!(ItemProps::new(0, 0, false).quality, QUALITY_MIN);
        assert_eq!(ItemProps::new(255, 0, false).quality, QUALITY_MAX);
        assert_eq!(
            ItemProps::new(100, 200, false).sharpening,
            SHARPENING_CEILING
        );
    }

    #[test]
    fn stacks_only_merge_with_equal_props() {
        let ring = item_by_key("reinforced_iron_band").expect("the stamina ring");
        let plain = ItemStack::one(ring.id);
        assert!(plain.can_merge(ItemStack::one(ring.id)));
        assert!(!plain.can_merge(ItemStack::with_props(ring.id, ItemProps::rolled(101))));
        assert!(!plain.can_merge(ItemStack::with_props(ring.id, ItemProps::new(100, 0, true))));
        assert!(!plain.can_merge(ItemStack::one(ItemId(1))));
    }

    #[test]
    fn registered_catalog_items_get_deterministic_ids_after_the_built_in_tables() {
        let ids = crate::loot::tests::test_items();
        for (offset, id) in ids.into_iter().enumerate() {
            assert_eq!(
                id.0,
                CATALOG_ID_BASE + offset as u16,
                "ids follow file order"
            );
            let def = item(id).expect("registered");
            assert_eq!(item_by_key(def.key), Some(def));
            assert_eq!(def.name, def.name.to_uppercase(), "names are uppercase");
        }
        assert_eq!(catalog_items().len(), 4);
        // Registering again changes nothing, even with different contents.
        assert!(!register_catalog_items(Vec::new()));
        assert_eq!(catalog_items().len(), 4);
        // Built-in ids and keys are untouched.
        const { assert!(CATALOG_ID_BASE > LOOT_ID_BASE + 11 * 16) };
        assert_eq!(
            item(ItemId(1)).map(|def| def.key),
            Some("simple_cloth_chestpiece")
        );
        assert_eq!(item(ItemId(CATALOG_ID_BASE + 100)), None);
    }

    #[test]
    fn data_keys_resolve_to_families_qualities_and_icons() {
        assert_eq!(ItemFamily::from_key("head"), Some(ItemFamily::Head));
        assert_eq!(ItemFamily::from_key("trinket"), Some(ItemFamily::Trinket));
        assert_eq!(ItemFamily::from_key("sword"), None);
        assert_eq!(ItemQuality::from_key("epic"), Some(ItemQuality::Epic));
        assert_eq!(ItemQuality::from_key("RARE"), Some(ItemQuality::Rare));
        assert_eq!(ItemIcon::from_key("ring"), Some(ItemIcon::Ring));
        assert_eq!(
            Stat::from_key("critical_strike"),
            Some(Stat::CriticalStrike)
        );
        assert_eq!(Stat::from_key("nope"), None);
        for stat in Stat::STORED {
            assert_eq!(Stat::from_key(stat.key()), Some(stat));
        }
    }
}
