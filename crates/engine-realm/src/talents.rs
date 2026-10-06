//! Talent identities for persistence: stable keys derived from the talent table.
//!
//! The talent table (`engine_game::firemage::talent::TALENTS`) names each talent for display.
//! A saved character stores a *key* — the name in `snake_case` — so the API can validate a
//! talent list without trusting the client and the client can restore it after loading.
//! Only the Fire Mage has a talent tree today; every other hero accepts an empty list.

use engine_game::firemage::talent::{TALENTS, TalentDef, TalentId};
use engine_game::hero::Hero;

/// The stable key of a talent: its display name, lowercased, words joined by underscores.
#[must_use]
pub fn key_of(def: &TalentDef) -> String {
    def.name
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("_")
}

/// Every talent `hero` can buy, with its key.
#[must_use]
pub fn talents_for(hero: Hero) -> Vec<(String, &'static TalentDef)> {
    match hero {
        Hero::FireMage => TALENTS.iter().map(|def| (key_of(def), def)).collect(),
        Hero::Warrior | Hero::Druid => Vec::new(),
    }
}

/// Whether `key` names a talent `hero` can have.
#[must_use]
pub fn is_valid(hero: Hero, key: &str) -> bool {
    talents_for(hero)
        .iter()
        .any(|(candidate, _)| candidate == key)
}

/// The talent a key names, for `hero`.
#[must_use]
pub fn talent_of(hero: Hero, key: &str) -> Option<TalentId> {
    talents_for(hero)
        .into_iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, def)| def.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_round_trip() {
        let all = talents_for(Hero::FireMage);
        let mut keys: Vec<&String> = all.iter().map(|(key, _)| key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), all.len());
        for (key, def) in &all {
            assert!(is_valid(Hero::FireMage, key));
            assert_eq!(talent_of(Hero::FireMage, key), Some(def.id));
            assert!(!is_valid(Hero::Warrior, key));
        }
        assert!(!is_valid(Hero::FireMage, "infinite_damage"));
    }
}
