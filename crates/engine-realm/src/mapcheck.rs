//! Validation for community maps, shared with the official dungeons.
//!
//! ```text
//!  community.json ─► parse ─► validate ─► Report ─► (publish ─► realm table ─► instance)
//!  graveyard_crypt.json ─────► validate_official ─┘
//! ```
//!
//! # Data, validated, then runtime
//!
//! A community map is *data only*. Its format is the editor's [`SceneDocument`] plus a title
//! and a few tags, and it can reference nothing but approved primitives: assets from the
//! environment library, monsters from the enemy archetype table, and the dungeon block's
//! fixed fields. There is no script, no custom ability, no loot table; a key this module does
//! not know is reported as `unsupported_feature` and the map is refused, never ignored. Official
//! content goes through the same checks with larger limits ([`Rules::official`]), so a rule
//! that is wrong for the crypt is wrong for everyone and gets fixed in one place.
//!
//! Nothing here executes anything, reads a file or a clock. It is pure functions over text
//! and plain data, which is why it compiles into the wasm realm module as-is. The command line
//! (`map_check`) is the only part that touches the filesystem.
//!
//! Every [`Issue`] carries a stable machine `code` (see `docs/community-maps.md`), a JSON-ish
//! `path` such as `scene.monsters[12].monster`, and a sentence for a human.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use engine_core::math::{Quat, Vec3};
use engine_environment::scene_file::{
    CollisionRecord, CollisionShapeRecord, FORMAT_VERSION, SceneDocument, asset_named,
};
use engine_game::enemy::EnemyKind;

use crate::catalog::DungeonDefinition;

#[cfg(test)]
mod tests;

/// The only community file format this build reads.
pub const COMMUNITY_FORMAT: u32 = 1;

/// The largest community file accepted, in bytes.
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// Fewest characters in a map's title.
pub const TITLE_MIN: usize = 3;
/// Most characters in a map's title.
pub const TITLE_MAX: usize = 48;
/// Most characters in a map's description.
pub const DESCRIPTION_MAX: usize = 1000;
/// Most tags on a map.
pub const TAGS_MAX: usize = 5;
/// Most characters in one tag.
pub const TAG_MAX: usize = 16;
/// Lowest and highest difficulty an author may claim.
pub const DIFFICULTY_RANGE: (u8, u8) = (1, 5);
/// Most characters in a pack, monster, scene or dungeon name.
pub const NAME_MAX: usize = 48;
/// Most adds a boss may call.
pub const ADD_COUNT_MAX: u8 = 16;

/// One problem found in a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// A stable machine-readable code, such as `unknown_monster`.
    pub code: &'static str,
    /// Where: `"scene.monsters[12].monster"`.
    pub path: String,
    /// What is wrong, for a person.
    pub message: String,
}

/// What in the map an [`Issue`] is about, read off its `path`.
///
/// The path format is part of the stable contract (`docs/community-maps.md`), so this is the
/// one place that reads it back: the editor selects the object an issue names without
/// parsing strings of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The map as a whole, or a list with no single member to point at.
    Map,
    /// The title, description, difficulty or tags.
    Meta,
    /// `scene.objects[i]`.
    Object(usize),
    /// `scene.monsters[i]`, including its patrol.
    Monster(usize),
    /// `scene.spawn_points[i]`.
    SpawnPoint(usize),
    /// `scene.lights[i]`.
    Light(usize),
    /// The dungeon block (`scene.dungeon`), except the two members below.
    Dungeon,
    /// `scene.dungeon.boss_gate`.
    BossGate,
    /// `scene.dungeon.add_spawns[i]`.
    AddSpawn(usize),
}

impl Issue {
    /// What this issue points at.
    #[must_use]
    pub fn target(&self) -> Target {
        let path = self.path.as_str();
        if path == "meta" || path.starts_with("meta.") || path.starts_with("meta[") {
            return Target::Meta;
        }
        let Some(rest) = path.strip_prefix("scene.") else {
            return Target::Map;
        };
        if let Some(rest) = rest.strip_prefix("dungeon") {
            if rest.starts_with(".boss_gate") {
                return Target::BossGate;
            }
            if let Some(index) = indexed(rest.strip_prefix(".add_spawns")) {
                return Target::AddSpawn(index);
            }
            return Target::Dungeon;
        }
        for prefix in ["objects", "monsters", "spawn_points", "lights"] {
            if let Some(rest) = rest.strip_prefix(prefix)
                && let Some(index) = indexed(Some(rest))
            {
                return match prefix {
                    "objects" => Target::Object(index),
                    "monsters" => Target::Monster(index),
                    "spawn_points" => Target::SpawnPoint(index),
                    _ => Target::Light(index),
                };
            }
        }
        Target::Map
    }
}

/// The `i` of a leading `[i]`, if `text` starts with one.
fn indexed(text: Option<&str>) -> Option<usize> {
    let inner = text?.strip_prefix('[')?;
    let (digits, _) = inner.split_once(']')?;
    digits.parse().ok()
}

impl std::fmt::Display for Issue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "[{}] {}: {}", self.code, self.path, self.message)
    }
}

