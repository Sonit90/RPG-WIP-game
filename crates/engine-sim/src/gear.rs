//! Gear: what a character is wearing when a fight is simulated.
//!
//! ```text
//! GearProfile --loadout(hero)--> Vec<(EquipSlot, item key, ItemProps)>
//!     --sheet_of_items--> engine_realm::loadout::sheet_of --> CharacterSheet
//!     --Encounter::new--> Roster sheet + Simulation::refresh_character
//! ```
//!
//! Gear is **not** a stat block made up for the simulator. A profile names real items from
//! `assets/catalog/loot.json` and the [`ItemProps`] (quality, enhancement) each is worn at,
//! and the stats are whatever the game computes from that: [`engine_realm::loadout::sheet_of`]
//! builds the sheet exactly as a realm fight does, `CharacterSheet::gear_stats` sums the
//! worn stacks scaled by [`ItemProps::multiplier`], and
//! [`engine_game::sim::Simulation::refresh_character`] pushes the result through the rating
//! curve and the main-stat bridge. Anything P2 adds to that path - sharpening, gems, sets,
//! books - therefore reaches the simulator without a second implementation of it.
//!
//! # The tiers
//!
//! | tier     | items                                    | quality | enhancement |
//! |----------|------------------------------------------|---------|-------------|
//! | `none`   | nothing worn: base stats only            | -       | -           |
//! | `low`    | best *rare* piece in every slot          | 70      | +0          |
//! | `medium` | epic in every other slot, rare elsewhere | 85      | +3          |
//! | `high`   | best *epic* in every slot that has one   | 100     | +6          |
//! | `p2_max` | the same pieces as `high`                | 110     | +10         |
//! | `p2_set` | `p2_max` with the full major set on      | 110     | +10         |
//!
//! `high` is the best the game currently lets a character reach short of a jackpot roll;
//! `p2_max` is the ceiling P2 is designed against (maximum quality, a heavily sharpened set).
//! A slot with no epic in the catalog falls back to its best rare, so every tier above `none`
//! fills all thirteen slots. `top` is accepted on the command line as an alias of `high`.
//!
//! # Choosing the items
//!
//! Best-in-slot is decided per hero by [`item_score`], a fixed weighting of the item's own
//! stats (main stat first, then Haste and Critical Strike, Versatility a distant third), with
//! ties broken by catalog order. There is no randomness, so a tier is the same set of items
//! on every run. Two-of-a-kind slots (rings, trinkets) take two *different* items while the
//! catalog has them, so the second ring is a rare when only one ring is epic.
//!
//! # Extension points
//!
//! * [`Loadout`] - a plain `Vec<(EquipSlot, key, ItemProps)>` - is the interface: build one
//!   by hand, or take a profile's and edit it, and [`sheet_of_items`] turns it into a sheet.
//! * [`Worn`] is what an [`crate::scenario::Encounter`] is dressed in: a sheet, or a bare
//!   stat block for the talent studies that state their gear as percentages.
//! * Gems, sets and books are further inputs to the same sheet or to the talent grid; they
//!   plug in beside [`sheet_of_items`], not into this module's tier table.

use std::sync::OnceLock;

use engine_game::character::{
    CharacterSheet, EquipSlot, ItemDef, ItemProps, ItemQuality, QUALITY_MAX, QUALITY_MIN, SetDef,
    Stat, StatBlock, item_by_key, registered_sets,
};
use engine_game::hero::Hero;
use engine_realm::loadout::{EquippedItem, Loadout as StoredLoadout, sheet_of};
use engine_realm::loot::{LootCatalog, LootItem, ensure_items_registered};

/// What a loadout is made of: slot, item key (`assets/catalog/loot.json`'s `id`) and props.
pub type Loadout = Vec<(EquipSlot, String, ItemProps)>;

/// The loot catalog, compiled in the way the realm compiles it.
const LOOT_JSON: &str = include_str!("../../../assets/catalog/loot.json");

