//! Dungeon keys: what a key is for, whether it is valid, how it grows and what it scales.
//!
//! ```text
//! key (dungeon, level) ──insert──▶ party ──▶ monsters scale by [`scaling`]
//!         ▲                                      │ success
//!         └────────── [`progress`] ◀─────────────┘
//! ```
//!
//! A key is one per character, stored by the persistence API; the realm copies a snapshot of
//! it when the character queues. Nothing here trusts a level a client sends: the snapshot is
//! checked against the dungeon's [`KeyRules`] before it is used.

use crate::catalog::{DungeonDefinition, KeyRules};
use crate::error::RealmError;

/// A character's key as the queue snapshotted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySnapshot {
    /// The dungeon it opens.
    pub dungeon_id: String,
    /// Its level.
    pub level: u32,
}

impl KeySnapshot {
    /// Whether it opens `dungeon` at a level the dungeon's rules allow.
    #[must_use]
    pub fn is_valid_for(&self, dungeon: &DungeonDefinition) -> bool {
        validate_key(dungeon, &self.dungeon_id, self.level).is_ok()
    }
}

/// The level of a key after a run: one higher on success (never above the maximum), the same
/// on failure. Always within the rules, whatever `level` was.
#[must_use]
pub fn progress(rules: &KeyRules, level: u32, success: bool) -> u32 {
    let next = if success {
        level.saturating_add(1)
    } else {
        level
    };
    next.clamp(rules.min_level, rules.max_level)
}

/// A party member's own key after a run they did not open: one level up, once, when the run
/// was a success and the key they brought was lower than the one used.
///
/// `None` for the owner (their key follows [`progress`]), for a failed run, for a member whose
/// key is already as high as the used one, and for a snapshot with no valid key
/// (`own_level` outside the rules). The result is within the rules whatever came in. The
/// persistence API applies it with a compare-and-set on `own_level`, so a key never rises
/// twice for one run.
#[must_use]
pub fn participant_progress(
    rules: &KeyRules,
    used_level: u32,
    own_level: u32,
    success: bool,
    is_owner: bool,
) -> Option<u32> {
    if !success
        || is_owner
        || own_level < rules.min_level
        || own_level > rules.max_level
        || own_level >= used_level
    {
        return None;
    }
    Some(progress(rules, own_level, true))
}

/// Whether a key for `key_dungeon_id` at `level` may be used in `dungeon`.
///
/// # Errors
///
/// A party error saying the key is for another dungeon, or is not valid (a level outside the
/// dungeon's rules).
pub fn validate_key(
    dungeon: &DungeonDefinition,
    key_dungeon_id: &str,
    level: u32,
) -> Result<(), RealmError> {
    if key_dungeon_id != dungeon.id {
        return Err(RealmError::party("That key is for another dungeon."));
    }
    if level < dungeon.keys.min_level || level > dungeon.keys.max_level {
        return Err(RealmError::party("That key is not valid."));
    }
    Ok(())
}

/// What a key of `level` multiplies monster `(health, damage)` by: `1 + per_level × (level − 1)`.
#[must_use]
pub fn scaling(rules: &KeyRules, level: u32) -> (f32, f32) {
    let steps = level.saturating_sub(1) as f32;
    (
        1.0 + rules.health_per_level * steps,
        1.0 + rules.damage_per_level * steps,
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn crypt() -> DungeonDefinition {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .dungeon("graveyard_crypt")
            .expect("the crypt")
            .clone()
    }

    #[test]
    fn success_raises_the_key_and_stops_at_the_maximum() {
        let rules = crypt().keys;
        assert_eq!(progress(&rules, 1, true), 2);
        assert_eq!(progress(&rules, rules.max_level - 1, true), rules.max_level);
        assert_eq!(progress(&rules, rules.max_level, true), rules.max_level);
    }

    #[test]
    fn failure_leaves_the_key_and_the_result_is_always_in_range() {
        let rules = crypt().keys;
        assert_eq!(progress(&rules, 7, false), 7);
        assert_eq!(progress(&rules, 0, false), rules.min_level);
        assert_eq!(progress(&rules, 0, true), rules.min_level);
        assert_eq!(progress(&rules, 999, false), rules.max_level);
        assert_eq!(progress(&rules, u32::MAX, true), rules.max_level);
    }

    #[test]
    fn a_participant_with_a_lower_key_rises_one_level_on_success_only() {
        let rules = crypt().keys;
        assert_eq!(participant_progress(&rules, 5, 2, true, false), Some(3));
        assert_eq!(participant_progress(&rules, 5, 4, true, false), Some(5));
        // Equal or higher keys, the owner and failures stay put.
        assert_eq!(participant_progress(&rules, 5, 5, true, false), None);
        assert_eq!(participant_progress(&rules, 5, 9, true, false), None);
        assert_eq!(participant_progress(&rules, 5, 2, true, true), None);
        assert_eq!(participant_progress(&rules, 5, 2, false, false), None);
        // No key (level 0) or an impossible one earns nothing.
        assert_eq!(participant_progress(&rules, 5, 0, true, false), None);
        assert_eq!(participant_progress(&rules, 99, 999, true, false), None);
    }

    #[test]
    fn a_key_must_match_the_dungeon_and_the_rules() {
        let dungeon = crypt();
        assert!(validate_key(&dungeon, "graveyard_crypt", 1).is_ok());
        assert!(validate_key(&dungeon, "graveyard_crypt", dungeon.keys.max_level).is_ok());
        let other = validate_key(&dungeon, "ashen_reliquary", 1).expect_err("another dungeon");
        assert_eq!(other.message, "That key is for another dungeon.");
        for level in [0, dungeon.keys.max_level + 1] {
            let error = validate_key(&dungeon, "graveyard_crypt", level).expect_err("range");
            assert_eq!(error.message, "That key is not valid.");
        }
        let key = KeySnapshot {
            dungeon_id: "graveyard_crypt".into(),
            level: 3,
        };
        assert!(key.is_valid_for(&dungeon));
        assert!(!KeySnapshot { level: 0, ..key }.is_valid_for(&dungeon));
    }

    #[test]
    fn scaling_grows_linearly_from_level_one() {
        let rules = KeyRules {
            min_level: 1,
            max_level: 20,
            health_per_level: 0.10,
            damage_per_level: 0.05,
        };
        assert_eq!(scaling(&rules, 1), (1.0, 1.0));
        let (health, damage) = scaling(&rules, 11);
        assert!((health - 2.0).abs() < 1e-5);
        assert!((damage - 1.5).abs() < 1e-5);
        assert_eq!(scaling(&rules, 0), (1.0, 1.0));
    }
}