/// Everything found in one map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Problems that make the map unplayable or unsafe. Any one refuses it.
    pub errors: Vec<Issue>,
    /// Things worth a second look that do not refuse the map.
    pub warnings: Vec<Issue>,
}

impl Report {
    /// Whether the map may be published.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Whether some error carries `code`.
    #[must_use]
    pub fn has_error(&self, code: &str) -> bool {
        self.errors.iter().any(|issue| issue.code == code)
    }

    /// Whether some warning carries `code`.
    #[must_use]
    pub fn has_warning(&self, code: &str) -> bool {
        self.warnings.iter().any(|issue| issue.code == code)
    }

    /// Adds an error (also how the play-time conversion reports a scene it cannot build).
    pub(crate) fn error(
        &mut self,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.errors.push(Issue {
            code,
            path: path.into(),
            message: message.into(),
        });
    }

    fn warning(&mut self, code: &'static str, path: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(Issue {
            code,
            path: path.into(),
            message: message.into(),
        });
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for issue in &self.errors {
            writeln!(formatter, "error   {issue}")?;
        }
        for issue in &self.warnings {
            writeln!(formatter, "warning {issue}")?;
        }
        write!(
            formatter,
            "{} error(s), {} warning(s)",
            self.errors.len(),
            self.warnings.len()
        )
    }
}

/// Which asset names a map may place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPolicy {
    /// Only the environment library's own names.
    Library,
    /// Any non-blank name (official content may name files under `assets/`).
    Any,
}

/// What a map is allowed to contain.
#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    /// Which asset names are placeable.
    pub assets: AssetPolicy,
    /// Which monsters may be placed.
    pub monsters: Vec<EnemyKind>,
    /// Most objects.
    pub max_objects: usize,
    /// Most monsters.
    pub max_monsters: usize,
    /// Most local lights.
    pub max_lights: usize,
    /// Largest `half_extent`, in metres.
    pub max_half_extent: f32,
    /// Most distinct packs that contain a boss archetype.
    pub max_boss_packs: usize,
    /// Most waypoints in one patrol.
    pub max_patrol_points: usize,
    /// Smallest and largest per-axis scale of an object or monster.
    pub scale_range: (f32, f32),
    /// Whether loot fields are accepted.
    pub allow_loot: bool,
    /// Whether `terrain` and `water` are accepted.
    pub allow_terrain_and_water: bool,
    /// Whether the dungeon block is mandatory.
    pub require_dungeon: bool,
}

impl Rules {
    /// Whether a map under these rules may place the asset called `name`.
    ///
    /// The one answer both the validator and the editor's "approved library only" warning
    /// give.
    #[must_use]
    pub fn allows_asset(&self, name: &str) -> bool {
        match self.assets {
            AssetPolicy::Library => asset_named(name).is_some(),
            AssetPolicy::Any => !name.trim().is_empty(),
        }
    }

    /// The monster `id` names, if these rules allow placing it.
    #[must_use]
    pub fn allowed_monster(&self, id: &str) -> Option<EnemyKind> {
        EnemyKind::from_id(id).filter(|kind| self.monsters.contains(kind))
    }

    /// The rules for a community upload, built from the real catalogs.
    #[must_use]
    pub fn default_community() -> Self {
        Self {
            assets: AssetPolicy::Library,
            monsters: EnemyKind::ALL.to_vec(),
            max_objects: 4000,
            max_monsters: 200,
            max_lights: 128,
            max_half_extent: 320.0,
            max_boss_packs: 1,
            max_patrol_points: 16,
            scale_range: (0.1, 10.0),
            allow_loot: false,
            allow_terrain_and_water: false,
            require_dungeon: true,
        }
    }

    /// The relaxed rules official content is held to.
    #[must_use]
    pub fn official() -> Self {
        Self {
            assets: AssetPolicy::Any,
            max_objects: 20_000,
            max_monsters: 1000,
            max_lights: 1000,
            max_half_extent: 2000.0,
            scale_range: (0.01, 1000.0),
            allow_loot: true,
            allow_terrain_and_water: true,
            ..Self::default_community()
        }
    }
}

/// The title, blurb and tags a map is listed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    /// Listing title, 3 to 48 characters.
    pub title: String,
    /// Description, up to 1000 characters.
    #[serde(default)]
    pub description: String,
    /// Author-claimed difficulty, 1 (easy) to 5 (brutal).
    pub difficulty: u8,
    /// Up to five tags of up to 16 characters.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A community map file: metadata plus a level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunityMap {
    /// The file format, [`COMMUNITY_FORMAT`].
    pub format: u32,
    /// How it is listed.
    pub meta: Meta,
    /// The level itself, in the editor's own format.
    pub scene: SceneDocument,
}

impl CommunityMap {
    /// Parses `text`, returning the map only if the structural checks (size, JSON, schema,
    /// unsupported keys) are clean.
    ///
    /// # Errors
    ///
    /// A [`Report`] naming every structural problem found.
    pub fn parse(text: &str) -> Result<Self, Report> {
        let (map, report) = parse_inner(text, &Rules::default_community());
        match map {
            Some(map) if report.is_valid() => Ok(map),
            _ => Err(report),
        }
    }
}

/// Parses and validates `text` in one go: the full pipeline a server runs on an upload.
#[must_use]
pub fn check_str(text: &str, rules: &Rules) -> Report {
    check_map(text, rules).1
}

