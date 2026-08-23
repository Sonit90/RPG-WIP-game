//! The HUD layout the player arranged, and the file it lives in.
//!
//! ```text
//! HudConfig          placements, orientations, spells and keys, as plain data
//!      |
//!      v
//! HudConfigManager   load, validate, hold, save
//!      |
//!      v
//! hud.cfg            JSON, beside the executable
//! ```
//!
//! ## Why this is a second file
//!
//! [`crate::GameSettings`] is a handful of scalars that change when a player opens a
//! settings screen. This is the whole interface layout, and it is rewritten every time an
//! element is dragged to a new place. Keeping them apart means dragging an action bar does
//! not rewrite the audio settings, and a layout file corrupted by a bad hand edit does not
//! cost the player their camera sensitivity.
//!
//! ## Why a spell is a string here
//!
//! This crate links no engine crate — that is what lets every load, validate and save rule
//! be tested with a temporary file and nothing else. So a slot's spell is a name and its key
//! is a name, and turning `"firebolt"` into a `FireAbility` and `"DIGIT1"` into a `KeyCode`
//! happens one layer up, in `engine-presentation`, which already knows both vocabularies. A
//! name this crate does not recognise is not an error here: it is carried through
//! unmodified, and the layer that does recognise names resolves it to an empty slot.
//!
//! ## Loading never fails
//!
//! [`HudConfigManager::load`] returns a configuration, not a `Result`, for the reason the
//! settings loader gives: a hand-edited file must not be able to stop the game from
//! starting. Every invalid field falls back to *that field's* default, and a file that
//! cannot be parsed at all falls back to the whole default layout.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Name of the HUD layout file. JSON content; the extension matches `settings.cfg`.
pub const HUD_FILE_NAME: &str = "hud.cfg";

/// How many action bars the interface has.
pub const ACTION_BARS: usize = 8;

/// How many slots one action bar has.
///
/// Ten, which is the number of abilities the class's bar holds and the number of keys the
/// digit row offers. The gameplay constant it agrees with is `engine_game::ABILITY_SLOTS`;
/// this crate cannot name it, and a test one layer up asserts the two are equal.
pub const ACTION_BAR_SLOTS: usize = 10;

/// Smallest a HUD element can be scaled to, as a multiple of its design size.
///
/// Half. Below that the 5x7 font's glyph scale rounds to nothing and a frame becomes an
/// unreadable smear rather than a small one.
pub const HUD_SCALE_MIN: f32 = 0.50;

/// Largest a HUD element can be scaled to.
pub const HUD_SCALE_MAX: f32 = 2.00;

/// The size an element is drawn at until the player changes it.
pub const HUD_SCALE_DEFAULT: f32 = 1.00;

/// How much one press of the bigger/smaller control changes an element's scale.
///
/// Five per cent. A tenth was coarse enough that a bar was either slightly too small or
/// noticeably too big, with nothing in between; half that gives the range from
/// [`HUD_SCALE_MIN`] to [`HUD_SCALE_MAX`] thirty stops instead of fifteen.
pub const HUD_SCALE_STEP: f32 = 0.05;

/// Furthest an element may be offset from its anchor, as a fraction of the viewport.
///
/// One whole screen in either direction, which is more than enough to reach anywhere from
/// any anchor. It exists to reject a number that means nothing — an offset of 400 is not a
/// position, it is a corrupt file — rather than to constrain where an element may sit. What
/// actually keeps an element reachable is the clamp applied when the layout is resolved.
pub const HUD_OFFSET_LIMIT: f32 = 1.00;

/// Which point of the screen an element's offset is measured from.
///
/// An anchor as well as a fraction, because a fraction alone does not survive a change of
/// aspect ratio the way a player expects. An action bar centred at the bottom of a 16:9
/// screen should still be centred at the bottom of a 21:9 one, not four-fifths of the way
/// across it. The anchor carries the intent; the offset carries the nudge away from it.
///
/// The same point of the *element* is aligned to the anchor, so a right-anchored element
/// grows leftwards and stays against the edge it was put against.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HudAnchor {
    /// The top-left corner.
    TopLeft,
    /// The middle of the top edge.
    Top,
    /// The top-right corner.
    TopRight,
    /// The middle of the left edge.
    Left,
    /// The middle of the screen.
    #[default]
    Center,
    /// The middle of the right edge.
    Right,
    /// The bottom-left corner.
    BottomLeft,
    /// The middle of the bottom edge.
    Bottom,
    /// The bottom-right corner.
    BottomRight,
}

impl HudAnchor {
    /// Every anchor, for a control that cycles through them and for tests.
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// Where this anchor sits, as a fraction of the viewport: `(0, 0)` is the top-left.
    #[must_use]
    pub fn origin(self) -> (f32, f32) {
        let x = match self {
            Self::TopLeft | Self::Left | Self::BottomLeft => 0.0,
            Self::Top | Self::Center | Self::Bottom => 0.5,
            Self::TopRight | Self::Right | Self::BottomRight => 1.0,
        };
        let y = match self {
            Self::TopLeft | Self::Top | Self::TopRight => 0.0,
            Self::Left | Self::Center | Self::Right => 0.5,
            Self::BottomLeft | Self::Bottom | Self::BottomRight => 1.0,
        };
        (x, y)
    }

    /// Which point of the element is placed at the anchor, as a fraction of its own size.
    ///
    /// The same fractions as [`HudAnchor::origin`]: an element anchored bottom-right has its
    /// own bottom-right corner at the screen's, so growing it moves it inwards rather than
    /// off the edge.
    #[must_use]
    pub fn alignment(self) -> (f32, f32) {
        self.origin()
    }