/// What an encounter's player is dressed in.
#[derive(Debug, Clone, PartialEq)]
pub enum Worn {
    /// A character sheet, set on the roster the way a realm fight sets it.
    Sheet(Box<CharacterSheet>),
    /// A stat block granted on top of a bare sheet, for studies that state gear as rating.
    Bonus(StatBlock),
}

impl From<CharacterSheet> for Worn {
    fn from(sheet: CharacterSheet) -> Self {
        Self::Sheet(Box::new(sheet))
    }
}

impl From<StatBlock> for Worn {
    fn from(stats: StatBlock) -> Self {
        Self::Bonus(stats)
    }
}

/// Which rung of the progression a profile stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GearTier {
    /// Nothing equipped.
    None,
    /// Rare pieces at low quality.
    Low,
    /// A rare and epic mix at good quality, lightly enhanced.
    Medium,
    /// Epic pieces at catalog quality, enhanced to +6.
    High,
    /// Epic pieces at the quality ceiling, enhanced to +10: what P2 is designed against.
    P2Max,
    /// `P2Max` with the class's full major set swapped in: what a set bonus is worth.
    P2Set,
}

impl GearTier {
    /// Every tier, lowest first.
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::P2Max,
        Self::P2Set,
    ];

    /// The written name the command line takes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::P2Max => "p2_max",
            Self::P2Set => "p2_set",
        }
    }

    /// Parses a tier name. `top` is the old name of `high`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text == "top" {
            return Some(Self::High);
        }
        Self::ALL.into_iter().find(|tier| tier.as_str() == text)
    }
}

/// Which catalog pieces a tier wears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Nothing.
    Nothing,
    /// The best rare piece in every slot.
    Rare,
    /// An epic in every other slot (counting from the first), the best rare elsewhere.
    Mixed,
    /// The best epic piece in every slot that has one, the best rare elsewhere.
    Epic,
    /// [`Pick::Epic`], with the hero's full major set worn over the slots it covers.
    MajorSet,
}

/// One way of dressing a character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GearProfile {
    /// Which rung it stands for.
    pub tier: GearTier,
    /// The name, for the report and the command line. The tier's own.
    pub variant: &'static str,
    /// Which pieces are worn.
    pub pick: Pick,
    /// The quality every piece is worn at, per cent of its base stats.
    pub quality: u8,
    /// The enhancement level of every piece.
    pub enhancement: u8,
}

/// **Every gear profile the simulator knows, in one table.** Adding one is adding a row.
pub const PROFILES: [GearProfile; 6] = [
    GearProfile {
        tier: GearTier::None,
        variant: "none",
        pick: Pick::Nothing,
        quality: 100,
        enhancement: 0,
    },
    GearProfile {
        tier: GearTier::Low,
        variant: "low",
        pick: Pick::Rare,
        quality: 70,
        enhancement: 0,
    },
    GearProfile {
        tier: GearTier::Medium,
        variant: "medium",
        pick: Pick::Mixed,
        quality: 85,
        enhancement: 3,
    },
    GearProfile {
        tier: GearTier::High,
        variant: "high",
        pick: Pick::Epic,
        quality: 100,
        enhancement: 6,
    },
    GearProfile {
        tier: GearTier::P2Max,
        variant: "p2_max",
        pick: Pick::Epic,
        quality: QUALITY_MAX,
        enhancement: 10,
    },
    GearProfile {
        tier: GearTier::P2Set,
        variant: "p2_set",
        pick: Pick::MajorSet,
        quality: QUALITY_MAX,
        enhancement: 10,
    },
];

impl GearProfile {
    /// The profiles that belong to the tiers a command line asked for, in table order.
    #[must_use]
    pub fn select(tiers: &[GearTier]) -> Vec<GearProfile> {
        PROFILES
            .iter()
            .copied()
            .filter(|profile| tiers.contains(&profile.tier))
            .collect()
    }

