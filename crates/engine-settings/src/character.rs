//! What each character is wearing and carrying, and the file it lives in.
//!
//! ```text
//! CharacterConfig          one loadout per hero: worn items, and the bag by position
//!      |
//!      v
//! CharacterConfigManager   load, validate, hold, save
//!      |
//!      v
//! character.cfg            JSON, beside the executable
//! ```
//!
//! ## Why this is a third file
//!
//! The same reason [`crate::hud`] is a second one. This is rewritten every time an item is
//! dragged, and a bag corrupted by a bad hand edit should not cost the player their camera
//! sensitivity or their action bars.
//!
//! ## Why an item is a string here
//!
//! This crate links no engine crate, which is what lets every load, validate and save rule be
//! tested with a temporary file and nothing else. So a worn item is a *name* and a slot is a
//! *name*, and turning `"simple_cloth_chestpiece"` into an item happens one layer up, in
//! `engine-presentation`, which already knows that vocabulary. A name this crate does not
//! recognise is not an error here: it is carried through unmodified, and the layer that does
//! recognise names resolves it to an empty slot.
//!
//! ## Positions are saved, not inferred
//!
//! A bag entry carries its own index. A player who left a gap in the third row finds the gap
//! in the third row, and a build that makes the bag bigger does not shuffle anything that was
//! already in it.
//!
//! ## Loading never fails
//!
//! [`CharacterConfigManager::load`] returns a configuration, not a `Result`, for the reason
//! the settings loader gives: a hand-edited file must not be able to stop the game from
//! starting.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Name of the character file. JSON content; the extension matches `settings.cfg`.
pub const CHARACTER_FILE_NAME: &str = "character.cfg";

/// Largest bag index the file may name.
///
/// A ceiling on nonsense rather than the size of the bag, which this crate does not know. An
/// index of four thousand is not a position, it is a corrupt file; an index that is merely
/// past the end of the current bag is dropped one layer up, where the size is known.
pub const MAX_INVENTORY_INDEX: usize = 1_024;

/// One item somewhere in a bag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// Which cell it is in.
    pub index: usize,
    /// The item's stable name.
    pub item: String,
    /// How many. At least one.
    pub quantity: u16,
}

impl InventoryEntry {
    /// One of `item`, in cell `index`.
    #[must_use]
    pub fn new(index: usize, item: impl Into<String>, quantity: u16) -> Self {
        Self {
            index,
            item: item.into(),
            quantity: quantity.max(1),
        }
    }

    /// Whether this entry is worth keeping: a real name, in a plausible cell.
    #[must_use]
    pub fn is_plausible(&self) -> bool {
        !self.item.trim().is_empty() && self.index <= MAX_INVENTORY_INDEX
    }
}

/// One worn item.
///
/// A list of these rather than a map, so the file has a stable order a person can read and
/// diff, and so a duplicate slot name is a thing the validator can see and drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquippedEntry {
    /// The slot's stable name: `"chest"`, `"ring1"`, and so on.
    pub slot: String,
    /// The item's stable name.
    pub item: String,
}

impl EquippedEntry {
    /// `item`, worn in `slot`.
    #[must_use]
    pub fn new(slot: impl Into<String>, item: impl Into<String>) -> Self {
        Self {
            slot: slot.into(),
            item: item.into(),
        }
    }

    /// Whether both names are worth keeping.
    #[must_use]
    pub fn is_plausible(&self) -> bool {
        !self.slot.trim().is_empty() && !self.item.trim().is_empty()
    }
}

/// One letter in a character's mailbox.
///
/// Sender and reason are names, like everything in this file; a name this build does not know
/// is kept as it is and resolved one layer up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MailEntry {
    /// Unique within the character's mailbox.
    pub id: u64,
    /// The attachment's stable name.
    pub item: String,
    /// Who sent it: `"dungeon_chest"`.
    pub sender: String,
    /// Why it was sent: `"bag_full"` or `"unclaimed"`.
    pub reason: String,
    /// When it arrived, in seconds on the clock the game stamped it with.
    pub received_at: f64,
    /// Whether the player has opened it.
    pub read: bool,
}

impl Default for MailEntry {
    fn default() -> Self {
        Self {
            id: 0,
            item: String::new(),
            sender: "dungeon_chest".to_owned(),
            reason: "unclaimed".to_owned(),
            received_at: 0.0,
            read: false,
        }
    }
}

impl MailEntry {
    /// Whether this letter is worth keeping: it names an item and its time is a number.
    #[must_use]
    pub fn is_plausible(&self) -> bool {
        !self.item.trim().is_empty() && self.received_at.is_finite()
    }
}