    /// The name printed for this anchor in the editor.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "TOP LEFT",
            Self::Top => "TOP",
            Self::TopRight => "TOP RIGHT",
            Self::Left => "LEFT",
            Self::Center => "CENTRE",
            Self::Right => "RIGHT",
            Self::BottomLeft => "BOTTOM LEFT",
            Self::Bottom => "BOTTOM",
            Self::BottomRight => "BOTTOM RIGHT",
        }
    }

    /// The anchor whose region of the screen contains a point given in viewport fractions.
    ///
    /// What a finished drag uses to decide which edge the player parked an element against:
    /// the screen is split into a three-by-three grid, and an element dropped in the
    /// bottom-middle ninth is anchored to the bottom middle. Re-anchoring on release rather
    /// than keeping the old anchor is what makes a bar dragged to the bottom of a 16:9
    /// screen stay at the bottom of a 21:9 one.
    #[must_use]
    pub fn nearest(x: f32, y: f32) -> Self {
        let column = if x < 1.0 / 3.0 {
            0
        } else if x < 2.0 / 3.0 {
            1
        } else {
            2
        };
        let row = if y < 1.0 / 3.0 {
            0
        } else if y < 2.0 / 3.0 {
            1
        } else {
            2
        };
        Self::ALL[row * 3 + column]
    }
}

/// Which way an action bar's slots run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HudOrientation {
    /// Left to right.
    #[default]
    Horizontal,
    /// Top to bottom.
    Vertical,
}

impl HudOrientation {
    /// The other one. What the editor's orientation control applies.
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    /// The name printed for this orientation in the editor.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Horizontal => "HORIZONTAL",
            Self::Vertical => "VERTICAL",
        }
    }
}

/// Where one HUD element sits, how big it is, and whether it is there at all.
///
/// The one shape every element shares. An action bar is this plus an orientation and ten
/// slots; a unit frame is this and nothing else — which is the point, because it means
/// dragging, scaling, toggling and saving are written once rather than once per element.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudPlacement {
    /// Whether the element is drawn and can be interacted with.
    pub enabled: bool,
    /// Which point of the screen [`HudPlacement::offset_x`] and `offset_y` are measured from.
    pub anchor: HudAnchor,
    /// Horizontal offset from the anchor, as a fraction of the viewport's width.
    pub offset_x: f32,
    /// Vertical offset from the anchor, as a fraction of the viewport's height.
    pub offset_y: f32,
    /// How big the element is drawn, as a multiple of its design size.
    pub scale: f32,
}

impl Default for HudPlacement {
    fn default() -> Self {
        Self {
            enabled: true,
            anchor: HudAnchor::Center,
            offset_x: 0.0,
            offset_y: 0.0,
            scale: HUD_SCALE_DEFAULT,
        }
    }
}

impl HudPlacement {
    /// A placement at an anchor and an offset, enabled and at its design size.
    #[must_use]
    pub fn at(anchor: HudAnchor, offset_x: f32, offset_y: f32) -> Self {
        Self {
            anchor,
            offset_x,
            offset_y,
            ..Self::default()
        }
    }

    /// The same placement, disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// The same placement at a different size.
    #[must_use]
    pub fn scaled(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    /// This placement with every out-of-range or non-finite field replaced by `fallback`'s.
    ///
    /// Replaced rather than clamped, for the reason the settings file already gives: a value
    /// that means nothing is not a request for the nearest value that does.
    #[must_use]
    pub fn validated(self, fallback: Self, what: &str) -> Self {
        let mut placement = self;
        if !offset_is_valid(placement.offset_x) || !offset_is_valid(placement.offset_y) {
            log::warn!(
                "hud: {what} is offset ({}, {}), which is not a position; using ({}, {})",
                placement.offset_x,
                placement.offset_y,
                fallback.offset_x,
                fallback.offset_y
            );
            placement.offset_x = fallback.offset_x;
            placement.offset_y = fallback.offset_y;
        }
        if !(placement.scale.is_finite()
            && (HUD_SCALE_MIN..=HUD_SCALE_MAX).contains(&placement.scale))
        {
            log::warn!(
                "hud: {what} is scaled {} , which is outside {HUD_SCALE_MIN}..{HUD_SCALE_MAX}; \
                 using {}",
                placement.scale,
                fallback.scale
            );
            placement.scale = fallback.scale;
        }
        placement
    }
}

/// Whether an offset is a position at all.
fn offset_is_valid(offset: f32) -> bool {
    offset.is_finite() && offset.abs() <= HUD_OFFSET_LIMIT
}

/// What one action bar slot holds.
///
/// Both fields are names rather than identifiers, and both are optional: an empty slot is
/// the ordinary state of most of eight bars, and a slot can hold a spell with no key or a
/// key with no spell without either being a broken configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudSlotConfig {
    /// The spell in this slot, by name, or `None` if it is empty.
    pub spell: Option<String>,
    /// The key that triggers it, by [`engine_core::input::KeyCode::name`], or `None`.
    pub key: Option<String>,
}

impl HudSlotConfig {
    /// A slot holding a spell and a key.
    #[must_use]
    pub fn new(spell: &str, key: &str) -> Self {
        Self {
            spell: Some(spell.to_owned()),
            key: Some(key.to_owned()),
        }
    }

    /// Whether the slot holds nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spell.is_none() && self.key.is_none()
    }
}

/// One action bar: where it is, which way it runs, and what is in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionBarConfig {
    /// Where the bar sits, how big it is, and whether it exists on screen.
    pub placement: HudPlacement,
    /// Which way its slots run.
    pub orientation: HudOrientation,
    /// Its slots, in slot order. Always [`ACTION_BAR_SLOTS`] long after validation.
    pub slots: Vec<HudSlotConfig>,
}

