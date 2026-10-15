//! Talents for persistence and the server: the shipped table, stable keys, validation.
//!
//! ```text
//! assets/catalog/talents.json ─► parse ─► TalentCatalog ─► engine_game::talent::install
//!                                                              │
//!   API (save talents, snapshot)  realm (build the fight)  client (menu)  simulator
//! ```
//!
//! `engine-game` owns what a talent *does* and keeps no serde; this module reads the numbers
//! it does it with from JSON, installs them once per program, and gives the API and the realm
//! the one validation both run: a saved character's keys are accepted only if every key names
//! a talent of its hero and the whole set costs no more than the points the character has.

use std::collections::BTreeMap;

use engine_game::hero::Hero;
use engine_game::talent::{
    HeroTalents, TalentCatalog, TalentGrid, TalentProgression, TalentSpec, TalentValues,
    hero_talents, progression,
};
use serde::Deserialize;

pub use engine_game::talent::TalentError;

use crate::catalog::CatalogError;

/// The talent catalog as written.
const TALENTS_JSON: &str = include_str!("../../../assets/catalog/talents.json");

#[derive(Deserialize)]
struct TalentFile {
    version: u32,
    progression: ProgressionRecord,
    heroes: BTreeMap<String, HeroRecord>,
}

#[derive(Deserialize)]
struct ProgressionRecord {
    starting_points: u32,
    points_per_completion: u32,
    bonus_points_min_key_level: u32,
    bonus_points: u32,
}

#[derive(Deserialize)]
struct HeroRecord {
    talents: Vec<TalentRecord>,
}

#[derive(Deserialize)]
struct TalentRecord {
    key: String,
    name: String,
    tier: u8,
    cost: u32,
    description: String,
    #[serde(default)]
    values: BTreeMap<String, f64>,
}

/// The talent catalog from its JSON text.
///
/// # Errors
///
/// Malformed JSON, an unknown version or hero, or a table the game cannot run: talents that
/// differ from the code's, a missing number, a bad tier or cost.
pub fn parse(text: &str) -> Result<TalentCatalog, CatalogError> {
    let file: TalentFile = serde_json::from_str(text)
        .map_err(|error| CatalogError(format!("talents.json: {error}")))?;
    if file.version != crate::catalog::CATALOG_VERSION {
        return Err(CatalogError(format!(
            "this build reads catalog version {}",
            crate::catalog::CATALOG_VERSION
        )));
    }
    let mut catalog = TalentCatalog {
        progression: TalentProgression {
            starting_points: file.progression.starting_points,
            points_per_completion: file.progression.points_per_completion,
            bonus_points_min_key_level: file.progression.bonus_points_min_key_level,
            bonus_points: file.progression.bonus_points,
        },
        heroes: [
            HeroTalents::default(),
            HeroTalents::default(),
            HeroTalents::default(),
        ],
    };
    for (name, record) in file.heroes {
        let hero = Hero::from_config_name(&name).ok_or_else(|| {
            CatalogError(format!(
                "talents.json names hero {name:?}, which does not exist"
            ))
        })?;
        catalog.heroes[hero.index()] = HeroTalents {
            talents: record
                .talents
                .into_iter()
                .map(|talent| {
                    #[allow(clippy::cast_possible_truncation, reason = "talent numbers are small")]
                    let values = TalentValues::new(
                        talent
                            .values
                            .into_iter()
                            .map(|(name, value)| (name, value as f32)),
                    );
                    TalentSpec::new(
                        &talent.key,
                        &talent.name,
                        talent.tier,
                        talent.cost,
                        &talent.description,
                        values,
                    )
                })
                .collect(),
        };
    }
    catalog
        .validate()
        .map_err(|message| CatalogError(format!("talents.json: {message}")))?;
    Ok(catalog)
}

/// The talent catalog this build was compiled with.
///
/// # Errors
///
/// Only if the shipped JSON is broken, which `cargo test` catches before anything ships.
pub fn builtin() -> Result<TalentCatalog, CatalogError> {
    parse(TALENTS_JSON)
}

