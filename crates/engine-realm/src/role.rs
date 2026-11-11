//! The three jobs a party is made of.

use serde::{Deserialize, Serialize};

/// What a character does in a party.
///
/// A party is exactly one of each ([`Role::ALL`]). The order is the order the interface lists
/// them in and the order a party is written down in, so it is stable on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Holds the enemy's attention and takes the hits.
    Tank,
    /// Keeps the party alive.
    Healer,
    /// Kills things.
    Dps,
}

impl Role {
    /// Every role, in party order.
    pub const ALL: [Self; 3] = [Self::Tank, Self::Healer, Self::Dps];

    /// The stable written name: what the catalog, the API and the module store.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Tank => "tank",
            Self::Healer => "healer",
            Self::Dps => "dps",
        }
    }

    /// The role with this written name.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|role| role.key() == key)
    }

    /// The display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Tank => "Tank",
            Self::Healer => "Healer",
            Self::Dps => "DPS",
        }
    }

    /// Dense index, for a per-role array.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Tank => 0,
            Self::Healer => 1,
            Self::Dps => 2,
        }
    }

    /// The role at `index`.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::from_key(role.key()), Some(role));
            assert_eq!(Role::from_index(role.index()), Some(role));
        }
        assert_eq!(Role::from_key("bard"), None);
    }
}
