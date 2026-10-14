//! What characters and dungeons exist: data, not code.
//!
//! ```text
//! assets/catalog/characters.json ─┐
//! assets/catalog/dungeons.json  ──┼─► Catalog ─► module (validation), client (screens),
//! assets/catalog/economy.json   ──┘              persistence API (starter characters)
//! ```
//!
//! All three files are compiled into every program that reads them ([`Catalog::builtin`]), so the
//! server, the client and the API cannot disagree about which dungeons exist: they were built
//! from the same bytes. Adding a dungeon or a character is a row in a JSON file and nothing
//! anywhere iterates a hardcoded list of three.
//!
//! A definition is the *template*: what a Warrior is. The persistent character a player owns
//! (level, experience, gear) lives in the persistence API and names its definition by
//! [`CharacterDefinition::id`].

use engine_game::hero::Hero;
use serde::Deserialize;

use crate::role::Role;

/// The character catalog as written.
const CHARACTERS_JSON: &str = include_str!("../../../assets/catalog/characters.json");
/// The dungeon catalog as written.
const DUNGEONS_JSON: &str = include_str!("../../../assets/catalog/dungeons.json");
/// The economy catalog as written.
const ECONOMY_JSON: &str = include_str!("../../../assets/catalog/economy.json");

/// The catalog format this build reads.
pub const CATALOG_VERSION: u32 = 1;

/// A playable character template.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterDefinition {
    /// Stable identifier. What a persistent character stores.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Class name, for the selection screen.
    pub class: String,
    /// The gameplay kit: abilities, base stats and model all come from the hero.
    pub hero: Hero,
    /// Which roles it may queue as. Never empty.
    pub roles: Vec<Role>,
    /// Whether a new account is given one.
    pub starter: bool,
    /// One or two sentences for the selection screen.
    pub description: String,
}

impl CharacterDefinition {
    /// Whether it may queue as `role`.
    #[must_use]
    pub fn can_play(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }
}

/// How hard a dungeon is. Shown, and later used to scale rewards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    /// The ordinary version.
    Normal,
    /// Harder, for geared parties.
    Heroic,
}

impl Difficulty {
    /// Display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Heroic => "Heroic",
        }
    }
}

/// Where a party member appears.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct StartPoint {
    /// Feet position.
    pub position: [f32; 3],
    /// Facing, radians about +Y. Zero faces −Z.
    pub yaw: f32,
}

/// An axis-aligned box: the starting barrier's collision.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct BarrierBox {
    /// Centre.
    pub center: [f32; 3],
    /// Half the size on each axis.
    pub half_extents: [f32; 3],
}

impl BarrierBox {
    /// Its minimum corner.
    #[must_use]
    pub fn min(&self) -> [f32; 3] {
        [
            self.center[0] - self.half_extents[0],
            self.center[1] - self.half_extents[1],
            self.center[2] - self.half_extents[2],
        ]
    }

    /// Its maximum corner.
    #[must_use]
    pub fn max(&self) -> [f32; 3] {
        [
            self.center[0] + self.half_extents[0],
            self.center[1] + self.half_extents[1],
            self.center[2] + self.half_extents[2],
        ]
    }
}

/// How a dungeon key's level changes the run.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct KeyRules {
    /// The lowest key level. A new key starts here.
    pub min_level: u32,
    /// The highest key level a key can be raised to.
    pub max_level: u32,
    /// Monster health grows by this fraction of the base for every level above the first.
    pub health_per_level: f32,
    /// Monster damage grows by this fraction of the base for every level above the first.
    pub damage_per_level: f32,
}

/// One resource a run pays out: `amount` at key level 1, plus `per_level` for every level above.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResourceReward {
    /// The resource's id in the economy catalog.
    pub id: String,
    /// Paid at the first key level.
    pub amount: u32,
    /// Added for every key level above the first.
    #[serde(default)]
    pub per_level: u32,
}

/// What finishing a dungeon pays, before the key level scales it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct RewardTable {
    /// Experience at the first key level.
    #[serde(default)]
    pub experience: u32,
    /// Experience added for every key level above the first.
    #[serde(default)]
    pub experience_per_level: u32,
    /// The resources paid.
    #[serde(default)]
    pub resources: Vec<ResourceReward>,
}