/// [`check_str`], also returning the parsed map whenever the typed parse succeeded (even if
/// validation found errors), so a server can list a draft by its title and show its report.
/// `None` when the text is not a map at all (too large, malformed, wrong shape).
#[must_use]
pub fn check_map(text: &str, rules: &Rules) -> (Option<CommunityMap>, Report) {
    let (map, mut report) = parse_inner(text, rules);
    if let Some(map) = &map {
        let rest = validate(map, rules);
        report.errors.extend(rest.errors);
        report.warnings.extend(rest.warnings);
    }
    (map, report)
}

/// Validates a parsed community map.
#[must_use]
pub fn validate(map: &CommunityMap, rules: &Rules) -> Report {
    let mut report = Report::default();
    if map.format != COMMUNITY_FORMAT {
        report.error(
            "unsupported_version",
            "format",
            format!(
                "format {} is not readable; this build reads {COMMUNITY_FORMAT}",
                map.format
            ),
        );
    }
    check_meta(&map.meta, &mut report);
    check_scene(&map.scene, rules, &[], "scene", &mut report);
    report
}

/// Validates an official scene under the relaxed [`Rules::official`], also checking the
/// spawn points its catalog row declares.
#[must_use]
pub fn validate_official(scene: &SceneDocument, dungeon: &DungeonDefinition) -> Report {
    let mut report = Report::default();
    let extra: Vec<(String, [f32; 3])> = dungeon
        .spawn_points
        .iter()
        .enumerate()
        .map(|(index, point)| (format!("catalog.spawn_points[{index}]"), point.position))
        .collect();
    check_scene(scene, &Rules::official(), &extra, "scene", &mut report);
    report
}

// ---------------------------------------------------------------------------------------
// Raw (text) stage: size, JSON, key whitelist, patrols.
// ---------------------------------------------------------------------------------------

const TOP_KEYS: &[&str] = &["format", "meta", "scene"];
const META_KEYS: &[&str] = &["title", "description", "difficulty", "tags"];
const SCENE_KEYS: &[&str] = &[
    "version",
    "name",
    "half_extent",
    "ground_height",
    "spawns",
    "lighting",
    "objects",
    "collision_shapes",
    "terrain",
    "water",
    "spawn_points",
    "lights",
    "monsters",
    "dungeon",
    "map",
];
/// Features the community rules refuse as a whole, reported as `unsupported_feature` and
/// removed before the typed parse.
const REFUSED_SCENE_KEYS: &[&str] = &["terrain", "water"];
const SPAWNS_KEYS: &[&str] = &["player", "featured_dummy", "group_center", "group_spacing"];
const LIGHTING_KEYS: &[&str] = &[
    "sun_direction",
    "sun_color",
    "sun_intensity",
    "ambient_color",
    "ambient_intensity",
    "sky_zenith",
    "sky_horizon",
];
const OBJECT_KEYS: &[&str] = &["asset", "position", "yaw", "scale", "collision"];
const COLLISION_KEYS: &[&str] = &["kind", "center", "half_extents", "shape"];
const SHAPE_KEYS: &[&str] = &["boxes"];
const BOX_KEYS: &[&str] = &["center", "half_extents"];
const SPAWN_POINT_KEYS: &[&str] = &["position", "yaw"];
const LIGHT_KEYS: &[&str] = &[
    "position",
    "color",
    "intensity",
    "range",
    "direction",
    "cone_radians",
    "casts_shadow",
    "priority",
];
const MONSTER_KEYS: &[&str] = &[
    "monster",
    "position",
    "yaw",
    "scale",
    "name",
    "pack",
    "aggro_radius",
    "level",
    "enabled",
    "completion",
    "patrol",
];
const DUNGEON_KEYS: &[&str] = &[
    "name",
    "required_completion",
    "boss_pack",
    "boss_gate",
    "add_spawns",
    "add_count",
    "add_pool",
    "allow_duplicate_adds",
];
const GATE_KEYS: &[&str] = &["center", "half_extents", "yaw"];
const MAP_KEYS: &[&str] = &["floor", "ceiling", "padding", "texels_per_metre", "bounds"];

/// Keys that mean "this declares loot". Matched case-insensitively anywhere in the scene.
const LOOT_KEYS: &[&str] = &[
    "loot",
    "loot_table",
    "loot_tables",
    "drops",
    "drop_table",
    "reward",
    "rewards",
    "reward_table",
];

fn parse_inner(text: &str, rules: &Rules) -> (Option<CommunityMap>, Report) {
    let mut report = Report::default();
    if text.len() > MAX_INPUT_BYTES {
        report.error(
            "too_large",
            "",
            format!(
                "{} bytes exceeds the {MAX_INPUT_BYTES} byte limit",
                text.len()
            ),
        );
        return (None, report);
    }
    let mut value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            report.error("malformed_json", "", error.to_string());
            return (None, report);
        }
    };
    let Some(top) = value.as_object_mut() else {
        report.error("schema_error", "", "the file must be a JSON object");
        return (None, report);
    };
    scrub_object(top, "", TOP_KEYS, rules, &mut report);
    if let Some(meta) = top.get_mut("meta").and_then(Value::as_object_mut) {
        scrub_object(meta, "meta", META_KEYS, rules, &mut report);
    }
    if let Some(scene) = top.get_mut("scene").and_then(Value::as_object_mut) {
        scrub_scene(scene, rules, &mut report);
    }
    match serde_json::from_value::<CommunityMap>(value) {
        Ok(map) => (Some(map), report),
        Err(error) => {
            report.error("schema_error", "", error.to_string());
            (None, report)
        }
    }
}