/// One character: which hero, what they wear, what they carry, what they have been mailed.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeroLoadout {
    /// The hero's stable name: `"firemage"`, `"warrior"`, `"druid"`.
    pub hero: String,
    /// What is worn, one entry per filled slot.
    pub equipment: Vec<EquippedEntry>,
    /// What is carried, one entry per filled cell, with its position.
    pub inventory: Vec<InventoryEntry>,
    /// Letters waiting in the mailbox, oldest first. Absent in files written before mail.
    pub mail: Vec<MailEntry>,
    /// The id the next letter gets, so ids are never reused after a letter is taken.
    pub next_mail_id: u64,
}

impl HeroLoadout {
    /// An empty loadout for `hero`.
    #[must_use]
    pub fn new(hero: impl Into<String>) -> Self {
        Self {
            hero: hero.into(),
            equipment: Vec::new(),
            inventory: Vec::new(),
            mail: Vec::new(),
            next_mail_id: 0,
        }
    }

    /// This loadout with every implausible entry dropped and every duplicate removed.
    ///
    /// The first mention of a slot or a cell wins, which makes a hand-edited file that names
    /// the same finger twice resolve the same way every time it is loaded.
    #[must_use]
    pub fn validated(mut self) -> Self {
        let mut seen_slots: Vec<String> = Vec::with_capacity(self.equipment.len());
        self.equipment.retain(|entry| {
            if !entry.is_plausible() || seen_slots.contains(&entry.slot) {
                log::warn!(
                    "character: dropping equipment entry {:?} for hero {:?}",
                    entry.slot,
                    self.hero
                );
                return false;
            }
            seen_slots.push(entry.slot.clone());
            true
        });

        let mut seen_cells: Vec<usize> = Vec::with_capacity(self.inventory.len());
        self.inventory.retain(|entry| {
            if !entry.is_plausible() || seen_cells.contains(&entry.index) {
                log::warn!(
                    "character: dropping inventory entry {} for hero {:?}",
                    entry.index,
                    self.hero
                );
                return false;
            }
            seen_cells.push(entry.index);
            true
        });
        for entry in &mut self.inventory {
            entry.quantity = entry.quantity.max(1);
        }

        let mut seen_letters: Vec<u64> = Vec::with_capacity(self.mail.len());
        self.mail.retain(|entry| {
            if !entry.is_plausible() || seen_letters.contains(&entry.id) {
                log::warn!(
                    "character: dropping mail {} for hero {:?}",
                    entry.id,
                    self.hero
                );
                return false;
            }
            seen_letters.push(entry.id);
            true
        });
        if let Some(highest) = seen_letters.iter().max() {
            self.next_mail_id = self.next_mail_id.max(highest.saturating_add(1));
        }
        self
    }
}

/// Every character the player owns.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterConfig {
    /// One loadout per hero. A hero with no entry has never been saved.
    pub heroes: Vec<HeroLoadout>,
}

impl CharacterConfig {
    /// The loadout saved for `hero`, if there is one.
    #[must_use]
    pub fn loadout(&self, hero: &str) -> Option<&HeroLoadout> {
        self.heroes.iter().find(|entry| entry.hero == hero)
    }

    /// Replaces the loadout for `loadout.hero`, or adds it.
    pub fn set_loadout(&mut self, loadout: HeroLoadout) {
        match self
            .heroes
            .iter_mut()
            .find(|entry| entry.hero == loadout.hero)
        {
            Some(existing) => *existing = loadout,
            None => self.heroes.push(loadout),
        }
    }

    /// This configuration with every loadout validated and every unnamed hero dropped.
    #[must_use]
    pub fn validated(mut self) -> Self {
        let mut seen: Vec<String> = Vec::with_capacity(self.heroes.len());
        self.heroes.retain(|loadout| {
            if loadout.hero.trim().is_empty() || seen.contains(&loadout.hero) {
                log::warn!("character: dropping loadout for hero {:?}", loadout.hero);
                return false;
            }
            seen.push(loadout.hero.clone());
            true
        });
        self.heroes = self
            .heroes
            .into_iter()
            .map(HeroLoadout::validated)
            .collect();
        self
    }
}

/// Loads, holds and writes [`CharacterConfig`].
#[derive(Debug)]
pub struct CharacterConfigManager {
    path: PathBuf,
    config: CharacterConfig,
    dirty: bool,
    existed: bool,
}

impl CharacterConfigManager {
    /// Loads the file beside the running executable, creating it if it is missing.
    #[must_use]
    pub fn load_from_game_directory() -> Self {
        Self::load(crate::game_directory().join(CHARACTER_FILE_NAME))
    }