    /// The profile the headline balance numbers use for a tier: its only one.
    #[must_use]
    pub fn headline(tier: GearTier) -> GearProfile {
        PROFILES
            .iter()
            .copied()
            .find(|profile| profile.tier == tier)
            .unwrap_or(PROFILES[0])
    }

    /// The props every piece of this profile is worn at.
    #[must_use]
    pub fn props(&self) -> ItemProps {
        ItemProps::new(
            self.quality.clamp(QUALITY_MIN, QUALITY_MAX),
            self.enhancement,
            false,
        )
    }

    /// The items `hero` wears in this profile, slot by slot.
    #[must_use]
    pub fn loadout(&self, hero: Hero) -> Loadout {
        let props = self.props();
        let mut taken: Vec<&'static str> = Vec::new();
        let mut loadout = Loadout::new();
        for (index, slot) in EquipSlot::ALL.into_iter().enumerate() {
            let epic = match self.pick {
                Pick::Nothing => break,
                Pick::Rare => false,
                Pick::Mixed => index % 2 == 0,
                Pick::Epic | Pick::MajorSet => true,
            };
            let Some(def) = best_in_slot(hero, slot, epic, &taken) else {
                continue;
            };
            taken.push(def.key);
            loadout.push((slot, def.key.to_owned(), props));
        }
        if self.pick == Pick::MajorSet
            && let Some(set) = major_set_for(hero)
        {
            // The set's pieces replace whatever was worn in the slots they fit, one slot per
            // piece, so the measured change is the set: its stats and its bonuses.
            let mut used: Vec<EquipSlot> = Vec::new();
            for key in &set.pieces {
                let Some(def) = item_by_key(key) else {
                    continue;
                };
                let Some(slot) = def.family.slots().find(|slot| !used.contains(slot)) else {
                    continue;
                };
                used.push(slot);
                let entry = (slot, def.key.to_owned(), props);
                match loadout.iter_mut().find(|(worn, _, _)| *worn == slot) {
                    Some(existing) => *existing = entry,
                    None => loadout.push(entry),
                }
            }
        }
        loadout
    }

    /// The character sheet `hero` wears this profile on.
    #[must_use]
    pub fn sheet(&self, hero: Hero) -> CharacterSheet {
        sheet_of_items(hero, &self.loadout(hero))
    }

    /// What an encounter is dressed in for `hero`.
    #[must_use]
    pub fn worn(&self, hero: Hero) -> Worn {
        self.sheet(hero).into()
    }

    /// A short label for a table: `none`, `high`, ...
    #[must_use]
    pub fn label(&self) -> String {
        self.variant.to_owned()
    }

    /// The profile in words, for the report's gear table: piece count, props and the ratings
    /// the game computes from them.
    #[must_use]
    pub fn describe(&self, hero: Hero) -> String {
        let sheet = self.sheet(hero);
        let stats = sheet.gear_stats();
        let worn = sheet.equipment().count();
        if worn == 0 {
            return "nothing worn".to_owned();
        }
        format!(
            "{worn} pieces at quality {} +{}: +{:.1} {}, {:.1} Haste ({:.1}%), {:.1} Crit ({:.1}%), {:.1} Stamina",
            self.quality,
            self.enhancement,
            stats.get(hero.main_stat()),
            hero.main_stat().name(),
            stats.get(Stat::Haste),
            stats.percent(Stat::Haste) * 100.0,
            stats.get(Stat::CriticalStrike),
            stats.percent(Stat::CriticalStrike) * 100.0,
            stats.get(Stat::Stamina),
        )
    }
}

/// **A character sheet from a hand-made loadout**, through the realm's own `sheet_of`.
///
/// Entries the game cannot honour (an unknown key, an item in the wrong slot) are dropped and
/// logged by the realm, as they are for a saved character. This is the door gems, sets and
/// books are meant to be added to: change the loadout or the sheet, never the encounter.
#[must_use]
pub fn sheet_of_items(hero: Hero, loadout: &[(EquipSlot, String, ItemProps)]) -> CharacterSheet {
    let stored = StoredLoadout {
        equipment: loadout
            .iter()
            .map(|(slot, key, props)| EquippedItem {
                slot: slot.key().to_owned(),
                item: key.clone(),
                quality: props.quality,
                enhancement: props.enhancement,
                broken: props.broken,
            })
            .collect(),
        ..StoredLoadout::default()
    };
    sheet_of(hero, &stored)
}