/// Reports and removes every key of `object` not in `allowed`. Loot-looking keys get their
/// own code; keys the rules refuse outright are removed so the typed parse is not confused.
fn scrub_object(
    object: &mut serde_json::Map<String, Value>,
    path: &str,
    allowed: &[&str],
    rules: &Rules,
    report: &mut Report,
) {
    let join = |key: &str| {
        if path.is_empty() {
            key.to_owned()
        } else {
            format!("{path}.{key}")
        }
    };
    let bad: Vec<String> = object
        .keys()
        .filter(|key| {
            let refused = !rules.allow_terrain_and_water
                && path == "scene"
                && REFUSED_SCENE_KEYS.contains(&key.as_str());
            refused || !allowed.contains(&key.as_str())
        })
        .cloned()
        .collect();
    for key in bad {
        let lower = key.to_ascii_lowercase();
        if LOOT_KEYS.contains(&lower.as_str()) {
            if !rules.allow_loot {
                report.error(
                    "loot_not_allowed",
                    join(&key),
                    "community maps cannot declare loot or rewards",
                );
            }
        } else {
            report.error(
                "unsupported_feature",
                join(&key),
                format!("'{key}' is not a supported field"),
            );
        }
        object.remove(&key);
    }
}

fn scrub_each(
    parent: &mut serde_json::Map<String, Value>,
    key: &str,
    path: &str,
    allowed: &[&str],
    rules: &Rules,
    report: &mut Report,
) {
    if let Some(Value::Array(items)) = parent.get_mut(key) {
        for (index, item) in items.iter_mut().enumerate() {
            if let Some(object) = item.as_object_mut() {
                scrub_object(object, &format!("{path}[{index}]"), allowed, rules, report);
            }
        }
    }
}

fn scrub_scene(scene: &mut serde_json::Map<String, Value>, rules: &Rules, report: &mut Report) {
    scrub_object(scene, "scene", SCENE_KEYS, rules, report);
    let half = scene.get("half_extent").and_then(Value::as_f64);

    for (key, allowed) in [
        ("spawns", SPAWNS_KEYS),
        ("lighting", LIGHTING_KEYS),
        ("map", MAP_KEYS),
    ] {
        if let Some(object) = scene.get_mut(key).and_then(Value::as_object_mut) {
            scrub_object(object, &format!("scene.{key}"), allowed, rules, report);
        }
    }
    scrub_each(
        scene,
        "objects",
        "scene.objects",
        OBJECT_KEYS,
        rules,
        report,
    );
    if let Some(Value::Array(objects)) = scene.get_mut("objects") {
        for (index, object) in objects.iter_mut().enumerate() {
            if let Some(collision) = object.get_mut("collision").and_then(Value::as_object_mut) {
                let path = format!("scene.objects[{index}].collision");
                scrub_object(collision, &path, COLLISION_KEYS, rules, report);
            }
        }
    }
    scrub_each(
        scene,
        "collision_shapes",
        "scene.collision_shapes",
        SHAPE_KEYS,
        rules,
        report,
    );
    if let Some(Value::Array(shapes)) = scene.get_mut("collision_shapes") {
        for (index, shape) in shapes.iter_mut().enumerate() {
            if let Some(shape) = shape.as_object_mut() {
                let path = format!("scene.collision_shapes[{index}].boxes");
                scrub_each(shape, "boxes", &path, BOX_KEYS, rules, report);
            }
        }
    }
    scrub_each(
        scene,
        "spawn_points",
        "scene.spawn_points",
        SPAWN_POINT_KEYS,
        rules,
        report,
    );
    scrub_each(scene, "lights", "scene.lights", LIGHT_KEYS, rules, report);
    scrub_each(
        scene,
        "monsters",
        "scene.monsters",
        MONSTER_KEYS,
        rules,
        report,
    );

    if let Some(Value::Array(monsters)) = scene.get("monsters") {
        for (index, monster) in monsters.iter().enumerate() {
            if let Some(patrol) = monster.get("patrol") {
                check_patrol(
                    patrol,
                    half,
                    &format!("scene.monsters[{index}].patrol"),
                    rules,
                    report,
                );
            }
        }
    }

    if let Some(dungeon) = scene.get_mut("dungeon").and_then(Value::as_object_mut) {
        scrub_object(dungeon, "scene.dungeon", DUNGEON_KEYS, rules, report);
        if let Some(gate) = dungeon.get_mut("boss_gate").and_then(Value::as_object_mut) {
            scrub_object(gate, "scene.dungeon.boss_gate", GATE_KEYS, rules, report);
        }
        scrub_each(
            dungeon,
            "add_spawns",
            "scene.dungeon.add_spawns",
            SPAWN_POINT_KEYS,
            rules,
            report,
        );
    }
}

