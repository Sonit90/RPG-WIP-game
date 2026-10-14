//! Item keys and talent keys, resolved to what an operator wants to read.

use engine_game::character::{item_by_key, Stat};
use engine_realm::loadout::EquippedItem;
use serde::Serialize;

/// One stat an item grants.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatValue {
    pub stat: String,
    pub value: f32,
}

/// A worn item, resolved against the item table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GearItem {
    pub slot: String,
    pub key: String,
    /// The display name; the key itself when this build does not know the item.
    pub name: String,
    pub quality: Option<String>,
    pub item_level: Option<u16>,
    pub stats: Vec<StatValue>,
}

/// A bought talent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TalentView {
    pub key: String,
    pub name: String,
}

/// Resolves the worn items. An unknown key stays listed, unresolved.
#[must_use]
pub fn resolve_gear(items: &[EquippedItem]) -> Vec<GearItem> {
    items
        .iter()
        .map(|worn| match item_by_key(&worn.item) {
            Some(def) => GearItem {
                slot: worn.slot.clone(),
                key: worn.item.clone(),
                name: def.name.to_owned(),
                quality: Some(def.quality.name().to_owned()),
                item_level: Some(def.item_level),
                stats: Stat::ALL
                    .into_iter()
                    .filter_map(|stat| {
                        let value = def.stats.get(stat);
                        (value != 0.0).then(|| StatValue {
                            stat: stat.key().to_owned(),
                            value,
                        })
                    })
                    .collect(),
            },
            None => GearItem {
                slot: worn.slot.clone(),
                key: worn.item.clone(),
                name: worn.item.clone(),
                quality: None,
                item_level: None,
                stats: Vec::new(),
            },
        })
        .collect()
}

/// Resolves talent keys for the class `definition_id` names. Unknown keys keep their key as
/// the name.
#[must_use]
pub fn resolve_talents(definition_id: &str, keys: &[String]) -> Vec<TalentView> {
    let available = realm_game::catalog()
        .character(definition_id)
        .map(|definition| engine_realm::talents::talents_for(definition.hero))
        .unwrap_or_default();
    keys.iter()
        .map(|key| TalentView {
            key: key.clone(),
            name: available
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map_or_else(|| key.clone(), |(_, def)| def.name.to_owned()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_item_stays_listed_by_key() {
        let gear = resolve_gear(&[EquippedItem {
            slot: "head".into(),
            item: "no_such_item".into(),
        }]);
        assert_eq!(gear[0].name, "no_such_item");
        assert!(gear[0].quality.is_none());
    }

    #[test]
    fn a_known_item_resolves_its_name() {
        let def = engine_game::character::ITEMS[0];
        let gear = resolve_gear(&[EquippedItem {
            slot: "chest".into(),
            item: def.key.into(),
        }]);
        assert_eq!(gear[0].name, def.name);
        assert_eq!(gear[0].item_level, Some(def.item_level));
    }

    #[test]
    fn talent_keys_resolve_for_the_fire_mage_and_pass_through_otherwise() {
        let mage = realm_game::catalog()
            .characters()
            .iter()
            .find(|c| !engine_realm::talents::talents_for(c.hero).is_empty())
            .expect("a class with talents");
        let (key, def) = engine_realm::talents::talents_for(mage.hero).remove(0);
        let resolved = resolve_talents(&mage.id, &[key.clone(), "mystery".into()]);
        assert_eq!(resolved[0].name, def.name);
        assert_eq!(resolved[1].name, "mystery");
        assert_eq!(resolve_talents("no_class", std::slice::from_ref(&key))[0].name, key);
    }
}