impl Default for ActionBarConfig {
    fn default() -> Self {
        Self {
            placement: HudPlacement::default().disabled(),
            orientation: HudOrientation::Horizontal,
            slots: vec![HudSlotConfig::default(); ACTION_BAR_SLOTS],
        }
    }
}

impl ActionBarConfig {
    /// The default placement of bar `index`, counting from zero.
    ///
    /// Bar one sits along the bottom, where the game's only bar sits today. The rest stack
    /// upwards from it in the order a player would enable them, so turning bar two on puts
    /// it directly above bar one rather than on top of it.
    #[must_use]
    pub fn default_placement(index: usize) -> HudPlacement {
        let placement = HudPlacement::at(HudAnchor::Bottom, 0.0, -0.030 - 0.075 * index as f32);
        if index == 0 {
            placement
        } else {
            placement.disabled()
        }
    }

    /// Bar `index` as a fresh install has it: placed, horizontal, and empty.
    ///
    /// Empty even for bar one. What goes *in* bar one is the class's ten abilities, and this
    /// crate cannot name them — the layer that can fills them in the first time the game
    /// runs, which is why [`HudConfigManager::existed`] is worth asking about.
    #[must_use]
    pub fn default_bar(index: usize) -> Self {
        Self {
            placement: Self::default_placement(index),
            ..Self::default()
        }
    }

    /// This bar with every invalid field replaced by the default for bar `index`.
    #[must_use]
    pub fn validated(mut self, index: usize) -> Self {
        let label = format!("action bar {}", index + 1);
        self.placement = self
            .placement
            .validated(Self::default_placement(index), &label);
        if self.slots.len() != ACTION_BAR_SLOTS {
            log::warn!(
                "hud: {label} has {} slots rather than {ACTION_BAR_SLOTS}; padding or trimming",
                self.slots.len()
            );
            self.slots
                .resize(ACTION_BAR_SLOTS, HudSlotConfig::default());
        }
        self
    }
}

/// The unit frames, one field each.
///
/// Named fields rather than a list, because there are exactly four of them and a file that
/// says `"player"` is one a person can edit. Each is a bare [`HudPlacement`]: a frame has no
/// orientation and no slots, so it needs nothing an element does not already have.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudFrames {
    /// The player's own frame.
    pub player: HudPlacement,
    /// The selected target.
    pub target: HudPlacement,
    /// The target's target.
    pub target_of_target: HudPlacement,
    /// The party.
    pub party: HudPlacement,
}

impl Default for HudFrames {
    /// The layout a new player starts with.
    ///
    /// ```text
    ///                    [ TARGET ] [ TOT ]
    ///
    ///   [ PLAYER ]
    ///   [ PARTY  ]
    ///
    ///                   [ ACTION BAR 1 ]
    /// ```
    fn default() -> Self {
        Self {
            player: HudPlacement::at(HudAnchor::Left, 0.030, -0.120),
            target: HudPlacement::at(HudAnchor::Top, -0.090, 0.030),
            target_of_target: HudPlacement::at(HudAnchor::Top, 0.140, 0.030).scaled(0.80),
            party: HudPlacement::at(HudAnchor::Left, 0.030, 0.060),
        }
    }
}

impl HudFrames {
    /// These frames with every invalid field replaced by its default.
    #[must_use]
    pub fn validated(self) -> Self {
        let fallback = Self::default();
        Self {
            player: self.player.validated(fallback.player, "the player frame"),
            target: self.target.validated(fallback.target, "the target frame"),
            target_of_target: self
                .target_of_target
                .validated(fallback.target_of_target, "the target-of-target frame"),
            party: self.party.validated(fallback.party, "the party frame"),
        }
    }
}

/// Whose arrangement the action bars' spells and keys are.
///
/// The bars themselves — where they sit, how big they are, which way they run — are the
/// player's interface and are always one arrangement for the account. This is only about
/// what is *in* the slots, which is the half that is a fact about a character: a Druid's
/// keys are not a FireMage's, and a player who arranges one and then swaps has to decide
/// whether they meant to arrange both.
///
/// Both answers are legitimate, so it is a setting rather than a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingScope {
    /// One set of spells and keys for the whole account, shared by every character.
    ///
    /// The default, because it is what the game did before the setting existed: bar one
    /// follows the class on a swap and the other seven are the player's own, whoever they
    /// are playing.
    #[default]
    Account,
    /// One set of spells and keys per character, remembered separately.
    ///
    /// A swap saves what the old character had and loads what the new one had, so arranging
    /// the Druid's bars cannot disturb the FireMage's.
    Character,
}

impl BindingScope {
    /// Every scope, in the order the setting offers them.
    pub const ALL: [Self; 2] = [Self::Account, Self::Character];

    /// Whether bars are remembered per character.
    #[must_use]
    pub fn is_per_character(self) -> bool {
        matches!(self, Self::Character)
    }

    /// The scope a checkbox in this state means.
    #[must_use]
    pub fn from_per_character(per_character: bool) -> Self {
        if per_character {
            Self::Character
        } else {
            Self::Account
        }
    }
}

/// One bar's slots, with no placement.
///
/// A per-character record holds what the slots contain and nothing else: where the bar is
/// drawn is the player's interface, one arrangement for the account, and duplicating it per
/// character would mean dragging a bar moved it for one hero and not the others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudBarBindings {
    /// The slots, in slot order. Always [`ACTION_BAR_SLOTS`] long after validation.
    pub slots: Vec<HudSlotConfig>,
}

impl Default for HudBarBindings {
    fn default() -> Self {
        Self {
            slots: vec![HudSlotConfig::default(); ACTION_BAR_SLOTS],
        }
    }
}

