//! Community maps at play time and in the API: ids, the dungeon a map becomes, what a
//! community run is (and is not) worth, and the request and response bodies.
//!
//! ```text
//!  upload ─► mapcheck::check_str ─► (draft, report) ─► publish ─► realm `community_map`
//!                                                                      │ "cm:<id>:<version>"
//!   party queues for the id ─► [`prepare`] (same parse + validate) ─► DungeonDefinition + scene
//! ```
//!
//! A community map is data. [`prepare`] is the single place that turns the stored text into
//! something the realm can run, and it runs the full mapcheck again: the API validates on every
//! write, the realm validates what it is handed, and the client builds from the text it
//! fetched, all through this function.
//!
//! # What a community run grants
//!
//! Nothing but a leaderboard entry. [`grants_nothing`] names the runs this applies to, and the
//! enforcement is layered so no single mistake pays out: the fight is armed with
//! [`no_loot_rules`], the realm skips the chest and drop tallies, `finish` pays
//! [`zero_rewards`], the API's `apply_result` stores a community result through
//! [`sanitized`] and skips every grant, and `apply_loot` refuses a delivery for a community run.
//!
//! # Ids
//!
//! A run's `dungeon_id` is `cm:<map id>:<version>` ([`dungeon_id`]); boards and history key the
//! map as `cm:<map id>` ([`board_id`]) so every version of a map shares one board.

use serde::{Deserialize, Serialize};

use engine_environment::EnvironmentScene;
use engine_game::loot::LootRules;

use crate::api::DungeonResult;
use crate::catalog::{BarrierBox, DungeonDefinition, KeyRules, RewardTable, StartPoint};
use crate::mapcheck::{CommunityMap, Report, Rules, check_map, check_str};
use crate::rewards::RunRewards;

/// What every community dungeon id starts with.
pub const PREFIX: &str = "cm:";

/// The hardest key a community run may use. Difficulty is the key level, 1 to this.
pub const MAX_DIFFICULTY: u32 = 10;

/// The official dungeon whose countdown and key scaling a community map borrows.
pub const TEMPLATE_DUNGEON: &str = "graveyard_crypt";

/// The largest request body the API reads for a map write, in bytes. Checked before parsing
/// (the mapcheck limit of 8 MiB is for files on disk; the wire is stricter).
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// The most versions one map may accumulate; the API refuses more.
pub const MAX_VERSIONS: u32 = 200;

/// The most maps one account may have that are not rejected.
pub const MAX_MAPS_PER_AUTHOR: i64 = 50;

/// Whether `dungeon_id` names a community map.
#[must_use]
pub fn is_community(dungeon_id: &str) -> bool {
    dungeon_id.starts_with(PREFIX)
}

/// Whether a run of `dungeon_id` pays no loot, currency, experience or key progress.
#[must_use]
pub fn grants_nothing(dungeon_id: &str) -> bool {
    is_community(dungeon_id)
}

/// `cm:<map id>:<version>`.
#[must_use]
pub fn dungeon_id(map_id: &str, version: u32) -> String {
    format!("{PREFIX}{map_id}:{version}")
}

/// A map id is a UUID in text: hex digits and dashes, bounded.
fn plausible_map_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The map id and version of `cm:<id>:<version>`.
#[must_use]
pub fn parse_dungeon_id(dungeon_id: &str) -> Option<(&str, u32)> {
    let rest = dungeon_id.strip_prefix(PREFIX)?;
    let (id, version) = rest.rsplit_once(':')?;
    let version: u32 = version.parse().ok().filter(|v| *v >= 1)?;
    plausible_map_id(id).then_some((id, version))
}

/// The key boards and history use for a dungeon id: `cm:<id>` for a community id of either
/// form, anything else unchanged.
#[must_use]
pub fn board_id(dungeon_id: &str) -> String {
    if let Some((id, _)) = parse_dungeon_id(dungeon_id) {
        return format!("{PREFIX}{id}");
    }
    dungeon_id.to_owned()
}