/// A dungeon a party can queue for.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonDefinition {
    /// Stable identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// A paragraph for the selection screen.
    pub description: String,
    /// The level file, relative to the assets root.
    pub scene: String,
    /// How hard it is.
    pub difficulty: Difficulty,
    /// Lowest character level admitted.
    pub min_level: u32,
    /// Highest character level admitted.
    pub max_level: u32,
    /// Lowest gear score admitted.
    pub min_gear_score: u32,
    /// The gear score it is tuned for. Shown, not enforced.
    pub recommended_gear_score: u32,
    /// What finishing it can give, for the selection screen.
    pub rewards: Vec<String>,
    /// How its keys scale the run.
    pub keys: KeyRules,
    /// What finishing it pays, per key level.
    pub reward_table: RewardTable,
    /// Seconds between the leader starting and the barrier lifting.
    pub countdown_seconds: u32,
    /// Where each party member appears, in party order. At least one per role.
    pub spawn_points: Vec<StartPoint>,
    /// The wall that holds the party in the start area during the countdown.
    pub start_barrier: BarrierBox,
}

impl DungeonDefinition {
    /// Why a character of `level` with `gear_score` may not enter, if it may not.
    ///
    /// # Errors
    ///
    /// A matchmaking error naming the requirement that was not met.
    pub fn admits(&self, level: u32, gear_score: u32) -> Result<(), crate::RealmError> {
        if level < self.min_level || level > self.max_level {
            return Err(crate::RealmError::matchmaking(format!(
                "{} is for levels {} to {}.",
                self.name, self.min_level, self.max_level
            )));
        }
        if gear_score < self.min_gear_score {
            return Err(crate::RealmError::matchmaking(format!(
                "{} requires a gear score of at least {}.",
                self.name, self.min_gear_score
            )));
        }
        Ok(())
    }

    /// Where the member in party slot `index` appears. Wraps if there are more members than
    /// points, so a catalog row with one point still places everyone.
    #[must_use]
    pub fn spawn_point(&self, index: usize) -> StartPoint {
        self.spawn_points
            .get(index % self.spawn_points.len().max(1))
            .copied()
            .unwrap_or(StartPoint {
                position: [0.0; 3],
                yaw: 0.0,
            })
    }
}

/// A kind of resource a character can hold.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ResourceDefinition {
    /// Stable identifier. What a loadout's resource map is keyed by.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One sentence for the tooltip.
    #[serde(default)]
    pub description: String,
}

/// What breaking an item down yields.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SalvageRule {
    /// The resource it yields.
    pub resource: String,
    /// Yielded for any item.
    pub base: u32,
    /// Added for every item level of the item.
    pub per_item_level: u32,
}

/// A fixed-rate trade of one resource for another.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExchangeRule {
    /// Stable identifier.
    pub id: String,
    /// The resource paid.
    pub from: String,
    /// How much of it one trade costs.
    pub from_amount: u32,
    /// The resource received.
    pub to: String,
    /// How much of it one trade gives.
    pub to_amount: u32,
}

/// Resources, salvage and exchanges.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Economy {
    version: u32,
    /// Every resource that exists.
    pub resources: Vec<ResourceDefinition>,
    /// What salvaging an item yields.
    pub salvage: SalvageRule,
    /// Every trade on offer.
    pub exchanges: Vec<ExchangeRule>,
}

impl Economy {
    /// The resource with this id.
    #[must_use]
    pub fn resource(&self, id: &str) -> Option<&ResourceDefinition> {
        self.resources.iter().find(|resource| resource.id == id)
    }

    /// The exchange with this id.
    #[must_use]
    pub fn exchange(&self, id: &str) -> Option<&ExchangeRule> {
        self.exchanges.iter().find(|exchange| exchange.id == id)
    }

    fn validate(&self) -> Result<(), CatalogError> {
        for (index, resource) in self.resources.iter().enumerate() {
            if self.resources[..index].iter().any(|r| r.id == resource.id) {
                return Err(CatalogError(format!(
                    "resource {:?} is listed twice",
                    resource.id
                )));
            }
        }
        if self.resource(&self.salvage.resource).is_none() {
            return Err(CatalogError(format!(
                "salvage yields unknown resource {:?}",
                self.salvage.resource
            )));
        }
        for (index, exchange) in self.exchanges.iter().enumerate() {
            if self.exchanges[..index].iter().any(|e| e.id == exchange.id) {
                return Err(CatalogError(format!(
                    "exchange {:?} is listed twice",
                    exchange.id
                )));
            }
            for id in [&exchange.from, &exchange.to] {
                if self.resource(id).is_none() {
                    return Err(CatalogError(format!(
                        "exchange {:?} names unknown resource {id:?}",
                        exchange.id
                    )));
                }
            }
            if exchange.from_amount == 0 || exchange.to_amount == 0 {
                return Err(CatalogError(format!(
                    "exchange {:?} has a zero amount",
                    exchange.id
                )));
            }
        }
        Ok(())
    }
}