/// Installs the shipped talents into `engine-game`, once. Safe to call again.
///
/// [`crate::catalog::Catalog::builtin`] does this, so every program that reads a catalog has
/// the shipped numbers; a program that does not can call it directly.
///
/// # Errors
///
/// Only if the shipped JSON is broken.
pub fn install_builtin() -> Result<(), CatalogError> {
    engine_game::talent::install(builtin()?)
        .map_err(|message| CatalogError(format!("talents.json: {message}")))
}

/// Every talent `hero` can buy, with its key.
#[must_use]
pub fn talents_for(hero: Hero) -> Vec<(String, &'static TalentSpec)> {
    hero_talents(hero)
        .talents
        .iter()
        .map(|spec| (spec.key.clone(), spec))
        .collect()
}

/// Whether `key` names a talent `hero` can have.
#[must_use]
pub fn is_valid(hero: Hero, key: &str) -> bool {
    hero_talents(hero).index_of(key).is_some()
}

/// Talent points a character that has earned `earned` has in all.
#[must_use]
pub fn total_points(earned: u32) -> u32 {
    progression().total_points(earned)
}

/// The grid a saved set of `keys` makes, for a character that has earned `earned` points.
///
/// The one check the API runs before saving and the realm runs before building a fight.
///
/// # Errors
///
/// [`TalentError::UnknownTalent`] for a key the hero lacks, [`TalentError::NotEnoughPoints`]
/// when the set costs more than the character has.
pub fn validate(hero: Hero, keys: &[String], earned: u32) -> Result<TalentGrid, TalentError> {
    TalentGrid::from_keys(hero, keys, total_points(earned))
}

/// What one run pays in talent points. Nothing for a failure.
#[must_use]
pub fn points_for_run(success: bool, key_level: u32) -> u32 {
    progression().points_for_run(success, key_level)
}

/// The talent keys a snapshot carries, as the one string the realm's tables store.
///
/// Comma-separated: keys are lowercase words and underscores.
#[must_use]
pub fn encode_keys(keys: &[String]) -> String {
    keys.join(",")
}