/// The map id of a board id (`cm:<id>`).
#[must_use]
pub fn map_of_board(board: &str) -> Option<&str> {
    let id = board.strip_prefix(PREFIX)?;
    plausible_map_id(id).then_some(id)
}

/// A community map ready to run.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The validated file.
    pub map: CommunityMap,
    /// Its level, built from the scene alone.
    pub scene: EnvironmentScene,
    /// The dungeon row the realm and client run it with.
    pub definition: DungeonDefinition,
}

/// Parses, validates (the full community rules) and builds `text` into something playable as
/// `dungeon_id`.
///
/// # Errors
///
/// The [`Report`] naming everything wrong, if there is any error.
pub fn prepare(
    text: &str,
    dungeon_id: &str,
    template: &DungeonDefinition,
) -> Result<Prepared, Report> {
    let rules = Rules::default_community();
    let mut report = check_str(text, &rules);
    if !report.is_valid() {
        return Err(report);
    }
    let map = CommunityMap::parse(text)?;
    let scene = match map.scene.to_scene(dungeon_id) {
        Ok(scene) => scene,
        Err(error) => {
            report.error("scene_error", "scene", error.to_string());
            return Err(report);
        }
    };
    let definition = definition(&map, dungeon_id, template);
    Ok(Prepared {
        map,
        scene,
        definition,
    })
}

/// The dungeon row for `map`: the template's countdown and scaling, the map's title, spawn
/// points and a start barrier ahead of them, no rewards, keys 1 to [`MAX_DIFFICULTY`].
#[must_use]
pub fn definition(
    map: &CommunityMap,
    dungeon_id: &str,
    template: &DungeonDefinition,
) -> DungeonDefinition {
    let spawn_points: Vec<StartPoint> = map
        .scene
        .spawn_points
        .iter()
        .map(|point| StartPoint {
            position: point.position,
            yaw: point.yaw,
        })
        .collect();
    let barrier = start_barrier(&spawn_points);
    DungeonDefinition {
        id: dungeon_id.to_owned(),
        name: map.meta.title.clone(),
        description: map.meta.description.clone(),
        scene: format!("community/{dungeon_id}"),
        min_level: 1,
        max_level: 999,
        min_gear_score: 0,
        recommended_gear_score: 0,
        rewards: Vec::new(),
        keys: KeyRules {
            min_level: 1,
            max_level: MAX_DIFFICULTY,
            ..template.keys
        },
        reward_table: RewardTable::default(),
        // The map's own clock, if it sets one (validated 300-3600 s). Without one a completion
        // is always timed and the boards show it as untimed.
        timer_seconds: map.scene.dungeon.as_ref().and_then(|dungeon| dungeon.timer_seconds),
        spawn_points: if spawn_points.is_empty() {
            template.spawn_points.clone()
        } else {
            spawn_points
        },
        start_barrier: barrier,
        ..template.clone()
    }
}

/// A wall ahead of the party that holds it during the countdown: 8 m in front of the spawn
/// points' centre, square to the nearest axis the first spawn faces.
fn start_barrier(spawns: &[StartPoint]) -> BarrierBox {
    let count = spawns.len().max(1) as f32;
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    for point in spawns {
        x += point.position[0];
        y += point.position[1];
        z += point.position[2];
    }
    let (x, y, z) = (x / count, y / count, z / count);
    let yaw = spawns.first().map_or(0.0, |point| point.yaw);
    // Facing: yaw 0 looks along -Z.
    let (fx, fz) = (-yaw.sin(), -yaw.cos());
    if fz.abs() >= fx.abs() {
        let side = if fz < 0.0 { -1.0 } else { 1.0 };
        BarrierBox {
            center: [x, y + 2.5, z + 8.0 * side],
            half_extents: [10.0, 2.5, 0.6],
        }
    } else {
        let side = if fx < 0.0 { -1.0 } else { 1.0 };
        BarrierBox {
            center: [x + 8.0 * side, y + 2.5, z],
            half_extents: [0.6, 2.5, 10.0],
        }
    }
}