/// Every character, dungeon and economy definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    characters: Vec<CharacterDefinition>,
    dungeons: Vec<DungeonDefinition>,
    economy: Economy,
}

/// Why a catalog could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogError(pub String);

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "catalog: {}", self.0)
    }
}

impl std::error::Error for CatalogError {}

#[derive(Deserialize)]
struct CharacterFile {
    version: u32,
    characters: Vec<CharacterRecord>,
}

#[derive(Deserialize)]
struct CharacterRecord {
    id: String,
    name: String,
    class: String,
    hero: String,
    roles: Vec<Role>,
    #[serde(default)]
    starter: bool,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct DungeonFile {
    version: u32,
    dungeons: Vec<DungeonRecord>,
}

#[derive(Deserialize)]
struct DungeonRecord {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    scene: String,
    difficulty: Difficulty,
    min_level: u32,
    max_level: u32,
    #[serde(default)]
    min_gear_score: u32,
    #[serde(default)]
    recommended_gear_score: u32,
    #[serde(default)]
    rewards: Vec<String>,
    keys: KeyRules,
    #[serde(default)]
    reward_table: RewardTable,
    countdown_seconds: u32,
    spawn_points: Vec<StartPoint>,
    start_barrier: BarrierBox,
}

impl Catalog {
    /// The catalog this build was compiled with.
    ///
    /// # Errors
    ///
    /// Only if the shipped JSON is broken, which `cargo test` catches before anything ships.
    pub fn builtin() -> Result<Self, CatalogError> {
        Self::parse(CHARACTERS_JSON, DUNGEONS_JSON, ECONOMY_JSON)
    }

    /// A catalog from the three documents' text.
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version, a duplicate id, an unknown hero, a character with
    /// no role, a dungeon with no spawn point or an impossible level range or key rules, a
    /// reward naming a resource that does not exist, or an economy that contradicts itself.
    pub fn parse(characters: &str, dungeons: &str, economy: &str) -> Result<Self, CatalogError> {
        let characters: CharacterFile = serde_json::from_str(characters)
            .map_err(|error| CatalogError(format!("characters.json: {error}")))?;
        let dungeons: DungeonFile = serde_json::from_str(dungeons)
            .map_err(|error| CatalogError(format!("dungeons.json: {error}")))?;
        let economy: Economy = serde_json::from_str(economy)
            .map_err(|error| CatalogError(format!("economy.json: {error}")))?;
        if characters.version != CATALOG_VERSION
            || dungeons.version != CATALOG_VERSION
            || economy.version != CATALOG_VERSION
        {
            return Err(CatalogError(format!(
                "this build reads catalog version {CATALOG_VERSION}"
            )));
        }

        economy.validate()?;

        let mut out = Self {
            characters: Vec::with_capacity(characters.characters.len()),
            dungeons: Vec::with_capacity(dungeons.dungeons.len()),
            economy,
        };
        for record in characters.characters {
            if out.character(&record.id).is_some() {
                return Err(CatalogError(format!(
                    "character {:?} is listed twice",
                    record.id
                )));
            }
            let hero = Hero::from_config_name(&record.hero).ok_or_else(|| {
                CatalogError(format!(
                    "character {:?} names hero {:?}, which this build does not have",
                    record.id, record.hero
                ))
            })?;
            if record.roles.is_empty() {
                return Err(CatalogError(format!(
                    "character {:?} has no role",
                    record.id
                )));
            }
            out.characters.push(CharacterDefinition {
                id: record.id,
                name: record.name,
                class: record.class,
                hero,
                roles: record.roles,
                starter: record.starter,
                description: record.description,
            });
        }
        for record in dungeons.dungeons {
            if out.dungeon(&record.id).is_some() {
                return Err(CatalogError(format!(
                    "dungeon {:?} is listed twice",
                    record.id
                )));
            }
            if record.spawn_points.is_empty() {
                return Err(CatalogError(format!(
                    "dungeon {:?} has no spawn point",
                    record.id
                )));
            }
            if record.min_level > record.max_level {
                return Err(CatalogError(format!(
                    "dungeon {:?} has min_level above max_level",
                    record.id
                )));
            }
            let keys = record.keys;
            if keys.min_level < 1 || keys.max_level < keys.min_level {
                return Err(CatalogError(format!(
                    "dungeon {:?} has an impossible key level range",
                    record.id
                )));
            }
            if !(keys.health_per_level >= 0.0 && keys.damage_per_level >= 0.0) {
                return Err(CatalogError(format!(
                    "dungeon {:?} has a negative key multiplier",
                    record.id
                )));
            }
            if let Some(unknown) = record
                .reward_table
                .resources
                .iter()
                .find(|reward| out.economy.resource(&reward.id).is_none())
            {
                return Err(CatalogError(format!(
                    "dungeon {:?} rewards unknown resource {:?}",
                    record.id, unknown.id
                )));
            }
            out.dungeons.push(DungeonDefinition {
                id: record.id,
                name: record.name,
                description: record.description,
                scene: record.scene,
                difficulty: record.difficulty,
                min_level: record.min_level,
                max_level: record.max_level,
                min_gear_score: record.min_gear_score,
                recommended_gear_score: record.recommended_gear_score,
                rewards: record.rewards,
                keys,
                reward_table: record.reward_table,
                countdown_seconds: record.countdown_seconds,
                spawn_points: record.spawn_points,
                start_barrier: record.start_barrier,
            });
        }
        Ok(out)
    }