impl HudBarBindings {
    /// These slots, padded or trimmed to the length the interface has.
    #[must_use]
    pub fn validated(mut self) -> Self {
        if self.slots.len() != ACTION_BAR_SLOTS {
            self.slots
                .resize(ACTION_BAR_SLOTS, HudSlotConfig::default());
        }
        self
    }
}

/// What one character has in its bars.
///
/// Written for every character the player has arranged, whatever the scope currently is: a
/// player who tries per-character bars, goes back to one set for the account and later
/// returns finds their characters as they left them, because nothing deleted them in
/// between.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeroBindingsConfig {
    /// The hero's stable name: `"firemage"`, `"warrior"`, `"druid"`.
    pub hero: String,
    /// Its bars, in bar order. Always [`ACTION_BARS`] long after validation.
    pub bars: Vec<HudBarBindings>,
}

impl Default for HeroBindingsConfig {
    fn default() -> Self {
        Self {
            hero: String::new(),
            bars: vec![HudBarBindings::default(); ACTION_BARS],
        }
    }
}

impl HeroBindingsConfig {
    /// An empty record for `hero`.
    #[must_use]
    pub fn new(hero: impl Into<String>) -> Self {
        Self {
            hero: hero.into(),
            ..Self::default()
        }
    }

    /// This record with the right number of bars, each with the right number of slots, and
    /// no key claimed twice.
    #[must_use]
    pub fn validated(mut self) -> Self {
        if self.bars.len() != ACTION_BARS {
            log::warn!(
                "hud: character {:?} has {} bars rather than {ACTION_BARS}; padding or trimming",
                self.hero,
                self.bars.len()
            );
            self.bars.resize(ACTION_BARS, HudBarBindings::default());
        }
        self.bars = self
            .bars
            .into_iter()
            .map(HudBarBindings::validated)
            .collect();
        let mut claimed: Vec<String> = Vec::new();
        for bar in &mut self.bars {
            for slot in &mut bar.slots {
                let Some(key) = slot.key.clone() else {
                    continue;
                };
                if claimed.contains(&key) {
                    log::warn!(
                        "hud: character {:?} claims key {key} twice; the later slot is left unbound",
                        self.hero
                    );
                    slot.key = None;
                } else {
                    claimed.push(key);
                }
            }
        }
        self
    }
}

/// The whole interface layout: eight action bars and four unit frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudConfig {
    /// The action bars, in bar order. Always [`ACTION_BARS`] long after validation.
    ///
    /// Where every bar is drawn, always; what is in its slots when the scope is the account.
    pub bars: Vec<ActionBarConfig>,
    /// What each character has in its bars, used when the scope is the character.
    ///
    /// Empty until a player saves a character's bars, and never emptied afterwards. A
    /// character with no record here has never been arranged, which is what tells the game
    /// to give it its class's own bar rather than somebody else's.
    #[serde(default)]
    pub heroes: Vec<HeroBindingsConfig>,
    /// The unit frames.
    pub frames: HudFrames,
}

impl Default for HudConfig {
    fn default() -> Self {
        Self {
            bars: (0..ACTION_BARS).map(ActionBarConfig::default_bar).collect(),
            heroes: Vec::new(),
            frames: HudFrames::default(),
        }
    }
}

impl HudConfig {
    /// This layout with every out-of-range, missing or duplicated value repaired.
    ///
    /// Total, and it never returns something the rest of the game has to check again: after
    /// this there are exactly [`ACTION_BARS`] bars of exactly [`ACTION_BAR_SLOTS`] slots, no
    /// key appears twice, and every offset and scale is a number that means something.
    #[must_use]
    pub fn validated(mut self) -> Self {
        if self.bars.len() != ACTION_BARS {
            log::warn!(
                "hud: {} action bars rather than {ACTION_BARS}; padding or trimming",
                self.bars.len()
            );
            self.bars.resize_with(ACTION_BARS, ActionBarConfig::default);
            for (index, bar) in self.bars.iter_mut().enumerate() {
                if *bar == ActionBarConfig::default() {
                    *bar = ActionBarConfig::default_bar(index);
                }
            }
        }
        self.bars = self
            .bars
            .into_iter()
            .enumerate()
            .map(|(index, bar)| bar.validated(index))
            .collect();
        self.frames = self.frames.validated();
        self.deduplicate_keys();
        self.validate_heroes();
        self
    }

    /// Unbinds every key that a slot earlier in the file already claimed.
    ///
    /// The first claim wins, because it is the one a player reading the file top to bottom
    /// would expect to. Two slots on one key is not a configuration the input layer can act
    /// on — a press would have to pick one — so the ambiguity is resolved here rather than
    /// left for whichever loop happens to run first.
    fn deduplicate_keys(&mut self) {
        let mut claimed: Vec<String> = Vec::new();
        for (bar_index, bar) in self.bars.iter_mut().enumerate() {
            for (slot_index, slot) in bar.slots.iter_mut().enumerate() {
                let Some(key) = slot.key.clone() else {
                    continue;
                };
                if claimed.contains(&key) {
                    log::warn!(
                        "hud: key {key} is claimed twice; action bar {} slot {} is left unbound",
                        bar_index + 1,
                        slot_index + 1
                    );
                    slot.key = None;
                } else {
                    claimed.push(key);
                }
            }
        }
    }