/// Loot rules that roll nothing: the shipped rules with every chance and amount zeroed and no
/// boss table. `None` only if the shipped loot rules are broken.
#[must_use]
pub fn no_loot_rules() -> Option<LootRules> {
    let mut rules = crate::loot::builtin_rules()?.clone();
    rules.equipment_per_completion = 0;
    rules.boss_tables.clear();
    rules.mob.equipment_chance = 0.0;
    rules.currency.per_mob_chance = 0.0;
    rules.currency.elite_chance = 0.0;
    rules.currency.amount = 0;
    rules.gold.per_mob_min = 0;
    rules.gold.per_mob_max = 0;
    rules.gold.boss_amount = 0;
    Some(rules)
}

/// What a community run pays: nothing at all.
#[must_use]
pub fn zero_rewards() -> RunRewards {
    RunRewards::default()
}

/// `result` with everything a community run must not grant removed: experience, resources,
/// talent points, looted currency, spending, key changes, the damage report. What remains is
/// the history and leaderboard entry. The dungeon id becomes the map's board id.
#[must_use]
pub fn sanitized(result: &DungeonResult) -> DungeonResult {
    DungeonResult {
        dungeon_id: board_id(&result.dungeon_id),
        experience: 0,
        key_owner: false,
        key_level_after: 0,
        resources: Vec::new(),
        damage_report: None,
        talent_points: 0,
        looted: Vec::new(),
        currency_spent: Vec::new(),
        participant_key_bump: false,
        own_key_level: 0,
        ..result.clone()
    }
}

/// Reads an editor-exported scene (a `SceneDocument` with a `dungeon` block and a top-level
/// `community` block `{title, description, difficulty, tags}`) and wraps it in the community
/// file format. A file already in the wrapper format (`format`, `meta`, `scene`) is returned
/// as it is. The result still goes through [`check_str`]; this only finds the metadata.
///
/// # Errors
///
/// A sentence for the player: not JSON, not an object, or no metadata to list the map by.
pub fn wrap_editor_scene(text: &str) -> Result<String, String> {
    if text.len() > MAX_BODY_BYTES {
        return Err(format!(
            "That file is larger than {} MiB.",
            MAX_BODY_BYTES / (1024 * 1024)
        ));
    }
    let mut value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("That file is not valid JSON: {e}"))?;
    let Some(object) = value.as_object_mut() else {
        return Err("That file is not a level (a JSON object was expected).".into());
    };
    if object.contains_key("format") && object.contains_key("meta") && object.contains_key("scene")
    {
        return Ok(text.to_owned());
    }
    let Some(community) = object.remove("community") else {
        return Err(
            "That level has no `community` block (title, description, difficulty, tags). \
             Add one in the editor's metadata panel."
                .into(),
        );
    };
    if !community.is_object() {
        return Err("The `community` block must be an object.".into());
    }
    let wrapped = serde_json::json!({
        "format": crate::mapcheck::COMMUNITY_FORMAT,
        "meta": community,
        "scene": value,
    });
    serde_json::to_string(&wrapped).map_err(|e| e.to_string())
}