/// The keys in a stored snapshot string.
#[must_use]
pub fn decode_keys(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_game::firemage::TalentEffects;
    use engine_game::talent::TalentKey;
    use engine_game::talent::druid::{DruidTalent, DruidTalentEffects};
    use engine_game::talent::warrior::{WarriorTalent, WarriorTalentEffects};

    #[test]
    fn the_shipped_json_is_exactly_the_numbers_the_code_defaults_to() {
        // The data move changes nothing: same names, tiers, costs, descriptions and values.
        assert_eq!(builtin().expect("parses"), TalentCatalog::default());
    }

    #[test]
    fn json_to_effects_covers_every_talent_of_every_hero() {
        let catalog = builtin().expect("parses");
        for hero in Hero::ALL {
            let table = &catalog.heroes[hero.index()];
            for (index, spec) in table.talents.iter().enumerate() {
                assert!(!spec.description.contains('{'), "{}", spec.description);
                let mut grid = TalentGrid::with_points(hero, 999);
                grid.purchase_index(index).expect("buys");
                match hero {
                    Hero::FireMage => assert_ne!(
                        TalentEffects::from_grid(&grid),
                        TalentEffects::default(),
                        "{}",
                        spec.key
                    ),
                    Hero::Druid => assert_ne!(
                        DruidTalentEffects::from_grid(&grid),
                        DruidTalentEffects::default(),
                        "{}",
                        spec.key
                    ),
                    Hero::Warrior => assert_ne!(
                        WarriorTalentEffects::from_grid(&grid),
                        WarriorTalentEffects::default(),
                        "{}",
                        spec.key
                    ),
                }
            }
        }
    }

    #[test]
    fn the_druid_talents_are_priced_by_tier() {
        let catalog = builtin().expect("parses");
        for (talent, tier, cost) in [
            (DruidTalent::SwiftWrath, 1, 3),
            (DruidTalent::VerdantSunfire, 1, 3),
            (DruidTalent::DeepExposure, 2, 4),
            (DruidTalent::LivingSeed, 2, 4),
            (DruidTalent::Overgrowth, 3, 4),
            (DruidTalent::AstralAttunement, 3, 4),
            (DruidTalent::BarkskinInstinct, 4, 5),
            (DruidTalent::TranquilMind, 4, 5),
        ] {
            let spec = &catalog.heroes[Hero::Druid.index()].talents[TalentKey::index(talent)];
            assert_eq!((spec.tier, spec.cost), (tier, cost), "{}", spec.key);
        }
    }

    #[test]
    fn the_warrior_talents_are_priced_by_tier() {
        let catalog = builtin().expect("parses");
        for (talent, tier, cost) in [
            (WarriorTalent::UnbrokenMomentum, 1, 3),
            (WarriorTalent::Shockwave, 1, 3),
            (WarriorTalent::Executioner, 2, 4),
            (WarriorTalent::BloodiedValor, 2, 4),
            (WarriorTalent::FerventGuard, 3, 4),
            (WarriorTalent::RelentlessCharge, 3, 4),
            (WarriorTalent::BulwarkRhythm, 4, 5),
            (WarriorTalent::Unyielding, 4, 5),
        ] {
            let spec = &catalog.heroes[Hero::Warrior.index()].talents[TalentKey::index(talent)];
            assert_eq!((spec.tier, spec.cost), (tier, cost), "{}", spec.key);
        }
    }

    #[test]
    fn a_table_that_disagrees_with_the_code_is_refused() {
        let broken = TALENTS_JSON.replace("\"swift_wrath\"", "\"swift_wrathful\"");
        assert!(parse(&broken).is_err());
        let missing = TALENTS_JSON.replace("\"astral_multiplier\"", "\"astral_mult\"");
        assert!(parse(&missing).is_err());
    }

    #[test]
    fn keys_are_unique_and_round_trip() {
        for hero in [Hero::FireMage, Hero::Druid] {
            let all = talents_for(hero);
            let mut keys: Vec<&String> = all.iter().map(|(key, _)| key).collect();
            keys.sort();
            keys.dedup();
            assert_eq!(keys.len(), all.len());
            for (key, _) in &all {
                assert!(is_valid(hero, key));
                assert!(!is_valid(Hero::Warrior, key));
            }
        }
        assert!(is_valid(Hero::Warrior, "shockwave"));
        assert!(!is_valid(Hero::Druid, "shockwave"));
        assert!(!is_valid(Hero::FireMage, "infinite_damage"));
        assert!(!is_valid(Hero::FireMage, "swift_wrath"));
        assert_eq!(
            decode_keys(&encode_keys(&["a".into(), "b".into()])),
            ["a", "b"]
        );
        assert!(decode_keys("").is_empty());
    }

    #[test]
    fn a_set_is_accepted_only_within_the_characters_points() {
        let keys = vec!["swift_wrath".to_owned()];
        // Five points to start: one 3-point talent fits, two do not.
        assert!(validate(Hero::Druid, &keys, 0).is_ok());
        let two = vec!["swift_wrath".to_owned(), "verdant_sunfire".to_owned()];
        assert_eq!(
            validate(Hero::Druid, &two, 0),
            Err(TalentError::NotEnoughPoints)
        );
        // One earned point makes six: enough.
        assert!(validate(Hero::Druid, &two, 1).is_ok());
        assert_eq!(
            validate(Hero::Druid, &["nope".to_owned()], 99),
            Err(TalentError::UnknownTalent)
        );
    }

    #[test]
    fn a_run_pays_points_once_per_success_with_a_bonus_for_high_keys() {
        assert_eq!(points_for_run(false, 10), 0);
        assert_eq!(points_for_run(true, 1), 1);
        assert_eq!(points_for_run(true, 2), 1);
        assert_eq!(points_for_run(true, 3), 2);
    }
}