/// The major set `hero` plays: the registered set of four or more pieces with a bonus that
/// changes one of the hero's own abilities. `None` if the catalog has none.
#[must_use]
pub fn major_set_for(hero: Hero) -> Option<&'static SetDef> {
    ensure_items_registered();
    registered_sets().iter().find(|set| {
        set.pieces.len() >= 4
            && set
                .bonuses
                .iter()
                .flat_map(|bonus| &bonus.effects)
                .filter_map(|effect| effect.ability())
                .any(|ability| ability.hero() == hero)
    })
}

/// The catalog's rows, in file order.
fn catalog_rows() -> &'static [LootItem] {
    static ROWS: OnceLock<Vec<LootItem>> = OnceLock::new();
    ROWS.get_or_init(|| {
        // Registers the same bytes under the same ids, once, so the keys below resolve.
        ensure_items_registered();
        LootCatalog::parse(LOOT_JSON)
            .map(|catalog| catalog.items)
            .unwrap_or_default()
    })
}

/// How much one point of a stat is worth to `hero` when choosing between two pieces.
///
/// Main stat first; Haste and Critical Strike next since every class scales with them;
/// Versatility is a flat multiplier worth less; Spirit is noise. Stamina and Armor only break
/// ties, except that the Warrior, who is meant to take hits, values armour a little more.
fn stat_weight(hero: Hero, stat: Stat) -> f32 {
    match stat {
        Stat::MainStat | Stat::Strength | Stat::Intellect => 1.0,
        Stat::Haste | Stat::CriticalStrike => 0.8,
        Stat::Versatility => 0.3,
        Stat::Stamina => 0.1,
        Stat::Armor if hero == Hero::Warrior => 0.05,
        Stat::Armor => 0.01,
        _ => 0.0,
    }
}

/// A piece's worth to `hero`: its base stats weighted by [`stat_weight`].
#[must_use]
pub fn item_score(hero: Hero, def: &ItemDef) -> f32 {
    Stat::STORED
        .into_iter()
        .map(|stat| def.stats.get(stat) * stat_weight(hero, stat))
        .sum()
}