/// A short heading for an issue code, for lists in the UI.
#[must_use]
pub fn code_title(code: &str) -> &'static str {
    match code {
        "too_large" => "File too large",
        "malformed_json" => "Not valid JSON",
        "schema_error" => "Wrong file layout",
        "unsupported_version" => "Unsupported version",
        "unsupported_feature" => "Unsupported field",
        "loot_not_allowed" => "Loot is not allowed",
        "bad_meta" => "Title, description or tags",
        "bad_string" => "Bad name",
        "too_many_objects" => "Too many objects",
        "too_many_monsters" => "Too many monsters",
        "too_many_lights" => "Too many lights",
        "bad_extent" => "Map size",
        "non_finite" => "Invalid number",
        "bad_number" => "Number out of range",
        "missing_asset" => "Unknown asset",
        "bad_collision_shape" => "Bad collision shape",
        "unknown_monster" => "Unknown monster",
        "boss_outside_boss_pack" => "Boss outside the boss pack",
        "too_many_boss_packs" => "More than one boss pack",
        "no_spawn" => "No spawn point",
        "invalid_spawn" => "Bad spawn or position",
        "missing_dungeon" => "No dungeon block",
        "missing_boss" => "No boss",
        "bad_boss_gate" => "Bad boss gate",
        "impossible_objective" => "Objective cannot be reached",
        "bad_add_pool" => "Bad add pool",
        "bad_patrol" => "Bad patrol",
        "scene_error" => "The level cannot be built",
        "no_monsters" => "No monsters",
        "boss_pack_without_boss" => "Boss pack has no boss",
        _ => "Problem",
    }
}

/// What a map is listed by: its title, blurb, difficulty and tags, made safe to store even for
/// a map that failed validation (a draft with a bad title is still a draft).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// 3 to 48 characters, no control characters; "Untitled map" if the file's is unusable.
    pub title: String,
    /// At most 1000 characters, no control characters.
    pub description: String,
    /// 1 to 5.
    pub difficulty: u8,
    /// At most five tags of 1 to 16 characters.
    pub tags: Vec<String>,
}

impl Listing {
    /// The listing of `map`.
    #[must_use]
    pub fn of(map: &CommunityMap) -> Self {
        let clean = |text: &str, max: usize| -> String {
            text.chars()
                .filter(|c| !c.is_control())
                .take(max)
                .collect::<String>()
                .trim()
                .to_owned()
        };
        let mut title = clean(&map.meta.title, 48);
        if title.chars().count() < 3 {
            title = "Untitled map".to_owned();
        }
        let tags = map
            .meta
            .tags
            .iter()
            .map(|tag| clean(tag, 16))
            .filter(|tag| !tag.is_empty())
            .take(5)
            .collect();
        Self {
            title,
            description: clean(&map.meta.description, 1000),
            difficulty: map.meta.difficulty.clamp(1, 5),
            tags,
        }
    }
}

/// Everything the API learns from one upload.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The parsed map, if the text was a map at all.
    pub map: Option<CommunityMap>,
    /// The full community report.
    pub report: Report,
}

/// Runs the community rules over `text` (the one validation every write goes through).
#[must_use]
pub fn check(text: &str) -> Checked {
    let (map, report) = check_map(text, &Rules::default_community());
    Checked { map, report }
}

// ---------------------------------------------------------------------------------------
// API bodies
// ---------------------------------------------------------------------------------------

/// One problem found in a map, as the API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueInfo {
    /// The stable code, such as `unknown_monster`.
    pub code: String,
    /// Where: `scene.monsters[12].monster`.
    pub path: String,
    /// What is wrong, for a person.
    pub message: String,
}

/// A validation report, as the API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReportInfo {
    /// Problems that refuse the map.
    #[serde(default)]
    pub errors: Vec<IssueInfo>,
    /// Things worth a look.
    #[serde(default)]
    pub warnings: Vec<IssueInfo>,
}

impl ReportInfo {
    /// Whether the map may be published.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

impl From<&Report> for ReportInfo {
    fn from(report: &Report) -> Self {
        let convert = |issues: &[crate::mapcheck::Issue]| {
            issues
                .iter()
                .map(|issue| IssueInfo {
                    code: issue.code.to_owned(),
                    path: issue.path.clone(),
                    message: issue.message.clone(),
                })
                .collect()
        };
        Self {
            errors: convert(&report.errors),
            warnings: convert(&report.warnings),
        }
    }
}

/// Where a map is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapStatus {
    /// Only its author sees it.
    Draft,
    /// Listed and playable.
    Published,
    /// Taken down by a moderator; the author cannot republish.
    Hidden,
    /// Refused by a moderator.
    Rejected,
}