    /// Whether this layout is already exactly what validation would produce.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        &self.clone().validated() == self
    }

    /// Repairs the per-character records: one per hero, each a usable set of bars.
    ///
    /// A record naming no hero is dropped, because nothing could ever ask for it, and a hero
    /// named twice keeps its first record for the same reason a key claimed twice keeps its
    /// first claim: the file read top to bottom is what the player would expect.
    fn validate_heroes(&mut self) {
        let mut seen: Vec<String> = Vec::new();
        let mut kept: Vec<HeroBindingsConfig> = Vec::with_capacity(self.heroes.len());
        for entry in std::mem::take(&mut self.heroes) {
            if entry.hero.is_empty() {
                log::warn!("hud: a per-character bar record names no character; dropping it");
                continue;
            }
            if seen.contains(&entry.hero) {
                log::warn!(
                    "hud: character {:?} has two bar records; keeping the first",
                    entry.hero
                );
                continue;
            }
            seen.push(entry.hero.clone());
            kept.push(entry.validated());
        }
        self.heroes = kept;
    }

    /// Bar `index`, if there is one.
    #[must_use]
    pub fn bar(&self, index: usize) -> Option<&ActionBarConfig> {
        self.bars.get(index)
    }

    /// What `hero` has in its bars, if that character has ever been arranged.
    #[must_use]
    pub fn hero_bindings(&self, hero: &str) -> Option<&HeroBindingsConfig> {
        self.heroes.iter().find(|entry| entry.hero == hero)
    }

    /// Replaces what `hero` has in its bars, adding the record if it is the first.
    pub fn set_hero_bindings(&mut self, bindings: HeroBindingsConfig) {
        match self
            .heroes
            .iter_mut()
            .find(|entry| entry.hero == bindings.hero)
        {
            Some(existing) => *existing = bindings,
            None => self.heroes.push(bindings),
        }
    }
}

/// Where the HUD layout is stored, and what is currently in it.
///
/// The same shape as [`crate::SettingsManager`], deliberately: holds the layout in memory,
/// marks it dirty when it changes, and writes only when asked. A drag mutates memory sixty
/// times a second and touches the disk once, on release.
#[derive(Debug, Clone)]
pub struct HudConfigManager {
    path: PathBuf,
    config: HudConfig,
    dirty: bool,
    existed: bool,
}

impl HudConfigManager {
    /// Loads the layout beside the running executable, creating the file if it is missing.
    #[must_use]
    pub fn load_from_game_directory() -> Self {
        Self::load(crate::game_directory().join(HUD_FILE_NAME))
    }

