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

/// Talent points the character has earned from runs.
pub fn earned_points(row: &CharacterRow) -> u32 {
    u32::try_from(row.talent_points_earned).unwrap_or(0)
}

/// Talent points its stored talents cost. A stored set that no longer parses costs nothing.
pub fn spent_points(row: &CharacterRow, hero: engine_game::hero::Hero) -> u32 {
    let table = engine_game::talent::hero_talents(hero);
    row.talents
        .0
        .iter()
        .filter_map(|key| table.index_of(key).map(|index| table.talents[index].cost))
        .sum()
}

/// The hero a character plays.
pub fn hero(row: &CharacterRow) -> engine_game::hero::Hero {
    hero_of(row)
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
            modifiers: key.modifiers.clone(),
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
        talent_points_total: engine_realm::talents::total_points(earned_points(row)),
        talent_points_available: engine_realm::talents::total_points(earned_points(row))
            .saturating_sub(spent_points(row, hero)),
        stats: stats_of(&sheet),
        completions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::Json;
    use uuid::Uuid;

    fn druid(talents: &[&str], earned: i32) -> CharacterRow {
        CharacterRow {
            id: Uuid::nil(),
            account_id: Uuid::nil(),
            definition_id: "druid".into(),
            name: "Druid".into(),
            level: 1,
            experience: 0,
            loadout: Json(engine_realm::loadout::starting_loadout(
                engine_game::hero::Hero::Druid,
            )),
            talents: Json(talents.iter().map(|key| (*key).to_owned()).collect()),
            talent_points_earned: earned,
        }
    }

    #[test]
    fn available_points_are_the_starting_points_plus_earned_minus_spent() {
        let row = druid(&["swift_wrath"], 2);
        let detail = detail(&row, vec![], None);
        assert_eq!(detail.talent_points_total, 5 + 2);
        assert_eq!(detail.talent_points_available, 5 + 2 - 3);
        assert_eq!(detail.talents, ["swift_wrath"]);
    }

    #[test]
    fn a_fresh_character_has_only_the_starting_points() {
        let detail = detail(&druid(&[], 0), vec![], None);
        assert_eq!(detail.talent_points_total, 5);
        assert_eq!(detail.talent_points_available, 5);
    }
}