impl MapStatus {
    /// The stored name.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Hidden => "hidden",
            Self::Rejected => "rejected",
        }
    }

    /// The status stored as `key`.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "draft" => Some(Self::Draft),
            "published" => Some(Self::Published),
            "hidden" => Some(Self::Hidden),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// One map in a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapSummary {
    /// The map's id (a UUID).
    pub id: String,
    /// Listing title.
    pub title: String,
    /// The author's account id.
    pub author_id: String,
    /// The author's display name.
    pub author: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// The author's claimed difficulty, 1 to 5.
    pub difficulty: u8,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Where it is in its life.
    pub status: MapStatus,
    /// The latest version.
    pub version: u32,
    /// Runs started on it (one per run, however many players).
    pub plays: i64,
    /// The fastest completed run, microseconds, if any was.
    #[serde(default)]
    pub fastest_us: Option<i64>,
    /// Completed runs.
    #[serde(default)]
    pub completions: i64,
    /// Whether the latest version validates (always true once published).
    #[serde(default)]
    pub valid: bool,
    /// Created, microseconds since the epoch.
    pub created_at_us: i64,
    /// Last changed, microseconds since the epoch.
    pub updated_at_us: i64,
}

/// A page of maps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapPage {
    /// The maps.
    pub maps: Vec<MapSummary>,
    /// The page, from 0.
    pub page: u32,
    /// Maps per page.
    pub page_size: u32,
    /// Whether a later page exists.
    #[serde(default)]
    pub has_more: bool,
}

/// One stored version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// The version, from 1.
    pub version: u32,
    /// Saved, microseconds since the epoch.
    pub created_at_us: i64,
    /// Whether it validated.
    pub valid: bool,
    /// Error count.
    pub errors: u32,
    /// Warning count.
    pub warnings: u32,
}

/// One map in full, with the report of its latest version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapDetail {
    /// The listing row.
    pub summary: MapSummary,
    /// The latest version's report.
    pub report: ReportInfo,
    /// Every version, newest first (the author sees all; others, the latest).
    #[serde(default)]
    pub versions: Vec<VersionInfo>,
    /// The dungeon id to queue for: `cm:<id>:<version>`. Empty unless the map is published.
    #[serde(default)]
    pub dungeon_id: String,
    /// The hardest difficulty a run may use.
    #[serde(default)]
    pub max_difficulty: u32,
}

/// `POST /community/maps` and `PUT /community/maps/{id}` and `POST /community/validate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveMapRequest {
    /// Idempotency key for this save; a retry returns the first answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The map file: `{format, meta, scene}`.
    pub data: serde_json::Value,
}

/// The answer to a save or a validate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveMapResponse {
    /// The report of what was saved (or checked).
    pub report: ReportInfo,
    /// The map after the save; `None` for a dry run.
    #[serde(default)]
    pub map: Option<MapDetail>,
}

/// `POST /community/maps/{id}/publish|unpublish`, and the moderation calls.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MapActionRequest {
    /// Idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// A moderator's note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The most characters a title search may have.
pub const SEARCH_MAX: usize = 48;

/// What a list of maps is asked for (the client's side of `GET /community/maps`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapQuery {
    /// A title substring.
    pub search: String,
    /// Only this author's maps (account id).
    pub author: Option<String>,
    /// The caller's own maps, every status.
    pub mine: bool,
    /// The order.
    pub sort: Option<MapSort>,
    /// The author-claimed difficulty, 1 to 5.
    pub difficulty: Option<u8>,
    /// From 0.
    pub page: u32,
}

