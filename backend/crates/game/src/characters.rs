//! Character rows to API bodies, with everything derived on the server.

use engine_realm::api::{CharacterDetail, CharacterSummary, DungeonCompletion, KeySummary};
use engine_realm::loadout::{gear_score, sheet_of, stats_of};

use crate::store::{CharacterRow, KeyRow};

/// The definition's hero, or the default for a definition this build no longer has.
fn hero_of(row: &CharacterRow) -> engine_game::hero::Hero {
    crate::catalog()
        .character(&row.definition_id)
        .map(|definition| definition.hero)
        .unwrap_or_default()
}

/// The summary the selection screen lists. Gear score computed here, from the stored loadout.
pub fn summary(row: &CharacterRow, key: Option<&KeyRow>) -> CharacterSummary {
    let definition = crate::catalog().character(&row.definition_id);
    let hero = hero_of(row);
    let sheet = sheet_of(hero, &row.loadout.0);
    CharacterSummary {
        id: row.id.to_string(),
        definition_id: row.definition_id.clone(),
        name: row.name.clone(),
        class: definition.map(|d| d.class.clone()).unwrap_or_default(),
        roles: definition.map(|d| d.roles.clone()).unwrap_or_default(),
        level: u32::try_from(row.level).unwrap_or(1),
        experience: u64::try_from(row.experience).unwrap_or(0),
        gear_score: gear_score(hero, &row.loadout.0),
        equipment: engine_realm::loadout::loadout_of(&sheet).equipment,
        key: key.map(|key| KeySummary {
            dungeon_id: key.dungeon_id.clone(),
            dungeon_name: crate::catalog()
                .dungeon(&key.dungeon_id)
                .map_or_else(|| key.dungeon_id.clone(), |dungeon| dungeon.name.clone()),
            level: key.level,
        }),
        // A BTreeMap: already sorted by id.
        resources: row
            .loadout
            .0
            .resources
            .iter()
            .map(|(id, amount)| (id.clone(), *amount))
            .collect(),
    }
}

/// Everything about one character.
pub fn detail(
    row: &CharacterRow,
    completions: Vec<DungeonCompletion>,
    key: Option<&KeyRow>,
) -> CharacterDetail {
    let hero = hero_of(row);
    let sheet = sheet_of(hero, &row.loadout.0);
    CharacterDetail {
        summary: summary(row, key),
        inventory: engine_realm::loadout::loadout_of(&sheet).inventory,
        mail: row.loadout.0.mail.clone(),
        talents: row.talents.0.clone(),
        stats: stats_of(&sheet),
        completions,
    }
}