fn check_patrol(patrol: &Value, half: Option<f64>, path: &str, rules: &Rules, report: &mut Report) {
    let Some(points) = patrol.as_array() else {
        report.error("bad_patrol", path, "a patrol is a list of [x, z] pairs");
        return;
    };
    if points.len() > rules.max_patrol_points {
        report.error(
            "bad_patrol",
            path,
            format!(
                "{} waypoints exceeds the limit of {}",
                points.len(),
                rules.max_patrol_points
            ),
        );
    }
    for (index, point) in points.iter().enumerate() {
        let pair = point
            .as_array()
            .filter(|pair| pair.len() == 2)
            .and_then(|pair| Some((pair[0].as_f64()?, pair[1].as_f64()?)));
        let Some((x, z)) = pair else {
            report.error(
                "bad_patrol",
                format!("{path}[{index}]"),
                "a waypoint is two numbers [x, z]",
            );
            continue;
        };
        if let Some(half) = half
            && (x.abs() > half || z.abs() > half)
        {
            report.error(
                "bad_patrol",
                format!("{path}[{index}]"),
                format!("waypoint ({x}, {z}) is outside the map (half extent {half})"),
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Typed stage.
// ---------------------------------------------------------------------------------------

fn printable(text: &str, allow_newline: bool) -> bool {
    text.chars()
        .all(|c| !c.is_control() || (allow_newline && c == '\n'))
}

fn check_meta(meta: &Meta, report: &mut Report) {
    let title_len = meta.title.trim().chars().count();
    if !(TITLE_MIN..=TITLE_MAX).contains(&title_len) || meta.title.chars().count() > TITLE_MAX {
        report.error(
            "bad_meta",
            "meta.title",
            format!("the title must be {TITLE_MIN} to {TITLE_MAX} characters"),
        );
    }
    if !printable(&meta.title, false) {
        report.error(
            "bad_meta",
            "meta.title",
            "the title contains control characters",
        );
    }
    if meta.description.chars().count() > DESCRIPTION_MAX {
        report.error(
            "bad_meta",
            "meta.description",
            format!("the description is over {DESCRIPTION_MAX} characters"),
        );
    }
    if !printable(&meta.description, true) {
        report.error(
            "bad_meta",
            "meta.description",
            "the description contains control characters",
        );
    }
    if !(DIFFICULTY_RANGE.0..=DIFFICULTY_RANGE.1).contains(&meta.difficulty) {
        report.error(
            "bad_meta",
            "meta.difficulty",
            format!(
                "difficulty must be {} to {}",
                DIFFICULTY_RANGE.0, DIFFICULTY_RANGE.1
            ),
        );
    }
    if meta.tags.len() > TAGS_MAX {
        report.error("bad_meta", "meta.tags", format!("at most {TAGS_MAX} tags"));
    }
    for (index, tag) in meta.tags.iter().enumerate() {
        let length = tag.trim().chars().count();
        if length == 0 || tag.chars().count() > TAG_MAX || !printable(tag, false) {
            report.error(
                "bad_meta",
                format!("meta.tags[{index}]"),
                format!("a tag is 1 to {TAG_MAX} printable characters"),
            );
        }
    }
}

fn finite3(values: [f32; 3]) -> bool {
    values.iter().all(|value| value.is_finite())
}

/// One blocking box in world terms, for the "is this point inside a wall" test.
struct Blocker {
    position: Vec3,
    inverse: Quat,
    scale: Vec3,
    boxes: Vec<([f32; 3], [f32; 3])>,
}

impl Blocker {
    fn contains(&self, point: Vec3) -> bool {
        let local = self.inverse.rotate(point - self.position);
        let local = Vec3::new(
            local.x / self.scale.x,
            local.y / self.scale.y,
            local.z / self.scale.z,
        );
        self.boxes.iter().any(|(center, half)| {
            (local.x - center[0]).abs() <= half[0]
                && (local.y - center[1]).abs() <= half[1]
                && (local.z - center[2]).abs() <= half[2]
        })
    }
}

fn default_boxes(asset: &str) -> Vec<([f32; 3], [f32; 3])> {
    let Some(asset) = asset_named(asset) else {
        return Vec::new();
    };
    match engine_environment::object::default_collision(asset) {
        engine_environment::EnvironmentCollision::Box {
            center,
            half_extents,
        } => vec![(
            [center.x, center.y, center.z],
            [half_extents.x, half_extents.y, half_extents.z],
        )],
        _ => Vec::new(),
    }
}

fn blockers(scene: &SceneDocument) -> Vec<Blocker> {
    let shapes: &[CollisionShapeRecord] = &scene.collision_shapes;
    scene
        .objects
        .iter()
        .filter(|object| {
            finite3(object.position)
                && finite3(object.scale)
                && object.yaw.is_finite()
                && object.scale.iter().all(|s| *s != 0.0)
        })
        .filter_map(|object| {
            let boxes = match object.collision {
                None => default_boxes(&object.asset),
                Some(CollisionRecord::None) => Vec::new(),
                Some(CollisionRecord::Box {
                    center,
                    half_extents,
                }) => vec![(center, half_extents)],
                Some(CollisionRecord::Compound { shape }) => shapes
                    .get(shape)
                    .map(|s| s.boxes.iter().map(|b| (b.center, b.half_extents)).collect())
                    .unwrap_or_default(),
            };
            if boxes.is_empty() {
                return None;
            }
            Some(Blocker {
                position: Vec3::new(object.position[0], object.position[1], object.position[2]),
                inverse: Quat::from_yaw_pitch_roll(object.yaw, 0.0, 0.0).conjugate(),
                scale: Vec3::new(object.scale[0], object.scale[1], object.scale[2]),
                boxes,
            })
        })
        .collect()
}

struct SceneCheck<'a> {
    scene: &'a SceneDocument,
    rules: &'a Rules,
    blockers: Vec<Blocker>,
}

impl SceneCheck<'_> {
    /// Checks a point that something stands on: finite, inside the map, not under the
    /// floor, and (for player-ish points) not inside a solid.
    fn check_point(&self, path: &str, position: [f32; 3], solid_check: bool, report: &mut Report) {
        if !finite3(position) {
            report.error("invalid_spawn", path, "the position is not a finite number");
            return;
        }
        let half = self.scene.half_extent;
        if position[0].abs() > half || position[2].abs() > half {
            report.error(
                "invalid_spawn",
                path,
                format!(
                    "({}, {}) is outside the map (half extent {half})",
                    position[0], position[2]
                ),
            );
        }
        if position[1] < self.scene.ground_height - 1.0 {
            report.error(
                "invalid_spawn",
                path,
                format!(
                    "y {} is below the ground (at {})",
                    position[1], self.scene.ground_height
                ),
            );
        }
        if solid_check {
            let probe = Vec3::new(position[0], position[1] + 0.05, position[2]);
            if self.blockers.iter().any(|blocker| blocker.contains(probe)) {
                report.error("invalid_spawn", path, "the point is inside a solid object");
            }
        }
    }
}

fn check_string(text: &str, path: &str, report: &mut Report) {
    if text.chars().count() > NAME_MAX || !printable(text, false) {
        report.error(
            "bad_string",
            path,
            format!("names are at most {NAME_MAX} printable characters"),
        );
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one pass over the document, in document order, reads best as one function"
)]
fn check_scene(
    scene: &SceneDocument,
    rules: &Rules,
    extra_spawns: &[(String, [f32; 3])],
    root: &str,
    report: &mut Report,
) {
    let p = |tail: &str| format!("{root}.{tail}");

    if scene.version != FORMAT_VERSION {
        report.error(
            "unsupported_version",
            p("version"),
            format!(
                "scene version {} is not readable; this build reads {FORMAT_VERSION}",
                scene.version
            ),
        );
    }
    check_string(&scene.name, &p("name"), report);

    if !scene.half_extent.is_finite()
        || scene.half_extent <= 0.0
        || scene.half_extent > rules.max_half_extent
    {
        report.error(
            "bad_extent",
            p("half_extent"),
            format!("half_extent must be in (0, {}]", rules.max_half_extent),
        );
    }
    if !scene.ground_height.is_finite() {
        report.error(
            "non_finite",
            p("ground_height"),
            "ground_height is not a number",
        );
    }

    if !rules.allow_terrain_and_water {
        if scene.terrain.is_some() {
            report.error(
                "unsupported_feature",
                p("terrain"),
                "terrain is not supported",
            );
        }
        if !scene.water.is_empty() {
            report.error("unsupported_feature", p("water"), "water is not supported");
        }
    }

    for (count, limit, code, name) in [
        (
            scene.objects.len(),
            rules.max_objects,
            "too_many_objects",
            "objects",
        ),
        (
            scene.monsters.len(),
            rules.max_monsters,
            "too_many_monsters",
            "monsters",
        ),
        (
            scene.lights.len(),
            rules.max_lights,
            "too_many_lights",
            "lights",
        ),
    ] {
        if count > limit {
            report.error(
                code,
                p(name),
                format!("{count} {name} exceeds the limit of {limit}"),
            );
        }
    }

    check_objects(scene, rules, root, report);
    check_lights(scene, root, report);

    let check = SceneCheck {
        scene,
        rules,
        blockers: blockers(scene),
    };

    if scene.spawn_points.is_empty() && extra_spawns.is_empty() {
        report.error(
            "no_spawn",
            p("spawn_points"),
            "the map needs at least one spawn point",
        );
    }
    for (index, spawn) in scene.spawn_points.iter().enumerate() {
        check.check_point(
            &p(&format!("spawn_points[{index}]")),
            spawn.position,
            true,
            report,
        );
        if !spawn.yaw.is_finite() {
            report.error(
                "invalid_spawn",
                p(&format!("spawn_points[{index}].yaw")),
                "yaw is not finite",
            );
        }
    }
    for (path, position) in extra_spawns {
        check.check_point(path, *position, true, report);
    }

    let monsters = check_monsters(&check, root, report);
    check_dungeon(&check, &monsters, root, report);
}

fn check_objects(scene: &SceneDocument, rules: &Rules, root: &str, report: &mut Report) {
    let (low, high) = rules.scale_range;
    for (index, object) in scene.objects.iter().enumerate() {
        let path = format!("{root}.objects[{index}]");
        if !rules.allows_asset(&object.asset) {
            report.error(
                "missing_asset",
                format!("{path}.asset"),
                format!("'{}' is not an approved asset", object.asset),
            );
        }
        if !finite3(object.position) || !object.yaw.is_finite() {
            report.error(
                "non_finite",
                format!("{path}.position"),
                "position or yaw is not finite",
            );
        }
        if !finite3(object.scale) {
            report.error("non_finite", format!("{path}.scale"), "scale is not finite");
        } else if object.scale.iter().any(|s| !(low..=high).contains(s)) {
            report.error(
                "bad_number",
                format!("{path}.scale"),
                format!("scale must be within {low} to {high}"),
            );
        }
        match object.collision {
            Some(CollisionRecord::Box {
                center,
                half_extents,
            }) if !finite3(center)
                || !finite3(half_extents)
                || half_extents.iter().any(|h| *h <= 0.0) =>
            {
                report.error(
                    "bad_number",
                    format!("{path}.collision"),
                    "a collision box needs finite numbers and positive half extents",
                );
            }
            Some(CollisionRecord::Compound { shape }) if shape >= scene.collision_shapes.len() => {
                report.error(
                    "bad_collision_shape",
                    format!("{path}.collision.shape"),
                    format!("shape {shape} does not exist"),
                );
            }
            _ => {}
        }
    }
    for (index, shape) in scene.collision_shapes.iter().enumerate() {
        for (b, entry) in shape.boxes.iter().enumerate() {
            if !finite3(entry.center)
                || !finite3(entry.half_extents)
                || entry.half_extents.iter().any(|h| *h <= 0.0)
            {
                report.error(
                    "bad_number",
                    format!("{root}.collision_shapes[{index}].boxes[{b}]"),
                    "a collision box needs finite numbers and positive half extents",
                );
            }
        }
    }
}

fn check_lights(scene: &SceneDocument, root: &str, report: &mut Report) {
    for (index, light) in scene.lights.iter().enumerate() {
        let valid = finite3(light.position)
            && finite3(light.color)
            && light.intensity.is_finite()
            && light.intensity >= 0.0
            && light.range.is_finite()
            && light.range > 0.0
            && light.range <= 500.0;
        if !valid {
            report.error(
                "bad_number",
                format!("{root}.lights[{index}]"),
                "a light needs finite numbers, intensity >= 0 and range in (0, 500]",
            );
        }
    }
}

/// What the dungeon check needs to know about the placed monsters.
struct MonsterSummary {
    /// Packs that exist among enabled placements.
    packs: Vec<String>,
    /// Packs that contain a boss archetype.
    boss_packs: Vec<String>,
    /// Sum of completion credit of enabled non-boss placements.
    credit: f32,
}

/// What killing a `kind` placed with `completion` counts towards the objective, if it counts:
/// bosses and non-finite numbers do not.
fn credit_of(kind: EnemyKind, completion: Option<f32>) -> Option<f32> {
    if kind.is_boss() {
        return None;
    }
    Some(completion.unwrap_or(kind.archetype().completion)).filter(|credit| credit.is_finite())
}

/// What an enabled placement of monster `id` counts towards `required_completion` under
/// `rules`: `None` for a boss, an unapproved monster or a disabled placement.
///
/// The validator's own rule, exposed so the editor's running total can never disagree with
/// the `impossible_objective` check.
#[must_use]
pub fn monster_credit(
    rules: &Rules,
    id: &str,
    enabled: bool,
    completion: Option<f32>,
) -> Option<f32> {
    if !enabled {
        return None;
    }
    credit_of(rules.allowed_monster(id)?, completion)
}

fn check_monsters(check: &SceneCheck<'_>, root: &str, report: &mut Report) -> MonsterSummary {
    let rules = check.rules;
    let (low, high) = rules.scale_range;
    let boss_pack = check
        .scene
        .dungeon
        .as_ref()
        .and_then(|d| d.boss_pack.as_deref());
    let mut summary = MonsterSummary {
        packs: Vec::new(),
        boss_packs: Vec::new(),
        credit: 0.0,
    };
    for (index, monster) in check.scene.monsters.iter().enumerate() {
        let path = format!("{root}.monsters[{index}]");
        check.check_point(&format!("{path}.position"), monster.position, false, report);
        if let Some(pack) = &monster.pack {
            check_string(pack, &format!("{path}.pack"), report);
        }
        if let Some(name) = &monster.name {
            check_string(name, &format!("{path}.name"), report);
        }
        if !monster.yaw.is_finite() {
            report.error("non_finite", format!("{path}.yaw"), "yaw is not finite");
        }
        if !monster.scale.is_finite() || !(low..=high).contains(&monster.scale) {
            report.error(
                "bad_number",
                format!("{path}.scale"),
                format!("scale must be within {low} to {high}"),
            );
        }
        if let Some(radius) = monster.aggro_radius
            && !(radius.is_finite() && (0.0..=60.0).contains(&radius))
        {
            report.error(
                "bad_number",
                format!("{path}.aggro_radius"),
                "aggro_radius must be 0 to 60",
            );
        }
        if !(1..=60).contains(&monster.level) {
            report.error(
                "bad_number",
                format!("{path}.level"),
                "level must be 1 to 60",
            );
        }
        if let Some(completion) = monster.completion
            && !(completion.is_finite() && (0.0..=100.0).contains(&completion))
        {
            report.error(
                "bad_number",
                format!("{path}.completion"),
                "completion must be 0 to 100",
            );
        }

        let Some(kind) = rules.allowed_monster(&monster.monster) else {
            report.error(
                "unknown_monster",
                format!("{path}.monster"),
                format!("'{}' is not an approved monster", monster.monster),
            );
            continue;
        };

        if kind.is_boss() {
            let pack = monster.pack.as_deref();
            if pack.is_none() || pack != boss_pack {
                report.error(
                    "boss_outside_boss_pack",
                    format!("{path}.monster"),
                    format!(
                        "boss '{}' may only be placed in the dungeon's boss pack",
                        kind.id()
                    ),
                );
            }
            if let Some(pack) = pack
                && !summary.boss_packs.iter().any(|existing| existing == pack)
            {
                summary.boss_packs.push(pack.to_owned());
            }
        }
        if monster.enabled {
            if let Some(pack) = &monster.pack
                && !summary.packs.contains(pack)
            {
                summary.packs.push(pack.clone());
            }
            if let Some(credit) = credit_of(kind, monster.completion) {
                summary.credit += credit;
            }
        }
    }
    if summary.boss_packs.len() > rules.max_boss_packs {
        report.error(
            "too_many_boss_packs",
            format!("{root}.monsters"),
            format!(
                "{} boss packs; at most {} allowed",
                summary.boss_packs.len(),
                rules.max_boss_packs
            ),
        );
    }
    if check.scene.monsters.is_empty() {
        report.warning(
            "no_monsters",
            format!("{root}.monsters"),
            "the map has no monsters",
        );
    }
    summary
}

fn check_dungeon(
    check: &SceneCheck<'_>,
    monsters: &MonsterSummary,
    root: &str,
    report: &mut Report,
) {
    let rules = check.rules;
    let Some(dungeon) = &check.scene.dungeon else {
        if rules.require_dungeon {
            report.error(
                "missing_dungeon",
                format!("{root}.dungeon"),
                "the map needs a dungeon block",
            );
        }
        return;
    };
    let path = format!("{root}.dungeon");
    check_string(&dungeon.name, &format!("{path}.name"), report);

    let required = dungeon.required_completion;
    if !required.is_finite() || required <= 0.0 {
        report.error(
            "bad_number",
            format!("{path}.required_completion"),
            "required_completion must be a positive number",
        );
    } else if monsters.credit + 1e-3 < required {
        report.error(
            "impossible_objective",
            format!("{path}.required_completion"),
            format!(
                "required completion {required} exceeds the {} the non-boss monsters are worth",
                monsters.credit
            ),
        );
    }

    match dungeon.boss_pack.as_deref() {
        None => report.error(
            "missing_boss",
            format!("{path}.boss_pack"),
            "the dungeon names no boss pack",
        ),
        Some(pack) if !monsters.packs.iter().any(|existing| existing == pack) => report.error(
            "missing_boss",
            format!("{path}.boss_pack"),
            format!("no enabled monster belongs to the pack '{pack}'"),
        ),
        Some(pack) => {
            if !monsters.boss_packs.iter().any(|existing| existing == pack) {
                report.warning(
                    "boss_pack_without_boss",
                    format!("{path}.boss_pack"),
                    format!("pack '{pack}' contains no boss archetype"),
                );
            }
        }
    }

    if let Some(gate) = &dungeon.boss_gate {
        let half = check.scene.half_extent;
        let valid = finite3(gate.center)
            && finite3(gate.half_extents)
            && gate.yaw.is_finite()
            && gate.half_extents.iter().all(|h| *h > 0.0)
            && gate.center[0].abs() <= half
            && gate.center[2].abs() <= half;
        if !valid {
            report.error(
                "bad_boss_gate",
                format!("{path}.boss_gate"),
                "the gate needs finite numbers, positive half extents and a centre inside the map",
            );
        }
    }

    for (index, spawn) in dungeon.add_spawns.iter().enumerate() {
        check.check_point(
            &format!("{path}.add_spawns[{index}]"),
            spawn.position,
            true,
            report,
        );
    }
    if dungeon.add_count > ADD_COUNT_MAX {
        report.error(
            "bad_number",
            format!("{path}.add_count"),
            format!("add_count is at most {ADD_COUNT_MAX}"),
        );
    }
    if dungeon.add_count > 0 && dungeon.add_pool.is_empty() {
        report.error(
            "bad_add_pool",
            format!("{path}.add_pool"),
            "add_count > 0 needs an add_pool",
        );
    }
    for (index, id) in dungeon.add_pool.iter().enumerate() {
        match EnemyKind::from_id(id).filter(|kind| rules.monsters.contains(kind)) {
            None => report.error(
                "bad_add_pool",
                format!("{path}.add_pool[{index}]"),
                format!("'{id}' is not an approved monster"),
            ),
            Some(kind) if kind.is_boss() => report.error(
                "bad_add_pool",
                format!("{path}.add_pool[{index}]"),
                format!("boss '{id}' cannot be an add"),
            ),
            Some(_) => {}
        }
    }
}