impl MapQuery {
    /// The query string (leading `?`), values percent-encoded.
    #[must_use]
    pub fn to_query(&self) -> String {
        let mut parts = Vec::new();
        let search = self.search.trim();
        if !search.is_empty() {
            parts.push(format!("q={}", percent_encode(search)));
        }
        if let Some(author) = &self.author {
            parts.push(format!("author={}", percent_encode(author)));
        }
        if self.mine {
            parts.push("mine=true".to_owned());
        }
        if let Some(sort) = self.sort {
            parts.push(format!("sort={}", sort.key()));
        }
        if let Some(difficulty) = self.difficulty {
            parts.push(format!("difficulty={difficulty}"));
        }
        if self.page > 0 {
            parts.push(format!("page={}", self.page));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// Percent-encodes everything but unreserved ASCII.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// How a list is sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapSort {
    /// Newest first.
    Newest,
    /// Most played first.
    Plays,
    /// Fastest completed run first.
    Fastest,
    /// Easiest first.
    Difficulty,
    /// Hardest first.
    Hardest,
}

impl MapSort {
    /// Every sort, in the order the client cycles them.
    pub const ALL: [Self; 5] = [
        Self::Newest,
        Self::Plays,
        Self::Fastest,
        Self::Difficulty,
        Self::Hardest,
    ];

    /// The query value.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Plays => "plays",
            Self::Fastest => "fastest",
            Self::Difficulty => "difficulty",
            Self::Hardest => "hardest",
        }
    }