    /// Every character definition, in catalog order.
    #[must_use]
    pub fn characters(&self) -> &[CharacterDefinition] {
        &self.characters
    }

    /// Every dungeon, in catalog order.
    #[must_use]
    pub fn dungeons(&self) -> &[DungeonDefinition] {
        &self.dungeons
    }

    /// Resources, salvage and exchanges.
    #[must_use]
    pub fn economy(&self) -> &Economy {
        &self.economy
    }

    /// The character definition with this id.
    #[must_use]
    pub fn character(&self, id: &str) -> Option<&CharacterDefinition> {
        self.characters
            .iter()
            .find(|definition| definition.id == id)
    }

    /// The dungeon with this id.
    #[must_use]
    pub fn dungeon(&self, id: &str) -> Option<&DungeonDefinition> {
        self.dungeons.iter().find(|definition| definition.id == id)
    }

    /// The definitions a new account is given one of each.
    pub fn starters(&self) -> impl Iterator<Item = &CharacterDefinition> {
        self.characters
            .iter()
            .filter(|definition| definition.starter)
    }
}

/// The level files a server without a filesystem carries inside itself.
///
/// Behind the `embedded-scenes` feature so the client — which reads levels from disk and is
/// already large — does not carry a second copy of every dungeon.
#[cfg(feature = "embedded-scenes")]
#[must_use]
pub fn embedded_scene(path: &str) -> Option<&'static str> {
    match path {
        "scenes/graveyard_crypt.json" => {
            Some(include_str!("../../../assets/scenes/graveyard_crypt.json"))
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_catalog_reads() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        assert!(catalog.characters().len() >= 3);
        assert!(!catalog.dungeons().is_empty());
        // Every role can be filled by some starter, or no starter party could ever form.
        for role in Role::ALL {
            assert!(
                catalog
                    .starters()
                    .any(|definition| definition.can_play(role)),
                "no starter character can play {role:?}"
            );
        }
        // Every dungeon places at least a whole party.
        for dungeon in catalog.dungeons() {
            assert!(dungeon.spawn_points.len() >= Role::ALL.len());
        }
    }

    #[test]
    fn every_hero_has_a_definition() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        for hero in Hero::ALL {
            assert!(
                catalog
                    .characters()
                    .iter()
                    .any(|definition| definition.hero == hero)
            );
        }
    }