/// The best catalog piece for `slot`: an epic if `epic` and the slot has an unworn one,
/// otherwise a rare; a piece not already worn if another exists. `None` only if the catalog has nothing
/// for the slot.
fn best_in_slot(
    hero: Hero,
    slot: EquipSlot,
    epic: bool,
    taken: &[&'static str],
) -> Option<&'static ItemDef> {
    let fitting: Vec<&'static ItemDef> = catalog_rows()
        .iter()
        .filter_map(|row| item_by_key(&row.id))
        // Set pieces are worn on purpose (the `p2_set` tier), never picked as best in slot:
        // otherwise a plain tier would quietly earn a bonus.
        .filter(|def| def.fits(slot) && def.set.is_none())
        .collect();
    // Preference order: the wanted quality not yet worn, then a rare not yet worn, then (only
    // if the catalog has too few pieces) a repeat of either.
    let of_quality = |wanted: ItemQuality, fresh: bool| -> Vec<&'static ItemDef> {
        fitting
            .iter()
            .copied()
            .filter(|def| def.quality == wanted && (!fresh || !taken.contains(&def.key)))
            .collect()
    };
    let mut attempts = Vec::new();
    if epic {
        attempts.push((ItemQuality::Epic, true));
    }
    attempts.push((ItemQuality::Rare, true));
    if epic {
        attempts.push((ItemQuality::Epic, false));
    }
    attempts.push((ItemQuality::Rare, false));
    let pool = attempts
        .into_iter()
        .map(|(quality, fresh)| of_quality(quality, fresh))
        .find(|pool| !pool.is_empty())
        .unwrap_or_default();
    // Strictly greater, so the earlier row wins a tie: catalog order is the tie-break.
    pool.into_iter().fold(None, |best, def| match best {
        Some(current) if item_score(hero, current) >= item_score(hero, def) => Some(current),
        _ => Some(def),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_names_resolve_and_top_is_high() {
        for tier in GearTier::ALL {
            assert_eq!(GearTier::parse(tier.as_str()), Some(tier));
        }
        assert_eq!(GearTier::parse("top"), Some(GearTier::High));
        assert_eq!(GearTier::parse("bogus"), None);
    }

    #[test]
    fn every_tier_above_none_fills_every_slot_with_real_items() {
        for hero in [Hero::Warrior, Hero::Druid, Hero::FireMage] {
            for profile in PROFILES {
                let sheet = profile.sheet(hero);
                let expected = if profile.tier == GearTier::None {
                    0
                } else {
                    EquipSlot::ALL.len()
                };
                assert_eq!(sheet.equipment().count(), expected, "{}", profile.label());
            }
        }
    }

    #[test]
    fn the_sheet_is_the_games_own_arithmetic_on_the_chosen_items() {
        let profile = GearProfile::headline(GearTier::High);
        let loadout = profile.loadout(Hero::FireMage);
        let by_hand = loadout.iter().fold(0.0, |total, (_, key, props)| {
            let def = item_by_key(key).expect("a catalog item");
            total + def.stats.get(Stat::MainStat) * props.multiplier()
        });
        let sheet = profile.sheet(Hero::FireMage);
        let gained = sheet.gear_stats().get(Stat::Intellect);
        assert!((gained - by_hand).abs() < 1e-3, "{gained} vs {by_hand}");
        assert_eq!(sheet.gear_stats().get(Stat::Strength), 0.0);
        let warrior = profile.sheet(Hero::Warrior).gear_stats();
        assert!(warrior.get(Stat::Strength) > 0.0);
        assert_eq!(warrior.get(Stat::Intellect), 0.0);
    }

    #[test]
    fn the_tiers_are_ordered_by_what_they_grant() {
        let total = |tier: GearTier| {
            let stats = GearProfile::headline(tier)
                .sheet(Hero::FireMage)
                .gear_stats();
            stats.get(Stat::Intellect) + stats.get(Stat::Haste) + stats.get(Stat::CriticalStrike)
        };
        let totals: Vec<f32> = GearTier::ALL.into_iter().map(total).collect();
        assert_eq!(totals[0], 0.0);
        assert!(
            totals.windows(2).all(|pair| pair[0] < pair[1]),
            "{totals:?}"
        );
    }


    #[test]
    fn only_the_set_tier_wears_a_set_and_it_wears_the_whole_major_one() {
        use engine_game::character::active_sets;
        for hero in [Hero::Warrior, Hero::Druid, Hero::FireMage] {
            let major = major_set_for(hero).expect("a major set per class");
            for profile in PROFILES {
                let sheet = profile.sheet(hero);
                let worn = active_sets(sheet.equipment());
                if profile.tier == GearTier::P2Set {
                    let active = worn.get(&major.id).expect("the major set is worn");
                    assert_eq!(usize::from(active.equipped), major.pieces.len());
                    assert_eq!(active.tiers_active(), major.bonuses.len());
                    assert_eq!(sheet.equipment().count(), EquipSlot::ALL.len());
                } else {
                    assert!(worn.is_empty(), "{} wears a set", profile.label());
                }
            }
        }
    }
    #[test]
    fn selection_is_deterministic_and_wears_no_item_twice() {
        for profile in &PROFILES[1..] {
            let first = profile.loadout(Hero::Druid);
            assert_eq!(first, profile.loadout(Hero::Druid));
            let mut keys: Vec<&String> = first.iter().map(|(_, key, _)| key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), first.len(), "{}", profile.label());
        }
    }
}