    /// Loads `path`, repairing or creating it as needed. Never fails.
    #[must_use]
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let (config, existed, needs_write) = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<HudConfig>(&text) {
                Ok(parsed) => {
                    let validated = parsed.clone().validated();
                    let repaired = validated != parsed;
                    (validated, true, repaired)
                }
                Err(error) => {
                    log::warn!(
                        "hud: {} is not valid JSON ({error}); using the default layout",
                        path.display()
                    );
                    // Not `existed`: a file that cannot be parsed tells the caller nothing
                    // about what the player arranged, so the defaults it gets are a fresh
                    // layout and should be filled in as one.
                    (HudConfig::default(), false, true)
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                log::info!(
                    "hud: {} does not exist; writing the default layout",
                    path.display()
                );
                (HudConfig::default(), false, true)
            }
            Err(error) => {
                log::warn!(
                    "hud: {} could not be read ({error}); using the default layout",
                    path.display()
                );
                return Self {
                    path,
                    config: HudConfig::default(),
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
    pub fn with_config(path: impl Into<PathBuf>, config: HudConfig) -> Self {
        Self {
            path: path.into(),
            config: config.validated(),
            dirty: false,
            existed: false,
        }
    }

    /// The layout in use.
    #[must_use]
    pub fn config(&self) -> &HudConfig {
        &self.config
    }

    /// Where the layout is stored.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a usable layout file was there when this loaded.
    ///
    /// False means the player has never arranged anything, which is the one moment the
    /// caller should fill bar one with the class's abilities and their keys. True means the
    /// file *is* the arrangement, empty slots and all, and filling any of it in would undo
    /// something the player deliberately cleared.
    #[must_use]
    pub fn existed(&self) -> bool {
        self.existed
    }

    /// Whether the layout in memory differs from what was last written.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Replaces the layout, validating it and marking it dirty if it changed.
    pub fn update(&mut self, config: HudConfig) {
        let validated = config.validated();
        if validated == self.config {
            return;
        }
        self.config = validated;
        self.dirty = true;
    }

    /// Writes the layout if it has changed since the last write.
    ///
    /// Returns whether anything was written. What a released drag and a closed editor call.
    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.save();
        true
    }

    /// Writes the layout, whether or not it has changed.
    ///
    /// A failed write is logged and otherwise ignored: an unwritable directory is a reason
    /// to lose an arrangement, not to lose the session.
    pub fn save(&mut self) {
        match write_config(&self.path, &self.config) {
            Ok(()) => {
                self.dirty = false;
                // `existed` is deliberately left alone. It records what was on disk when
                // this loaded, which is the question "has the player ever arranged
                // anything" — and writing the defaults out is not an arrangement.
                log::debug!("hud: wrote {}", self.path.display());
            }
            Err(error) => log::warn!(
                "hud: could not write {} ({error}); the change is in memory only",
                self.path.display()
            ),
        }
    }
}

/// Serializes `config` into `path`, creating the parent directory if needed.
fn write_config(path: &Path, config: &HudConfig) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(config)?;
    text.push('\n');
    fs::write(path, text)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {

    #[test]
    fn a_character_keeps_its_own_bars_beside_the_accounts() {
        let mut config = HudConfig::default();
        assert!(config.hero_bindings("druid").is_none());

        let mut druid = HeroBindingsConfig::new("druid");
        druid.bars[0].slots[0] = HudSlotConfig::new("moonfire", "DIGIT1");
        config.set_hero_bindings(druid.clone());
        assert_eq!(config.hero_bindings("druid"), Some(&druid));

        // Saving the same character again replaces its record rather than adding a second.
        let mut again = HeroBindingsConfig::new("druid");
        again.bars[0].slots[0] = HudSlotConfig::new("wrath", "DIGIT1");
        config.set_hero_bindings(again.clone());
        assert_eq!(config.heroes.len(), 1);
        assert_eq!(config.hero_bindings("druid"), Some(&again));
    }

    #[test]
    fn per_character_bars_survive_a_write_and_a_read() {
        let dir = TempDir::new("hero-bindings");

        let mut manager = HudConfigManager::load(dir.file());
        let mut config = manager.config().clone();
        let mut warrior = HeroBindingsConfig::new("warrior");
        warrior.bars[2].slots[4] = HudSlotConfig::new("shield_block", "F");
        config.set_hero_bindings(warrior.clone());
        manager.update(config);
        assert!(manager.save_if_dirty());

        let reloaded = HudConfigManager::load(dir.file());
        assert_eq!(reloaded.config().hero_bindings("warrior"), Some(&warrior));
        // The account's own bars are untouched by a per-character record.
        assert_eq!(reloaded.config().bars.len(), ACTION_BARS);
    }

    #[test]
    fn a_file_written_before_the_setting_existed_still_loads() {
        // The whole point of `#[serde(default)]` on the section: an old file has no
        // `heroes` key at all, and must load as a player with no per-character records
        // rather than as a parse failure.
        let config: HudConfig =
            serde_json::from_str(r#"{"bars": [], "frames": {}}"#).expect("an old file parses");
        let config = config.validated();
        assert!(config.heroes.is_empty());
        assert_eq!(config.bars.len(), ACTION_BARS);
    }

    #[test]
    fn a_hand_edited_character_section_is_repaired_rather_than_refused() {
        let mut config = HudConfig::default();
        // Three faults at once: a record naming nobody, the same hero twice, and a record
        // with the wrong number of bars and a key claimed twice inside it.
        config.heroes.push(HeroBindingsConfig::new(""));
        let mut druid = HeroBindingsConfig::new("druid");
        druid.bars.truncate(2);
        druid.bars[0].slots[0] = HudSlotConfig::new("moonfire", "DIGIT1");
        druid.bars[1].slots[3] = HudSlotConfig::new("wrath", "DIGIT1");
        config.heroes.push(druid);
        config.heroes.push(HeroBindingsConfig::new("druid"));

        let config = config.validated();
        assert_eq!(config.heroes.len(), 1, "the nameless and the duplicate stayed");
        let kept = config.hero_bindings("druid").expect("the first druid record");
        assert_eq!(kept.bars.len(), ACTION_BARS);
        assert_eq!(kept.bars[0].slots.len(), ACTION_BAR_SLOTS);
        assert_eq!(kept.bars[0].slots[0].key.as_deref(), Some("DIGIT1"));
        assert_eq!(kept.bars[1].slots[3].key, None, "the second claim survived");
        assert!(config.is_valid(), "validation is not a fixed point");
    }

    #[test]
    fn a_scope_is_read_from_the_name_a_player_would_write() {
        assert_eq!(BindingScope::default(), BindingScope::Account);
        assert!(!BindingScope::Account.is_per_character());
        assert!(BindingScope::Character.is_per_character());
        assert_eq!(
            BindingScope::from_per_character(true),
            BindingScope::Character
        );
        assert_eq!(
            BindingScope::from_per_character(false),
            BindingScope::Account
        );
        let written = serde_json::to_string(&BindingScope::Character).expect("a scope writes");
        assert_eq!(written, "\"character\"");
        let read: BindingScope = serde_json::from_str("\"account\"").expect("a scope reads");
        assert_eq!(read, BindingScope::Account);
    }
    use super::*;

    /// A directory of this test's own, removed when the guard is dropped.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("engine-hud-{name}"));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a writable temporary directory");
            Self { path }
        }

        fn file(&self) -> PathBuf {
            self.path.join(HUD_FILE_NAME)
        }

        fn write(&self, contents: &str) {
            fs::write(self.file(), contents).expect("a writable temporary file");
        }

        fn read(&self) -> String {
            fs::read_to_string(self.file()).expect("the layout file exists")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn the_default_layout_has_eight_bars_of_ten_slots_with_only_the_first_enabled() {
        let config = HudConfig::default();
        assert_eq!(config.bars.len(), ACTION_BARS);
        for (index, bar) in config.bars.iter().enumerate() {
            assert_eq!(bar.slots.len(), ACTION_BAR_SLOTS, "bar {index}");
            assert_eq!(bar.orientation, HudOrientation::Horizontal, "bar {index}");
            assert_eq!(
                bar.placement.enabled,
                index == 0,
                "bar {index} is enabled by default"
            );
        }
        // Every frame a new player gets is on screen; the bars they have not asked for
        // are not.
        let frames = config.frames;
        assert!(frames.player.enabled);
        assert!(frames.target.enabled);
        assert!(frames.target_of_target.enabled);
        assert!(frames.party.enabled);
        assert!(config.is_valid());
    }

    #[test]
    fn the_default_bars_do_not_sit_on_top_of_one_another() {
        // Enabling bar two should put it above bar one rather than through it.
        let config = HudConfig::default();
        for index in 1..ACTION_BARS {
            let above = config.bars[index].placement.offset_y;
            let below = config.bars[index - 1].placement.offset_y;
            assert!(
                above < below,
                "bar {} is not above bar {}",
                index + 1,
                index
            );
        }
    }

    #[test]
    fn a_layout_survives_a_round_trip_through_json() {
        let mut config = HudConfig::default();
        config.bars[0].orientation = HudOrientation::Vertical;
        config.bars[0].placement.anchor = HudAnchor::Left;
        config.bars[0].placement.scale = 1.5;
        config.bars[0].slots[3] = HudSlotConfig::new("meteor", "DIGIT4");
        config.bars[2].placement.enabled = true;
        config.frames.party.enabled = false;
        config.frames.target.offset_x = 0.25;

        let text = serde_json::to_string_pretty(&config).expect("a serializable layout");
        let parsed: HudConfig = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed, config);
    }

    #[test]
    fn a_missing_file_is_created_with_the_defaults() {
        let dir = TempDir::new("missing");
        let manager = HudConfigManager::load(dir.file());

        assert_eq!(manager.config(), &HudConfig::default());
        assert!(!manager.existed(), "nothing was there to load");
        assert!(dir.file().exists(), "the file was not created");
        assert!(dir.read().ends_with('\n'), "no trailing newline");
    }

    #[test]
    fn a_saved_layout_comes_back_on_the_next_launch() {
        let dir = TempDir::new("restart");
        let mut manager = HudConfigManager::load(dir.file());

        let mut config = manager.config().clone();
        config.bars[1].placement.enabled = true;
        config.bars[1].orientation = HudOrientation::Vertical;
        config.bars[1].placement.anchor = HudAnchor::Right;
        config.bars[1].placement.offset_x = -0.05;
        config.bars[1].placement.scale = 1.2;
        config.bars[1].slots[0] = HudSlotConfig::new("blink", "F1");
        config.frames.player.offset_y = 0.3;
        config.frames.party.enabled = false;
        manager.update(config.clone());
        assert!(manager.is_dirty());
        assert!(manager.save_if_dirty());

        let reloaded = HudConfigManager::load(dir.file());
        assert!(reloaded.existed(), "the file was not recognised");
        assert_eq!(reloaded.config(), &config);
    }

    #[test]
    fn a_layout_that_did_not_change_is_not_written_again() {
        let dir = TempDir::new("clean");
        let mut manager = HudConfigManager::load(dir.file());
        assert!(!manager.is_dirty(), "the defaults were just written");

        manager.update(HudConfig::default());
        assert!(!manager.is_dirty(), "an identical layout was marked dirty");
        assert!(!manager.save_if_dirty());
    }

    #[test]
    fn dragging_does_not_touch_the_disk_until_the_move_is_committed() {
        let dir = TempDir::new("drag");
        let mut manager = HudConfigManager::load(dir.file());
        let written = dir.read();

        // Sixty frames of a drag: the layout changes every frame and the file does not.
        for frame in 0..60 {
            let mut config = manager.config().clone();
            config.frames.player.offset_x = frame as f32 * 0.001;
            manager.update(config);
            assert_eq!(dir.read(), written, "frame {frame} wrote to disk");
        }

        assert!(manager.save_if_dirty(), "the commit wrote nothing");
        assert_ne!(dir.read(), written);
    }

    #[test]
    fn a_malformed_file_is_replaced_by_a_valid_one() {
        let dir = TempDir::new("malformed");
        dir.write("{ this is not json");
        let manager = HudConfigManager::load(dir.file());

        assert_eq!(manager.config(), &HudConfig::default());
        assert!(!manager.existed(), "unreadable is not arranged");
        let repaired: HudConfig = serde_json::from_str(&dir.read()).expect("the file was repaired");
        assert_eq!(repaired, HudConfig::default());
    }

    #[test]
    fn an_empty_object_loads_as_the_defaults() {
        let dir = TempDir::new("empty-object");
        dir.write("{}");
        let manager = HudConfigManager::load(dir.file());
        assert_eq!(manager.config(), &HudConfig::default());
        assert!(manager.existed(), "the file parsed, so it was there");
    }

    #[test]
    fn an_unknown_field_is_ignored_and_the_known_ones_still_load() {
        let dir = TempDir::new("unknown-field");
        dir.write(
            r#"{ "future_element": { "enabled": true },
                "frames": { "party": { "enabled": false } } }"#,
        );
        let manager = HudConfigManager::load(dir.file());
        assert!(!manager.config().frames.party.enabled);
        assert_eq!(manager.config().bars.len(), ACTION_BARS);
    }

    #[test]
    fn an_offset_that_is_not_a_position_falls_back_to_the_default() {
        for bad in [400.0, -400.0, f32::NAN, f32::INFINITY] {
            let mut config = HudConfig::default();
            config.frames.player.offset_x = bad;
            let validated = config.validated();
            assert_eq!(
                validated.frames.player.offset_x,
                HudFrames::default().player.offset_x,
                "{bad} was accepted"
            );
            // Both axes go back together: half of a meaningless position is still not one.
            assert_eq!(
                validated.frames.player.offset_y,
                HudFrames::default().player.offset_y
            );
        }
    }

    #[test]
    fn a_scale_outside_its_bounds_falls_back_to_the_default() {
        for bad in [0.0, 0.1, 8.0, -1.0, f32::NAN] {
            let mut config = HudConfig::default();
            config.bars[0].placement.scale = bad;
            let validated = config.validated();
            assert_eq!(
                validated.bars[0].placement.scale, HUD_SCALE_DEFAULT,
                "{bad} was accepted"
            );
        }
        // The bounds themselves are valid.
        for good in [HUD_SCALE_MIN, HUD_SCALE_DEFAULT, HUD_SCALE_MAX] {
            let mut config = HudConfig::default();
            config.bars[0].placement.scale = good;
            assert_eq!(config.validated().bars[0].placement.scale, good);
        }
    }

    #[test]
    fn a_short_or_long_bar_list_is_padded_and_trimmed() {
        let dir = TempDir::new("short-bars");
        dir.write(r#"{ "bars": [ { "orientation": "vertical" } ] }"#);
        let manager = HudConfigManager::load(dir.file());

        assert_eq!(manager.config().bars.len(), ACTION_BARS);
        assert_eq!(
            manager.config().bars[0].orientation,
            HudOrientation::Vertical
        );
        for bar in &manager.config().bars {
            assert_eq!(bar.slots.len(), ACTION_BAR_SLOTS);
        }

        let mut long = HudConfig::default();
        long.bars.push(ActionBarConfig::default());
        assert_eq!(long.validated().bars.len(), ACTION_BARS);
    }

    #[test]
    fn a_bar_with_the_wrong_number_of_slots_is_padded_without_losing_the_ones_it_had() {
        let mut config = HudConfig::default();
        config.bars[0].slots = vec![HudSlotConfig::new("firebolt", "DIGIT1")];
        let validated = config.validated();

        assert_eq!(validated.bars[0].slots.len(), ACTION_BAR_SLOTS);
        assert_eq!(
            validated.bars[0].slots[0],
            HudSlotConfig::new("firebolt", "DIGIT1")
        );
        assert!(validated.bars[0].slots[1].is_empty());
    }

    #[test]
    fn a_key_claimed_twice_is_left_to_the_slot_that_claimed_it_first() {
        let mut config = HudConfig::default();
        config.bars[0].slots[0] = HudSlotConfig::new("firebolt", "DIGIT1");
        config.bars[0].slots[4] = HudSlotConfig::new("meteor", "DIGIT1");
        config.bars[3].slots[7] = HudSlotConfig::new("blink", "DIGIT1");

        let validated = config.validated();
        assert_eq!(validated.bars[0].slots[0].key.as_deref(), Some("DIGIT1"));
        assert_eq!(
            validated.bars[0].slots[4].key, None,
            "the second claim held"
        );
        assert_eq!(validated.bars[3].slots[7].key, None, "the third claim held");
        // Only the binding is dropped. What the slot casts is not in dispute.
        assert_eq!(validated.bars[0].slots[4].spell.as_deref(), Some("meteor"));
    }

    #[test]
    fn a_field_of_the_wrong_type_falls_back_without_taking_the_file_with_it() {
        let dir = TempDir::new("wrong-type");
        dir.write(r#"{ "frames": { "player": { "scale": "big", "enabled": false } } }"#);
        let manager = HudConfigManager::load(dir.file());

        // serde cannot parse the object at all once one field is the wrong type, so the
        // whole file falls back — the same rule the settings loader has, and the reason the
        // file is repaired rather than left to fail again next launch.
        assert_eq!(manager.config(), &HudConfig::default());
        let repaired: HudConfig = serde_json::from_str(&dir.read()).expect("the file is valid now");
        assert_eq!(repaired, HudConfig::default());
    }

    #[test]
    fn an_unrecognised_spell_or_key_name_is_carried_through_untouched() {
        // This crate does not know what a spell is. Resolving a name is the job of the layer
        // that has the registry, and dropping it here would delete a slot that a newer build
        // could read perfectly well.
        let mut config = HudConfig::default();
        config.bars[0].slots[0] = HudSlotConfig::new("frostbolt", "MOUSE4");
        let validated = config.validated();
        assert_eq!(
            validated.bars[0].slots[0],
            HudSlotConfig::new("frostbolt", "MOUSE4")
        );
    }

    #[test]
    fn every_anchor_has_a_distinct_corner_and_survives_json() {
        let mut seen = Vec::new();
        for anchor in HudAnchor::ALL {
            let origin = anchor.origin();
            assert!(
                !seen.contains(&(origin.0.to_bits(), origin.1.to_bits())),
                "{anchor:?} shares a corner"
            );
            seen.push((origin.0.to_bits(), origin.1.to_bits()));
            assert_eq!(anchor.alignment(), origin);
            assert!(!anchor.label().is_empty());

            let text = serde_json::to_string(&anchor).expect("a serializable anchor");
            let parsed: HudAnchor = serde_json::from_str(&text).expect("valid JSON");
            assert_eq!(parsed, anchor);
        }
    }

    #[test]
    fn a_point_is_anchored_to_the_ninth_of_the_screen_it_is_in() {
        assert_eq!(HudAnchor::nearest(0.05, 0.05), HudAnchor::TopLeft);
        assert_eq!(HudAnchor::nearest(0.50, 0.02), HudAnchor::Top);
        assert_eq!(HudAnchor::nearest(0.95, 0.05), HudAnchor::TopRight);
        assert_eq!(HudAnchor::nearest(0.05, 0.50), HudAnchor::Left);
        assert_eq!(HudAnchor::nearest(0.50, 0.50), HudAnchor::Center);
        assert_eq!(HudAnchor::nearest(0.95, 0.50), HudAnchor::Right);
        assert_eq!(HudAnchor::nearest(0.05, 0.95), HudAnchor::BottomLeft);
        assert_eq!(HudAnchor::nearest(0.50, 0.98), HudAnchor::Bottom);
        assert_eq!(HudAnchor::nearest(0.95, 0.95), HudAnchor::BottomRight);
    }

    #[test]
    fn an_orientation_flips_back_to_itself_and_survives_json() {
        for orientation in [HudOrientation::Horizontal, HudOrientation::Vertical] {
            assert_eq!(orientation.flipped().flipped(), orientation);
            assert_ne!(orientation.flipped(), orientation);
            let text = serde_json::to_string(&orientation).expect("a serializable orientation");
            assert_eq!(
                serde_json::from_str::<HudOrientation>(&text).expect("valid JSON"),
                orientation
            );
        }
    }

    #[test]
    fn the_file_is_written_pretty_so_it_can_be_edited_by_hand() {
        let dir = TempDir::new("pretty");
        let _manager = HudConfigManager::load(dir.file());
        let text = dir.read();
        assert!(text.contains("\n  \"bars\""), "not pretty-printed: {text}");
        assert!(text.contains("\"orientation\": \"horizontal\""));
    }
}