    /// Loads `path`, repairing or creating it as needed. Never fails.
    #[must_use]
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (config, existed, needs_write) = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<CharacterConfig>(&text) {
                Ok(parsed) => {
                    let validated = parsed.clone().validated();
                    let repaired = validated != parsed;
                    (validated, true, repaired)
                }
                Err(error) => {
                    log::warn!(
                        "character: {} is not valid JSON ({error}); starting fresh characters",
                        path.display()
                    );
                    // Not `existed`: a file that cannot be parsed says nothing about what the
                    // player owned, so the caller should dress the roster as a new one.
                    (CharacterConfig::default(), false, true)
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                log::info!(
                    "character: {} does not exist; writing fresh characters",
                    path.display()
                );
                (CharacterConfig::default(), false, true)
            }
            Err(error) => {
                log::warn!(
                    "character: {} could not be read ({error}); starting fresh characters",
                    path.display()
                );
                return Self {
                    path,
                    config: CharacterConfig::default(),
                    dirty: false,
                    existed: false,
                };
            }
        };

        let mut manager = Self {
            path,
            config,
            dirty: needs_write,
            existed,
        };
        if needs_write {
            manager.save();
        }
        manager
    }

    /// A manager over `path` holding `config`, with nothing read and nothing written.
    #[must_use]
    pub fn with_config(path: impl Into<PathBuf>, config: CharacterConfig) -> Self {
        Self {
            path: path.into(),
            config: config.validated(),
            dirty: false,
            existed: false,
        }
    }

    /// What is loaded.
    #[must_use]
    pub fn config(&self) -> &CharacterConfig {
        &self.config
    }

    /// Where it is stored.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a usable file was there when this loaded.
    ///
    /// False means the player has never had a character saved, which is the one moment the
    /// caller should dress the roster in its starting gear. True means the file *is* what
    /// they own, empty slots and all, and filling any of it in would hand them items they had
    /// deliberately taken off.
    #[must_use]
    pub fn existed(&self) -> bool {
        self.existed
    }

    /// Whether what is held differs from what was last written.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Replaces what is held, validating it and marking it dirty if it changed.
    pub fn update(&mut self, config: CharacterConfig) {
        let validated = config.validated();
        if validated == self.config {
            return;
        }
        self.config = validated;
        self.dirty = true;
    }

    /// Writes the file if anything has changed since the last write.
    ///
    /// Returns whether anything was written.
    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.save();
        true
    }

    /// Writes the file, whether or not anything has changed.
    pub fn save(&mut self) {
        match serde_json::to_string_pretty(&self.config) {
            Ok(text) => {
                if let Some(parent) = self.path.parent()
                    && let Err(error) = fs::create_dir_all(parent)
                {
                    log::warn!("character: could not create {} ({error})", parent.display());
                    return;
                }
                match fs::write(&self.path, text) {
                    Ok(()) => self.dirty = false,
                    Err(error) => log::warn!(
                        "character: could not write {} ({error})",
                        self.path.display()
                    ),
                }
            }
            Err(error) => log::warn!("character: could not encode the characters ({error})"),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("engine-character-{name}"));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a writable temporary directory");
        path.join(CHARACTER_FILE_NAME)
    }

    fn loadout() -> HeroLoadout {
        HeroLoadout {
            hero: "firemage".to_owned(),
            equipment: vec![
                EquippedEntry::new("chest", "simple_cloth_chestpiece"),
                EquippedEntry::new("feet", "simple_cloth_boots"),
            ],
            inventory: vec![InventoryEntry::new(5, "plain_copper_band", 1)],
            ..HeroLoadout::default()
        }
    }

    #[test]
    fn a_missing_file_is_written_with_no_characters_in_it() {
        let path = temp("missing");
        let manager = CharacterConfigManager::load(&path);
        assert!(!manager.existed(), "nothing was there to be found");
        assert!(manager.config().heroes.is_empty());
        assert!(path.exists(), "the file was not created");
    }

    #[test]
    fn what_is_saved_is_what_is_loaded_back() {
        let path = temp("round-trip");
        let mut manager = CharacterConfigManager::load(&path);
        let mut config = CharacterConfig::default();
        config.set_loadout(loadout());
        manager.update(config.clone());
        assert!(manager.save_if_dirty());
        assert!(!manager.save_if_dirty(), "a clean file was written again");

        let reloaded = CharacterConfigManager::load(&path);
        assert!(reloaded.existed());
        assert_eq!(reloaded.config(), &config);
        assert_eq!(
            reloaded
                .config()
                .loadout("firemage")
                .expect("the saved hero")
                .inventory[0]
                .index,
            5,
            "the position was not kept"
        );
    }

    #[test]
    fn a_loadout_is_replaced_rather_than_repeated() {
        let mut config = CharacterConfig::default();
        config.set_loadout(loadout());
        config.set_loadout(HeroLoadout::new("firemage"));
        assert_eq!(config.heroes.len(), 1);
        assert!(
            config
                .loadout("firemage")
                .expect("the hero")
                .equipment
                .is_empty()
        );
    }

    #[test]
    fn a_garbled_file_starts_fresh_characters_rather_than_failing() {
        let path = temp("garbled");
        fs::write(&path, "{ this is not json").expect("a writable file");
        let manager = CharacterConfigManager::load(&path);
        assert!(
            !manager.existed(),
            "a file that cannot be read is not a save"
        );
        assert!(manager.config().heroes.is_empty());
    }

    #[test]
    fn nonsense_entries_are_dropped_and_the_rest_survives() {
        let config = CharacterConfig {
            heroes: vec![
                HeroLoadout {
                    hero: "warrior".to_owned(),
                    equipment: vec![
                        EquippedEntry::new("chest", "simple_cloth_chestpiece"),
                        EquippedEntry::new("chest", "worn_travellers_cloak"),
                        EquippedEntry::new("", "nothing"),
                    ],
                    inventory: vec![
                        InventoryEntry::new(0, "plain_copper_band", 1),
                        InventoryEntry::new(0, "tarnished_pendant", 1),
                        InventoryEntry::new(MAX_INVENTORY_INDEX + 1, "tarnished_pendant", 1),
                        InventoryEntry {
                            index: 3,
                            item: "dull_glass_figurine".to_owned(),
                            quantity: 0,
                        },
                    ],
                    ..HeroLoadout::default()
                },
                HeroLoadout::new(""),
                HeroLoadout::new("warrior"),
            ],
        }
        .validated();

        assert_eq!(
            config.heroes.len(),
            1,
            "the empty and duplicate heroes stay"
        );
        let warrior = config.loadout("warrior").expect("the warrior");
        assert_eq!(warrior.equipment.len(), 1);
        assert_eq!(warrior.equipment[0].item, "simple_cloth_chestpiece");
        assert_eq!(warrior.inventory.len(), 2);
        assert_eq!(warrior.inventory[0].item, "plain_copper_band");
        assert_eq!(
            warrior.inventory[1].quantity, 1,
            "a quantity of zero stands"
        );
    }

    #[test]
    fn a_name_this_crate_does_not_know_is_carried_through_untouched() {
        // The rule the HUD file follows: this crate does not have the vocabulary to say a
        // name is wrong, so it does not try. The layer that does resolves it to nothing.
        let config = CharacterConfig {
            heroes: vec![HeroLoadout {
                hero: "necromancer".to_owned(),
                equipment: vec![EquippedEntry::new("phylactery", "soul_jar")],
                inventory: Vec::new(),
                ..HeroLoadout::default()
            }],
        }
        .validated();
        assert_eq!(config.heroes[0].equipment[0].item, "soul_jar");
    }

    fn letter(id: u64, item: &str) -> MailEntry {
        MailEntry {
            id,
            item: item.to_owned(),
            ..MailEntry::default()
        }
    }

    #[test]
    fn mail_is_saved_and_loaded_back() {
        let path = temp("mail-round-trip");
        let mut manager = CharacterConfigManager::load(&path);
        let mut hero = HeroLoadout::new("warrior");
        hero.mail = vec![
            MailEntry {
                reason: "bag_full".to_owned(),
                received_at: 1_750_000_000.5,
                read: true,
                ..letter(3, "plain_copper_band")
            },
            letter(7, "tarnished_pendant"),
        ];
        hero.next_mail_id = 12;
        let mut config = CharacterConfig::default();
        config.set_loadout(hero);
        manager.update(config.clone());
        manager.save();

        let reloaded = CharacterConfigManager::load(&path);
        assert_eq!(reloaded.config(), &config);
        assert_eq!(reloaded.config().heroes[0].next_mail_id, 12);
    }

    #[test]
    fn a_file_written_before_mail_still_loads() {
        let path = temp("pre-mail");
        fs::write(
            &path,
            r#"{"heroes":[{"hero":"druid","equipment":[],"inventory":[]}]}"#,
        )
        .expect("a writable file");
        let manager = CharacterConfigManager::load(&path);
        assert!(manager.existed());
        let druid = manager.config().loadout("druid").expect("the druid");
        assert!(druid.mail.is_empty());
    }

    #[test]
    fn implausible_and_duplicate_letters_are_dropped_and_ids_stay_ahead() {
        let mut hero = HeroLoadout::new("firemage");
        hero.mail = vec![
            letter(4, "plain_copper_band"),
            letter(4, "tarnished_pendant"),
            letter(5, ""),
            letter(9, "tarnished_pendant"),
        ];
        let hero = hero.validated();
        assert_eq!(hero.mail.len(), 2);
        assert_eq!(hero.next_mail_id, 10, "a new letter could reuse an id");
    }
}