    /// The sort written as `key`.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sort| sort.key() == key)
    }

    /// A label for the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest",
            Self::Plays => "Most played",
            Self::Fastest => "Fastest clear",
            Self::Difficulty => "Easiest",
            Self::Hardest => "Hardest",
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    const FIXTURE: &str = include_str!("../tests/fixtures/community/valid_small.json");

    fn template() -> DungeonDefinition {
        Catalog::builtin()
            .expect("the catalog")
            .dungeon(TEMPLATE_DUNGEON)
            .expect("the template")
            .clone()
    }

    #[test]
    fn ids_round_trip_and_reject_nonsense() {
        let id = dungeon_id("0b8f6d0e-1c2a-4d3b-9e4f-5a6b7c8d9e0f", 3);
        assert_eq!(id, "cm:0b8f6d0e-1c2a-4d3b-9e4f-5a6b7c8d9e0f:3");
        assert_eq!(
            parse_dungeon_id(&id),
            Some(("0b8f6d0e-1c2a-4d3b-9e4f-5a6b7c8d9e0f", 3))
        );
        assert_eq!(board_id(&id), "cm:0b8f6d0e-1c2a-4d3b-9e4f-5a6b7c8d9e0f");
        assert_eq!(board_id("graveyard_crypt"), "graveyard_crypt");
        for bad in ["cm:", "cm:abc", "cm:abc:0", "cm:abc:x", "cm:zzz:1", "cm::1", "crypt:abc:1"] {
            assert_eq!(parse_dungeon_id(bad), None, "{bad}");
        }
        assert!(is_community("cm:anything"));
        assert!(!is_community("graveyard_crypt"));
        assert_eq!(map_of_board("cm:ab-12"), Some("ab-12"));
        assert_eq!(map_of_board("graveyard_crypt"), None);
    }

    #[test]
    fn a_valid_map_prepares_into_a_dungeon_that_pays_nothing() {
        let id = dungeon_id("ab", 1);
        let prepared = prepare(FIXTURE, &id, &template()).expect("the fixture is valid");
        let definition = &prepared.definition;
        assert_eq!(definition.id, id);
        assert_eq!(definition.name, "Small Test Crypt");
        assert_eq!(definition.keys.max_level, MAX_DIFFICULTY);
        assert!(definition.reward_table.resources.is_empty());
        assert_eq!(definition.reward_table.experience, 0);
        assert!(definition.rewards.is_empty());
        assert_eq!(definition.timer_seconds, None);
        assert_eq!(definition.spawn_points.len(), 3);
        assert!(prepared.scene.dungeon.is_some());
        // The same rules gate the key: a community key opens only its own map, up to the cap.
        assert!(crate::key::validate_key(definition, &id, MAX_DIFFICULTY).is_ok());
        assert!(crate::key::validate_key(definition, &id, MAX_DIFFICULTY + 1).is_err());
        assert!(crate::key::validate_key(definition, "graveyard_crypt", 1).is_err());
    }

    #[test]
    fn a_map_timer_reaches_the_run_outcome() {
        use crate::SECOND;
        use crate::outcome::{RunEnd, RunOutcome, classify, timer_us};
        let id = dungeon_id("ab", 1);
        // No timer in the file: untimed, so every completion is timed.
        let untimed = prepare(FIXTURE, &id, &template()).expect("valid").definition;
        assert_eq!(timer_us(&untimed), None);
        assert_eq!(
            classify(RunEnd::Completed, 99_999 * SECOND, timer_us(&untimed)),
            RunOutcome::Timed
        );
        // A timer in the dungeon block becomes the definition's clock.
        let mut value: serde_json::Value = serde_json::from_str(FIXTURE).expect("json");
        value["scene"]["dungeon"]["timer_seconds"] = serde_json::json!(600);
        let timed = prepare(&value.to_string(), &id, &template())
            .expect("valid")
            .definition;
        assert_eq!(timed.timer_seconds, Some(600));
        let clock = timer_us(&timed);
        assert_eq!(clock, Some(600 * SECOND));
        assert_eq!(classify(RunEnd::Completed, 600 * SECOND, clock), RunOutcome::Timed);
        assert_eq!(classify(RunEnd::Completed, 600 * SECOND + 1, clock), RunOutcome::Overtime);
        // A timer outside the range never prepares.
        value["scene"]["dungeon"]["timer_seconds"] = serde_json::json!(5);
        assert!(
            prepare(&value.to_string(), &id, &template())
                .expect_err("refused")
                .has_error("bad_timer")
        );
    }

    #[test]
    fn an_invalid_map_never_prepares() {
        let broken = FIXTURE.replace("crypt_floor", "no_such_asset");
        let report = prepare(&broken, &dungeon_id("ab", 1), &template()).expect_err("refused");
        assert!(report.has_error("missing_asset"));
        let report = prepare("{", &dungeon_id("ab", 1), &template()).expect_err("refused");
        assert!(report.has_error("malformed_json"));
        // Water and terrain are refused for community maps.
        let with_water = FIXTURE.replacen("\"objects\"", "\"water\": [], \"objects\"", 1);
        let report = prepare(&with_water, &dungeon_id("ab", 1), &template()).expect_err("refused");
        assert!(report.has_error("unsupported_feature"));
    }

    #[test]
    fn the_start_barrier_stands_ahead_of_the_spawns() {
        let spawns = [StartPoint {
            position: [0.0, 0.0, -50.0],
            yaw: 0.0,
        }];
        let barrier = start_barrier(&spawns);
        // Yaw 0 faces -Z: the wall is at smaller z, 8 m out.
        assert!((barrier.center[2] - -58.0).abs() < 1e-4);
        assert!(barrier.half_extents[0] > barrier.half_extents[2]);
        let east = start_barrier(&[StartPoint {
            position: [0.0, 0.0, 0.0],
            yaw: -std::f32::consts::FRAC_PI_2,
        }]);
        assert!(east.center[0] > 7.0, "{east:?}");
        assert!(east.half_extents[2] > east.half_extents[0]);
    }

    #[test]
    fn no_loot_rules_roll_nothing() {
        let rules = no_loot_rules().expect("shipped rules");
        assert_eq!(rules.equipment_per_completion, 0);
        assert!(rules.boss_tables.is_empty());
        assert!(rules.mob.equipment_chance <= 0.0);
        assert_eq!(rules.gold.boss_amount, 0);
        assert_eq!(rules.currency.amount, 0);
    }

    #[test]
    fn a_community_run_pays_no_rewards_and_no_key_progress() {
        assert!(grants_nothing("cm:ab:1"));
        assert!(!grants_nothing("graveyard_crypt"));
        assert_eq!(zero_rewards(), RunRewards::default());
        assert_eq!(zero_rewards().experience, 0);
        assert!(zero_rewards().resources.is_empty());
        assert_eq!(zero_rewards().talent_points, 0);
        assert_eq!(zero_rewards().key_level_after, None);
    }

    #[test]
    fn a_sanitized_result_keeps_the_history_and_drops_every_grant() {
        let result = DungeonResult {
            result_id: "u:a".into(),
            run_uid: "u".into(),
            account_id: "a".into(),
            character_id: "c".into(),
            dungeon_id: "cm:ab:2".into(),
            success: true,
            duration_us: 77,
            experience: 500,
            key_level: 4,
            key_owner: true,
            key_level_after: 5,
            resources: vec![("crypt_dust".into(), 9)],
            damage_report: None,
            talent_points: 2,
            looted: vec![("gold".into(), 40)],
            currency_spent: vec![("aether_shard".into(), 1)],
            participant_key_bump: true,
            own_key_level: 3,
            outcome: Some(crate::outcome::RunOutcome::Timed),
            timer_us: 0,
            party: Vec::new(),
            modifiers: Vec::new(),
        };
        let clean = sanitized(&result);
        assert_eq!(clean.dungeon_id, "cm:ab");
        assert_eq!((clean.duration_us, clean.key_level, clean.success), (77, 4, true));
        assert_eq!(clean.experience, 0);
        assert!(clean.resources.is_empty() && clean.looted.is_empty());
        assert!(clean.currency_spent.is_empty() && clean.debits().is_empty());
        assert_eq!(clean.talent_points, 0);
        assert!(clean.key_update().is_none());
    }

    #[test]
    fn editor_scenes_are_wrapped_and_wrapped_files_pass_through() {
        let wrapped: serde_json::Value = serde_json::from_str(FIXTURE).expect("json");
        let scene = wrapped["scene"].clone();
        let mut editor = scene.clone();
        editor["community"] = serde_json::json!({
            "title": "Editor Crypt", "description": "x", "difficulty": 2, "tags": []
        });
        let text = wrap_editor_scene(&editor.to_string()).expect("wraps");
        let report = check_str(&text, &Rules::default_community());
        assert!(report.is_valid(), "{report}");
        assert!(text.contains("Editor Crypt"));
        assert_eq!(wrap_editor_scene(FIXTURE).expect("passes"), FIXTURE);
        assert!(wrap_editor_scene(&scene.to_string()).expect_err("no block").contains("community"));
        assert!(wrap_editor_scene("[]").is_err());
        assert!(wrap_editor_scene("nope").is_err());
    }

    #[test]
    fn a_query_encodes_only_what_it_sets() {
        assert_eq!(MapQuery::default().to_query(), "");
        let query = MapQuery {
            search: " cr ypt&x ".into(),
            author: Some("a-b".into()),
            mine: true,
            sort: Some(MapSort::Plays),
            difficulty: Some(3),
            page: 2,
        };
        assert_eq!(
            query.to_query(),
            "?q=cr%20ypt%26x&author=a-b&mine=true&sort=plays&difficulty=3&page=2"
        );
    }

    #[test]
    fn statuses_and_sorts_round_trip() {
        for status in [
            MapStatus::Draft,
            MapStatus::Published,
            MapStatus::Hidden,
            MapStatus::Rejected,
        ] {
            assert_eq!(MapStatus::from_key(status.key()), Some(status));
        }
        for sort in MapSort::ALL {
            assert_eq!(MapSort::from_key(sort.key()), Some(sort));
        }
        assert_eq!(MapSort::from_key("sql"), None);
    }
}
