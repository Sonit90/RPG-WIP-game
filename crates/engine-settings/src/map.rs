//! Map settings: how the minimap turns, and which key opens the dungeon map.
//!
//! ```text
//! settings.cfg   "map": { "minimap_orientation": "rotating", "toggle_key": "M" }
//!        |
//! MapSettings    this module: a word the player picked, and a key name
//!        |
//! engine-presentation's minimap reads the orientation every frame; engine-app turns the key
//! name into the binding the map answers.
//! ```
//!
//! Both fields load leniently: a word that is not an orientation, a key that is not a key, or
//! a value of the wrong type each fall back to their default on their own, and never cost the
//! rest of the file.

use serde::{Deserialize, Deserializer, Serialize};

/// How the minimap turns while the player plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MinimapOrientation {
    /// The player stays centred and the map turns, so the way they are looking is always up.
    ///
    /// The default: it is what the genre's minimap does.
    #[default]
    Rotating,
    /// North stays at the top; the player's arrow moves across the map and turns instead.
    NorthUp,
}

impl MinimapOrientation {
    /// Both modes, in the order the settings screen lists them.
    pub const ALL: [Self; 2] = [Self::Rotating, Self::NorthUp];

    /// The word the file uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rotating => "rotating",
            Self::NorthUp => "north_up",
        }
    }

    /// The mode a word names, or `None`. Case-insensitive and trimmed: the file is
    /// hand-edited.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str().eq_ignore_ascii_case(name.trim()))
    }

    /// What the settings screen calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Rotating => "ROTATING - FOLLOW PLAYER",
            Self::NorthUp => "STATIC - NORTH UP",
        }
    }

    /// One line on what it does, for the settings screen.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Rotating => "Keep the player centred and turn the map.",
            Self::NorthUp => "Keep north at the top and turn the player's arrow.",
        }
    }
}

impl<'de> Deserialize<'de> for MinimapOrientation {
    /// Any word that is not a mode — or anything that is not a word — is the default.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let parsed = value.as_str().and_then(Self::from_name);
        if parsed.is_none() {
            log::warn!("settings: map.minimap_orientation {value} is not a mode; using rotating");
        }
        Ok(parsed.unwrap_or_default())
    }
}

/// The key a fresh install opens the dungeon map with: `M`, where the genre puts it.
pub const TOGGLE_MAP_KEY_DEFAULT: &str = "M";

/// A key's written name (`"M"`, `"F10"`, `"NUMPAD5"`), held inline.
///
/// Inline rather than a `String` because [`crate::GameSettings`] is `Copy` — it is handed
/// around by value everywhere a setting is read — and a key name is a handful of ASCII
/// capitals. Sixteen bytes is longer than any name the engine gives a key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct KeyName {
    bytes: [u8; 16],
    len: u8,
}

impl KeyName {
    /// The name, upper-cased and trimmed, or `None` if it is empty, too long or not ASCII.
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() || name.len() > 16 || !name.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(name.bytes()) {
            *slot = byte.to_ascii_uppercase();
        }
        Some(Self {
            bytes,
            len: name.len() as u8,
        })
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or(TOGGLE_MAP_KEY_DEFAULT)
    }
}

impl Default for KeyName {
    fn default() -> Self {
        Self::new(TOGGLE_MAP_KEY_DEFAULT).unwrap_or(Self {
            bytes: [b'M', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            len: 1,
        })
    }
}

impl std::fmt::Debug for KeyName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", self.as_str())
    }
}

impl Serialize for KeyName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for KeyName {
    /// Anything that is not a usable name is the default key.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let parsed = value.as_str().and_then(Self::new);
        if parsed.is_none() {
            log::warn!("settings: map.toggle_key {value} is not a key name; using M");
        }
        Ok(parsed.unwrap_or_default())
    }
}

/// What the player can change about the dungeon map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MapSettings {
    /// How the minimap turns.
    pub minimap_orientation: MinimapOrientation,
    /// The key that opens and closes the full-screen map, by its written name (`"M"`).
    ///
    /// A name rather than a code for the reason `hud.cfg` stores names: reordering the key
    /// enum must not rebind anyone. The application checks it against every other binding
    /// before using it and falls back to [`TOGGLE_MAP_KEY_DEFAULT`] if it collides or is not
    /// a key — this crate knows no keys, so that question is not its to answer.
    pub toggle_key: KeyName,
    /// Whether the map's transitions — hover emphasis, content fades, zoom glides — are
    /// replaced by instant changes.
    pub reduced_motion: bool,
}

impl MapSettings {
    /// These settings with anything unusable replaced by its default.
    ///
    /// Nothing here can be out of range once loaded — both fields fall back on their own —
    /// so this keeps the shape of every other settings section.
    #[must_use]
    pub fn validated(self) -> Self {
        self
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_default_rotates_and_opens_on_m() {
        let settings = MapSettings::default();
        assert_eq!(settings.minimap_orientation, MinimapOrientation::Rotating);
        assert_eq!(settings.toggle_key.as_str(), "M");
    }

    #[test]
    fn both_modes_round_trip_through_the_file_by_name() {
        for mode in MinimapOrientation::ALL {
            let settings = MapSettings {
                minimap_orientation: mode,
                ..MapSettings::default()
            };
            let text = serde_json::to_string(&settings).expect("encodes");
            assert!(text.contains(mode.as_str()), "{text}");
            let back: MapSettings = serde_json::from_str(&text).expect("decodes");
            assert_eq!(back, settings);
        }
    }

    #[test]
    fn a_key_name_is_upper_case_and_bounded() {
        assert_eq!(
            KeyName::new(" f10 ").map(|key| key.as_str().to_owned()),
            Some("F10".to_owned())
        );
        assert!(KeyName::new("").is_none());
        assert!(KeyName::new("A_NAME_FAR_TOO_LONG_TO_BE_A_KEY").is_none());
        assert_eq!(KeyName::default().as_str(), "M");
    }

    #[test]
    fn nonsense_falls_back_field_by_field() {
        let parsed: MapSettings =
            serde_json::from_str("{\"minimap_orientation\": \"sideways\", \"toggle_key\": 7}")
                .expect("one bad field does not fail the section");
        assert_eq!(parsed, MapSettings::default());

        let parsed: MapSettings = serde_json::from_str(
            "{\"minimap_orientation\": \"North_Up\", \"toggle_key\": \" n \"}",
        )
        .expect("decodes");
        assert_eq!(parsed.minimap_orientation, MinimapOrientation::NorthUp);
        assert_eq!(parsed.toggle_key.as_str(), "N");

        let parsed: MapSettings = serde_json::from_str("{}").expect("decodes");
        assert_eq!(parsed, MapSettings::default());
    }
}