    #[test]
    fn duplicate_and_unknown_rows_are_refused() {
        let dungeons = r#"{"version":1,"dungeons":[]}"#;
        let twice = r#"{"version":1,"characters":[
            {"id":"a","name":"A","class":"C","hero":"warrior","roles":["tank"]},
            {"id":"a","name":"A","class":"C","hero":"warrior","roles":["tank"]}]}"#;
        assert!(Catalog::parse(twice, dungeons, ECONOMY_JSON).is_err());
        let unknown = r#"{"version":1,"characters":[
            {"id":"a","name":"A","class":"C","hero":"bard","roles":["tank"]}]}"#;
        assert!(Catalog::parse(unknown, dungeons, ECONOMY_JSON).is_err());
        let roleless = r#"{"version":1,"characters":[
            {"id":"a","name":"A","class":"C","hero":"warrior","roles":[]}]}"#;
        assert!(Catalog::parse(roleless, dungeons, ECONOMY_JSON).is_err());
    }

    /// The shipped dungeons document with one substring replaced.
    fn dungeons_with(from: &str, to: &str) -> String {
        assert!(
            DUNGEONS_JSON.contains(from),
            "{from} is not in the shipped file"
        );
        DUNGEONS_JSON.replacen(from, to, 1)
    }

    fn parse_with_dungeons(dungeons: &str) -> Result<Catalog, CatalogError> {
        Catalog::parse(CHARACTERS_JSON, dungeons, ECONOMY_JSON)
    }

    #[test]
    fn keys_rewards_and_economy_are_read() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let crypt = catalog.dungeon("graveyard_crypt").expect("the crypt");
        assert_eq!(crypt.keys.min_level, 1);
        assert_eq!(crypt.keys.max_level, 20);
        assert!((crypt.keys.health_per_level - 0.10).abs() < 1e-6);
        assert!((crypt.keys.damage_per_level - 0.05).abs() < 1e-6);
        assert_eq!(crypt.reward_table.experience, 200);
        assert_eq!(crypt.reward_table.experience_per_level, 50);
        assert_eq!(
            crypt.reward_table.resources,
            vec![ResourceReward {
                id: "crypt_dust".into(),
                amount: 5,
                per_level: 2
            }]
        );
        let economy = catalog.economy();
        assert_eq!(economy.resources.len(), 3);
        assert_eq!(economy.salvage.resource, "bone_shard");
        let trade = economy.exchange("dust_to_ember").expect("a trade");
        assert_eq!((trade.from_amount, trade.to_amount), (20, 1));
        assert!(economy.exchange("nope").is_none());
    }

    #[test]
    fn bad_key_rules_are_refused() {
        let inverted = dungeons_with("\"max_level\": 20", "\"max_level\": 0");
        assert!(parse_with_dungeons(&inverted).is_err());
        let zero_min = dungeons_with(
            "\"min_level\": 1,\n        \"max_level\": 20",
            "\"min_level\": 0,\n        \"max_level\": 20",
        );
        assert!(parse_with_dungeons(&zero_min).is_err());
        let negative = dungeons_with("\"health_per_level\": 0.10", "\"health_per_level\": -0.1");
        assert!(parse_with_dungeons(&negative).is_err());
    }

    #[test]
    fn rewards_must_name_real_resources() {
        let unknown = dungeons_with("\"id\": \"crypt_dust\"", "\"id\": \"moon_cheese\"");
        assert!(parse_with_dungeons(&unknown).is_err());
    }

    #[test]
    fn economy_references_are_checked() {
        let edit = |from: &str, to: &str| ECONOMY_JSON.replacen(from, to, 1);
        for bad in [
            edit("\"resource\": \"bone_shard\"", "\"resource\": \"nothing\""),
            edit("\"to\": \"steward_ember\"", "\"to\": \"nothing\""),
            edit("\"from_amount\": 20", "\"from_amount\": 0"),
        ] {
            assert_ne!(bad, ECONOMY_JSON);
            assert!(Catalog::parse(CHARACTERS_JSON, DUNGEONS_JSON, &bad).is_err());
        }
    }

    #[test]
    fn admission_checks_level_and_gear() {
        let catalog = Catalog::builtin().expect("the shipped catalog parses");
        let dungeon = &catalog.dungeons()[0];
        assert!(
            dungeon
                .admits(dungeon.min_level, dungeon.min_gear_score)
                .is_ok()
        );
        assert!(dungeon.admits(dungeon.max_level + 1, 10_000).is_err());
        if dungeon.min_level > 0 {
            assert!(dungeon.admits(dungeon.min_level - 1, 10_000).is_err());
        }
    }
}
