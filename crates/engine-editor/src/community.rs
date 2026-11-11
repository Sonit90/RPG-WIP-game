//! Authoring community maps: the dungeon block, the listing metadata, validation and export.
//!
//! ```text
//!  scene + Meta ──► export_text ──► mapcheck::check_str ──► Report ──► issue list
//!       ▲                                  │ errors?                       │ click
//!       │ undo / redo                      └──► refuse the export          ▼
//!  dungeon panel, community panel                                  select + frame it
//! ```
//!
//! # Where the community half of a map lives
//!
//! A community file is `{format, meta, scene}` (see `docs/community-maps.md`). The `scene`
//! half is the editor's own [`EnvironmentScene`], dungeon block included. The `meta` half is
//! not part of a scene, so the editor keeps it beside the level: in memory as
//! [`Editor::community_meta`], on disk as `<level>.meta.json` next to the level file, written
//! by every save and read when the level is opened. A level with no `.meta.json` is an
//! ordinary level; making it a community map is one button.
//!
//! The metadata is part of every undo snapshot, so a title typed in the wrong box comes back
//! with `CTRL+Z` exactly like a wall.
//!
//! # One validator
//!
//! Nothing here re-implements a rule. The level is written out as the community file it would
//! be, and [`mapcheck::check_str`] reads that text — the same call the server makes on an
//! upload — so what the issue list says is what the upload would say. The limits the text
//! fields enforce, the approved-library warning and the running completion total all come
//! from `mapcheck`'s own constants and functions.
//!
//! # Validation is on demand
//!
//! Never per frame. [`Editor::track_validation`] notes that the level changed, waits for
//! [`SETTLE_SECONDS`] of calm (and for any drag or typed number to finish), then runs once.
//! The *Validate* button runs it immediately.

use std::path::{Path, PathBuf};

use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_environment::monster::{BossGate, DungeonSettings};
use engine_environment::scene_file::{self, SceneDocument};
use engine_environment::spawn::SpawnPoint;
use engine_realm::mapcheck::{
    self, ADD_COUNT_MAX, COMMUNITY_FORMAT, CommunityMap, DESCRIPTION_MAX, DIFFICULTY_RANGE, Meta,
    NAME_MAX, Report, Rules, TAG_MAX, TAGS_MAX, TIMER_RANGE, TITLE_MAX, Target,
};

use crate::commands::Command;
use crate::field::{Charset, FieldAnswer, TextField};
use crate::{Editor, Selected, format_value, theme};

/// The starting map, as the community file it is: a floor, walls, a spawn, one pack of trash,
/// a boss pack, a gate and a boss's adds. It is data, not code — change the level by changing
/// this file, and `the_template_validates_clean` says whether it still passes.
const TEMPLATE: &str = include_str!("../../../assets/templates/community_blank.json");

/// How long the level must sit unchanged before it is validated, in seconds.
pub const SETTLE_SECONDS: f32 = 0.8;

/// The half-extents of a gate the *Place Gate* button puts down: a doorway across a corridor.
const GATE_HALF_EXTENTS: [f32; 3] = [8.0, 3.0, 0.6];

/// The most waypoints one patrol may have (`Rules::max_patrol_points`).
fn patrol_limit() -> usize {
    Rules::default_community().max_patrol_points
}

// ---------------------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------------------

/// Which page of the inspector is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// The selection: position, size, and what the selected thing is.
    #[default]
    Inspector,
    /// The dungeon block: boss, gate, spawns, objective, adds, patrols.
    Dungeon,
    /// The title, description, difficulty and tags the map is listed with.
    Community,
    /// The validator's issues, and export.
    Validate,
}

impl Tab {
    /// Every tab, left to right.
    pub const ALL: [Self; 4] = [
        Self::Inspector,
        Self::Dungeon,
        Self::Community,
        Self::Validate,
    ];

    /// What its tab says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Inspector => "Selection",
            Self::Dungeon => "Dungeon",
            Self::Community => "Listing",
            Self::Validate => "Validate",
        }
    }
}

/// A number of the dungeon block or the listing, typed into a field on one of the tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelField {
    /// The completion that counts as 100 %.
    RequiredCompletion,
    /// How many adds the boss calls.
    AddCount,
    /// The run's clock in seconds; zero is no clock (untimed).
    TimerSeconds,
    /// The boss gate's centre along X.
    GateX,
    /// The boss gate's centre along Y.
    GateY,
    /// The boss gate's centre along Z.
    GateZ,
    /// The boss gate's width, metres.
    GateWidth,
    /// The boss gate's height, metres.
    GateHeight,
    /// The boss gate's thickness, metres.
    GateDepth,
    /// The boss gate's turn about +Y, degrees.
    GateYaw,
    /// The difficulty the author claims, 1 to 5.
    Difficulty,
}

impl LevelField {
    /// How many decimals the field shows.
    #[must_use]
    pub fn decimals(self) -> usize {
        match self {
            Self::AddCount | Self::TimerSeconds | Self::Difficulty => 0,
            _ => 1,
        }
    }

    /// The smallest and largest value the field takes, before the map's own size narrows the
    /// gate's centre.
    fn range(self) -> (f32, f32) {
        match self {
            Self::RequiredCompletion => (0.1, 10_000.0),
            Self::AddCount => (0.0, f32::from(ADD_COUNT_MAX)),
            Self::TimerSeconds => (0.0, *TIMER_RANGE.end() as f32),
            Self::GateX | Self::GateY | Self::GateZ => (-2000.0, 2000.0),
            Self::GateWidth | Self::GateHeight | Self::GateDepth => (0.1, 200.0),
            Self::GateYaw => (-360.0, 360.0),
            Self::Difficulty => (f32::from(DIFFICULTY_RANGE.0), f32::from(DIFFICULTY_RANGE.1)),
        }
    }
}

/// A line of text in the dungeon block or the listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTarget {
    /// The map's title.
    Title,
    /// The map's description.
    Description,
    /// The map's tags, separated by spaces.
    Tags,
    /// The dungeon's name.
    DungeonName,
}

impl TextTarget {
    /// The longest text the field takes, matching the validator.
    fn limit(self) -> usize {
        match self {
            Self::Title => TITLE_MAX,
            Self::Description => DESCRIPTION_MAX,
            Self::Tags => TAGS_MAX * (TAG_MAX + 1) - 1,
            Self::DungeonName => NAME_MAX,
        }
    }
}

/// What a typed value will be applied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// A number.
    Number(LevelField),
    /// A line of text.
    Text(TextTarget),
}

/// A value being typed into one of the tabs.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelEdit {
    /// What it is for.
    pub target: EditTarget,
    /// What has been typed.
    pub field: TextField,
}

/// Why an export did not happen.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportError {
    /// The level is not a community map: it has no listing yet.
    NotCommunity,
    /// The validator found errors; nothing was written.
    Invalid(Report),
    /// The file could not be written.
    Io(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCommunity => write!(formatter, "this level is not a community map"),
            Self::Invalid(report) => write!(
                formatter,
                "{} error(s) must be fixed first",
                report.errors.len()
            ),
            Self::Io(reason) => write!(formatter, "could not write: {reason}"),
        }
    }
}

/// The editor's community-authoring state.
#[derive(Debug, Default)]
pub(crate) struct State {
    /// The listing, once this level is a community map.
    pub meta: Option<Meta>,
    /// Which inspector page is showing.
    pub tab: Tab,
    /// What the validator last said.
    pub report: Option<Report>,
    /// How many times the validator has run, for the tests that say it does not run per frame.
    pub runs: u32,
    /// When the level last changed and has not been validated since.
    pub dirty_at: Option<f32>,
    /// A value being typed into one of the tabs.
    pub edit: Option<LevelEdit>,
    /// The last "approved library only" warning, until the next approved placement.
    pub warning: Option<String>,
    /// Whether the Save As prompt that is open is naming an export.
    pub export_prompt: bool,
    /// Where the last export went.
    pub last_export: Option<PathBuf>,
    /// The editor's revision when the level was last looked at, so an edit made by any route
    /// (a button, undo, a command) is noticed.
    pub seen_revision: u64,
}

// ---------------------------------------------------------------------------------------
// The map as data
// ---------------------------------------------------------------------------------------

/// A listing for a level that has none: its name, an easy difficulty, no tags.
#[must_use]
pub fn default_meta(scene: &EnvironmentScene) -> Meta {
    let title = theme::display_name(&scene.name);
    Meta {
        title: title.chars().take(TITLE_MAX).collect(),
        description: String::new(),
        difficulty: DIFFICULTY_RANGE.0,
        tags: Vec::new(),
    }
}

/// The document a scene is written as, without the collision shapes nothing refers to.
#[must_use]
pub fn document(scene: &EnvironmentScene) -> SceneDocument {
    let mut written = scene.clone();
    written.prune_collision_shapes();
    SceneDocument::from_scene(&written)
}

/// The community file for `scene` and `meta`, as text: exactly what an upload carries.
#[must_use]
pub fn export_text(scene: &EnvironmentScene, meta: &Meta) -> String {
    let map = CommunityMap {
        format: COMMUNITY_FORMAT,
        meta: meta.clone(),
        scene: document(scene),
    };
    serde_json::to_string_pretty(&map).unwrap_or_default()
}

/// What the community validator says about `scene`.
///
/// Without a listing the level is checked as a scene only: a placeholder listing stands in and
/// its issues are left out, because nobody has written a title to complain about yet.
#[must_use]
pub fn check_scene(scene: &EnvironmentScene, meta: Option<&Meta>) -> Report {
    let placeholder;
    let listing = match meta {
        Some(meta) => meta,
        None => {
            placeholder = Meta {
                title: "Placeholder".to_owned(),
                ..default_meta(scene)
            };
            &placeholder
        }
    };
    let mut report = mapcheck::check_str(&export_text(scene, listing), &Rules::default_community());
    if meta.is_none() {
        report.errors.retain(|issue| issue.target() != Target::Meta);
        report
            .warnings
            .retain(|issue| issue.target() != Target::Meta);
    }
    report
}

/// Reads a community file's text into its listing and level.
///
/// Lenient where the validator is strict: a map with errors still opens, so its author can fix
/// it.
///
/// # Errors
///
/// A sentence naming what is wrong with the file.
pub fn read_text(text: &str) -> Result<(Meta, EnvironmentScene), String> {
    let map: CommunityMap =
        serde_json::from_str(text).map_err(|error| format!("not a community map: {error}"))?;
    let scene = map
        .scene
        .to_scene("community map")
        .map_err(|error| error.to_string())?;
    Ok((map.meta, scene))
}

/// The starting map "New community map" opens.
///
/// # Errors
///
/// A sentence, if the template asset is damaged. A test reads it, so a damaged one never ships.
pub fn template() -> Result<(Meta, EnvironmentScene), String> {
    read_text(TEMPLATE)
}

/// The file beside a level that holds its listing.
#[must_use]
pub fn sidecar_path(level: &Path) -> PathBuf {
    level.with_extension("meta.json")
}

/// The listing stored beside `level`, if there is one.
#[must_use]
pub fn read_sidecar(level: &Path) -> Option<Meta> {
    let text = std::fs::read_to_string(sidecar_path(level)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Stores `meta` beside `level`.
///
/// # Errors
///
/// A sentence if it cannot be written.
pub fn write_sidecar(level: &Path, meta: &Meta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(meta).map_err(|error| error.to_string())?;
    std::fs::write(sidecar_path(level), text).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------------------
// The dungeon block as numbers
// ---------------------------------------------------------------------------------------

/// What the placed monsters add up to against what the dungeon asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Completion {
    /// The summed completion credit of every enabled, non-boss monster — the validator's own
    /// sum (`mapcheck::monster_credit`).
    pub placed: f32,
    /// The dungeon's `required_completion`, if it has a dungeon block.
    pub required: Option<f32>,
}

impl Completion {
    /// Whether the objective can be met. A level with no dungeon block asks for nothing.
    #[must_use]
    pub fn is_enough(&self) -> bool {
        self.required
            .is_none_or(|required| self.placed + 1e-3 >= required)
    }
}

/// The completion the placed monsters are worth, and what the dungeon requires.
#[must_use]
pub fn completion(scene: &EnvironmentScene) -> Completion {
    let rules = Rules::default_community();
    let placed = scene
        .monsters
        .iter()
        .filter_map(|monster| {
            mapcheck::monster_credit(
                &rules,
                &monster.monster,
                monster.enabled,
                monster.completion,
            )
        })
        .sum();
    Completion {
        placed,
        required: scene
            .dungeon
            .as_ref()
            .map(|dungeon| dungeon.required_completion),
    }
}

/// The distinct pack names of the placed monsters, in the order they first appear.
#[must_use]
pub fn pack_names(scene: &EnvironmentScene) -> Vec<String> {
    let mut packs: Vec<String> = Vec::new();
    for pack in scene
        .monsters
        .iter()
        .filter_map(|monster| monster.pack.as_ref())
    {
        if !packs.contains(pack) {
            packs.push(pack.clone());
        }
    }
    packs
}

/// The value of one of the tabs' numbers.
#[must_use]
pub fn field_value(
    scene: &EnvironmentScene,
    meta: Option<&Meta>,
    field: LevelField,
) -> Option<f32> {
    use LevelField as F;
    if field == F::Difficulty {
        return meta.map(|meta| f32::from(meta.difficulty));
    }
    let dungeon = scene.dungeon.as_ref()?;
    match field {
        F::RequiredCompletion => Some(dungeon.required_completion),
        F::AddCount => Some(f32::from(dungeon.add_count)),
        F::TimerSeconds => Some(dungeon.timer_seconds.unwrap_or(0) as f32),
        F::Difficulty => None,
        _ => {
            let gate = dungeon.boss_gate?;
            Some(match field {
                F::GateX => gate.center.x,
                F::GateY => gate.center.y,
                F::GateZ => gate.center.z,
                F::GateWidth => gate.half_extents.x * 2.0,
                F::GateHeight => gate.half_extents.y * 2.0,
                F::GateDepth => gate.half_extents.z * 2.0,
                _ => gate.yaw.to_degrees(),
            })
        }
    }
}

/// Sets one of the tabs' numbers, clamped to what the validator accepts. Returns whether it
/// changed.
pub fn set_field(
    scene: &mut EnvironmentScene,
    meta: &mut Option<Meta>,
    field: LevelField,
    value: f32,
) -> bool {
    use LevelField as F;
    if !value.is_finite() {
        return false;
    }
    let (low, high) = field.range();
    let mut value = value.clamp(low, high);
    let half_extent = scene.half_extent;
    if field == F::Difficulty {
        let Some(meta) = meta.as_mut() else {
            return false;
        };
        let next = value.round() as u8;
        let changed = meta.difficulty != next;
        meta.difficulty = next;
        return changed;
    }
    let Some(dungeon) = scene.dungeon.as_mut() else {
        return false;
    };
    match field {
        F::RequiredCompletion => {
            let changed = dungeon.required_completion != value;
            dungeon.required_completion = value;
            changed
        }
        F::AddCount => {
            let next = value.round() as u8;
            let changed = dungeon.add_count != next;
            dungeon.add_count = next;
            changed
        }
        F::TimerSeconds => {
            // Zero is "no clock"; anything else is held to the validator's range.
            let next = match value.round() as u32 {
                0 => None,
                seconds => Some(seconds.clamp(*TIMER_RANGE.start(), *TIMER_RANGE.end())),
            };
            let changed = dungeon.timer_seconds != next;
            dungeon.timer_seconds = next;
            changed
        }
        F::Difficulty => false,
        _ => {
            let Some(gate) = dungeon.boss_gate.as_mut() else {
                return false;
            };
            let before = *gate;
            match field {
                F::GateX => {
                    value = value.clamp(-half_extent, half_extent);
                    gate.center.x = value;
                }
                F::GateY => gate.center.y = value,
                F::GateZ => {
                    value = value.clamp(-half_extent, half_extent);
                    gate.center.z = value;
                }
                F::GateWidth => gate.half_extents.x = value * 0.5,
                F::GateHeight => gate.half_extents.y = value * 0.5,
                F::GateDepth => gate.half_extents.z = value * 0.5,
                _ => gate.yaw = value.to_radians(),
            }
            *gate != before
        }
    }
}

/// The text of one of the tabs' lines.
#[must_use]
pub fn text_value(
    scene: &EnvironmentScene,
    meta: Option<&Meta>,
    target: TextTarget,
) -> Option<String> {
    match target {
        TextTarget::Title => meta.map(|meta| meta.title.clone()),
        TextTarget::Description => meta.map(|meta| meta.description.clone()),
        TextTarget::Tags => meta.map(|meta| meta.tags.join(" ")),
        TextTarget::DungeonName => scene.dungeon.as_ref().map(|dungeon| dungeon.name.clone()),
    }
}

/// Sets one of the tabs' lines, cut to the validator's limits. Returns whether it changed.
pub fn set_text(
    scene: &mut EnvironmentScene,
    meta: &mut Option<Meta>,
    target: TextTarget,
    text: &str,
) -> bool {
    let text: String = text.chars().take(target.limit()).collect();
    match target {
        TextTarget::Title => meta
            .as_mut()
            .is_some_and(|meta| std::mem::replace(&mut meta.title, text.clone()) != text),
        TextTarget::Description => meta
            .as_mut()
            .is_some_and(|meta| std::mem::replace(&mut meta.description, text.clone()) != text),
        TextTarget::Tags => {
            let tags: Vec<String> = text
                .split_whitespace()
                .take(TAGS_MAX)
                .map(|tag| tag.chars().take(TAG_MAX).collect())
                .collect();
            meta.as_mut()
                .is_some_and(|meta| std::mem::replace(&mut meta.tags, tags.clone()) != tags)
        }
        TextTarget::DungeonName => scene
            .dungeon
            .as_mut()
            .is_some_and(|dungeon| std::mem::replace(&mut dungeon.name, text.clone()) != text),
    }
}

/// A dungeon block for a level that has none: named after the level, complete at what its
/// monsters are worth (at least one).
fn new_dungeon(scene: &EnvironmentScene) -> DungeonSettings {
    let name: String = theme::display_name(&scene.name)
        .to_uppercase()
        .chars()
        .take(NAME_MAX)
        .collect();
    let required = completion(scene).placed.floor().max(1.0);
    DungeonSettings::new(name, required)
}

/// The level's dungeon block, made first if it has none.
fn ensure_dungeon(scene: &mut EnvironmentScene) -> &mut DungeonSettings {
    let fresh = new_dungeon(scene);
    scene.dungeon.get_or_insert(fresh)
}

// ---------------------------------------------------------------------------------------
// The editor's side of it
// ---------------------------------------------------------------------------------------

impl Editor {
    /// The listing this level is published with, once it is a community map.
    #[must_use]
    pub fn community_meta(&self) -> Option<&Meta> {
        self.community.meta.as_ref()
    }

    /// Whether this level is being authored as a community map.
    #[must_use]
    pub fn is_community(&self) -> bool {
        self.community.meta.is_some()
    }

    /// Which inspector page is showing.
    #[must_use]
    pub fn tab(&self) -> Tab {
        self.community.tab
    }

    /// What the validator last said about the level, if it has been asked.
    #[must_use]
    pub fn validation(&self) -> Option<&Report> {
        self.community.report.as_ref()
    }

    /// How many times the validator has run in this session.
    #[must_use]
    pub fn validation_runs(&self) -> u32 {
        self.community.runs
    }

    /// Whether the level has changed since the report on screen was made.
    #[must_use]
    pub fn validation_is_stale(&self) -> bool {
        self.community.dirty_at.is_some()
    }

    /// The last "approved library only" warning.
    #[must_use]
    pub fn approval_warning(&self) -> Option<&str> {
        self.community.warning.as_deref()
    }

    /// The value being typed into one of the tabs, if there is one.
    #[must_use]
    pub fn level_edit(&self) -> Option<&LevelEdit> {
        self.community.edit.as_ref()
    }

    /// Where the last export went.
    #[must_use]
    pub fn last_export(&self) -> Option<&Path> {
        self.community.last_export.as_deref()
    }

    /// Whether the Save As prompt that is open is naming an export.
    #[must_use]
    pub fn is_export_prompt(&self) -> bool {
        self.community.export_prompt
    }

    /// Runs the validator now and keeps its report.
    pub fn validate_now(&mut self, scene: &EnvironmentScene) -> &Report {
        let report = check_scene(scene, self.community.meta.as_ref());
        self.community.runs += 1;
        self.community.dirty_at = None;
        self.community.report.insert(report)
    }

    /// Notes that the level changed (`edited`) and validates it once it has been left alone.
    ///
    /// Called once per frame; runs the validator at most once per burst of edits. Only for a
    /// community map: an ordinary level is never checked behind its author's back.
    pub(crate) fn track_validation(&mut self, scene: &EnvironmentScene, edited: bool) {
        if !self.active || self.community.meta.is_none() {
            return;
        }
        let edited = edited || self.community.seen_revision != self.revision;
        self.community.seen_revision = self.revision;
        if edited {
            self.community.dirty_at = Some(self.clock);
        }
        let Some(since) = self.community.dirty_at else {
            return;
        };
        // A drag or a typed number is not finished: the level will change again.
        let busy = self.gizmo_drag.is_some()
            || self.marquee.is_some()
            || self.scrub.is_some()
            || self.paint.is_some()
            || self.inspector_edit.is_some()
            || self.community.edit.is_some();
        if busy {
            self.community.dirty_at = Some(self.clock);
            return;
        }
        if self.clock - since >= SETTLE_SECONDS {
            self.validate_now(scene);
        }
    }

    /// Selects and frames what issue number `index` (errors, then warnings) is about.
    ///
    /// An issue about the dungeon block or the listing opens that tab instead. Returns what the
    /// issue pointed at, or `None` if there is no such issue or its target no longer exists.
    pub fn select_issue(&mut self, scene: &EnvironmentScene, index: usize) -> Option<Target> {
        let report = self.community.report.as_ref()?;
        let issue = report.errors.iter().chain(&report.warnings).nth(index)?;
        let target = issue.target();
        let message = format!("{}: {}", issue.code, issue.message);
        let what = match target {
            Target::Object(i) => (i < scene.objects.len()).then_some(Selected::Object(i)),
            Target::Monster(i) => (i < scene.monsters.len()).then_some(Selected::Monster(i)),
            Target::SpawnPoint(i) => (i < scene.spawn_points.len()).then_some(Selected::Spawn(i)),
            Target::Light(i) => (i < scene.lights.len()).then_some(Selected::Light(i)),
            Target::Dungeon | Target::BossGate | Target::AddSpawn(_) => {
                self.community.tab = Tab::Dungeon;
                None
            }
            Target::Meta => {
                self.community.tab = Tab::Community;
                None
            }
            Target::Map => None,
        };
        if let Some(what) = what {
            self.select(what, false);
            self.frame_selection(scene);
        } else if matches!(
            target,
            Target::Object(_) | Target::Monster(_) | Target::SpawnPoint(_) | Target::Light(_)
        ) {
            self.status = "that placement is gone: validate again".to_owned();
            return None;
        }
        // After the framing, which has its own thing to say.
        self.status = message;
        Some(target)
    }

    /// Writes the community file for the level to `path`, if the validator accepts it.
    ///
    /// The file is [`export_text`] verbatim, so `mapcheck` reading it back finds exactly the
    /// report this call checked. Errors refuse the export and nothing is written; warnings do
    /// not.
    ///
    /// # Errors
    ///
    /// [`ExportError`] saying why nothing was written.
    pub fn export_community(
        &mut self,
        scene: &EnvironmentScene,
        path: &Path,
    ) -> Result<Report, ExportError> {
        let Some(meta) = self.community.meta.clone() else {
            self.status = "Make this a community map first (Listing tab)".to_owned();
            self.community.tab = Tab::Community;
            return Err(ExportError::NotCommunity);
        };
        let text = export_text(scene, &meta);
        let report = mapcheck::check_str(&text, &Rules::default_community());
        self.community.runs += 1;
        self.community.dirty_at = None;
        self.community.report = Some(report.clone());
        if !report.is_valid() {
            self.status = format!(
                "Export refused: {} error(s). They are listed under Validate",
                report.errors.len()
            );
            self.community.tab = Tab::Validate;
            return Err(ExportError::Invalid(report));
        }
        let written = match path.parent() {
            Some(directory) if !directory.as_os_str().is_empty() => {
                std::fs::create_dir_all(directory)
            }
            _ => Ok(()),
        }
        .and_then(|()| std::fs::write(path, &text));
        if let Err(error) = written {
            self.status = format!("COULD NOT EXPORT: {error}");
            return Err(ExportError::Io(error.to_string()));
        }
        self.community.last_export = Some(path.to_path_buf());
        self.status = format!("Exported to {}", path.display());
        Ok(report)
    }

    /// Opens the Save As prompt to name an export.
    fn begin_export_prompt(&mut self) {
        let stem = self
            .save_path
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        self.save_as = Some(TextField::new(&stem, Charset::FileName));
        self.community.export_prompt = true;
        self.status = "Export: type a name, Enter writes it".to_owned();
    }

    /// Finishes the export prompt: writes `<level folder>/<name>.community.json`.
    pub(crate) fn finish_export_prompt(&mut self, scene: &EnvironmentScene, name: &str) {
        let directory = self
            .save_path
            .parent()
            .map_or_else(|| PathBuf::from("assets/scenes"), Path::to_path_buf);
        let path = directory.join(format!("{name}.community.json"));
        // The outcome is in the status line and the issue list; the prompt has nothing to add.
        let _ = self.export_community(scene, &path);
    }

    /// Starts a new community map from the template: a working copy next to the current level,
    /// opened the way any level is opened from the browser.
    pub(crate) fn new_community_map(&mut self) {
        match template() {
            Ok((meta, mut scene)) => {
                if let Some(copy) = self.write_working_copy(&scene, &meta, "community_map") {
                    // A new map is called what its file is called.
                    if let Some(name) = copy.file_stem() {
                        scene.name = name.to_string_lossy().into_owned().into();
                        let _ = scene_file::save(&scene, &copy);
                    }
                    self.request_open(copy);
                }
            }
            Err(error) => self.status = format!("COULD NOT START: {error}"),
        }
    }

    /// Writes `scene` and `meta` as a level beside the current one, under a name not taken yet,
    /// and returns where it went.
    fn write_working_copy(
        &mut self,
        scene: &EnvironmentScene,
        meta: &Meta,
        stem: &str,
    ) -> Option<PathBuf> {
        let directory = self
            .save_path
            .parent()
            .map_or_else(|| PathBuf::from("assets/scenes"), Path::to_path_buf);
        let stem: String = stem
            .trim_end_matches(".community")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let stem = if stem.is_empty() {
            "community_map".to_owned()
        } else {
            stem
        };
        let mut path = directory.join(format!("{stem}.json"));
        let mut number = 2;
        while path.exists() {
            path = directory.join(format!("{stem}_{number}.json"));
            number += 1;
        }
        let saved = scene_file::save(scene, &path)
            .map_err(|error| error.to_string())
            .and_then(|()| write_sidecar(&path, meta));
        if let Err(error) = saved {
            self.status = format!("COULD NOT START: {error}");
            return None;
        }
        log::info!(
            "editor: community map '{}' written to {} as a working copy",
            scene.name,
            path.display()
        );
        Some(path)
    }

    /// Queues `path` for the application to open, turning a community file into a working copy
    /// of it first.
    pub(crate) fn queue_open(&mut self, path: PathBuf) {
        if self.open_community_file(&path) {
            return;
        }
        self.status = format!("Opening {}", path.display());
        self.open_request = Some(path);
    }

    /// If `path` is a community file rather than a level, makes a working copy of it and asks to
    /// open that. Returns whether it was one: `false` opens as an ordinary level.
    ///
    /// The application opens levels, and a community file is not one: it is a level plus its
    /// listing. The copy is the level, with the listing beside it where [`read_sidecar`] finds
    /// it, so the application's own open path loads it and [`Editor::opened`] picks the listing
    /// up.
    fn open_community_file(&mut self, path: &Path) -> bool {
        if path.extension().is_none_or(|extension| extension != "json") {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        match read_text(&text) {
            Ok((meta, scene)) => {
                let stem = path
                    .file_stem()
                    .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
                if let Some(copy) = self.write_working_copy(&scene, &meta, &stem) {
                    self.status = format!("Opening {}", copy.display());
                    self.open_request = Some(copy);
                }
                true
            }
            Err(error) => {
                // A level has no `format` and no `meta`; only a file that claims to be a
                // community map and is not a good one is worth a message.
                let claims = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .is_some_and(|value| {
                        value.get("meta").is_some() && value.get("format").is_some()
                    });
                if claims {
                    self.status = format!("COULD NOT OPEN: {error}");
                }
                claims
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // The approved library
    // -----------------------------------------------------------------------------------

    /// Warns, in a community map, that an asset just placed is not in the approved library.
    pub(crate) fn check_asset_approved(&mut self, name: &str) {
        if self.community.meta.is_none() {
            return;
        }
        if Rules::default_community().allows_asset(name) {
            self.community.warning = None;
            return;
        }
        let message = format!(
            "Approved library only: '{}' cannot go in a community map",
            theme::display_name(name)
        );
        self.status.clone_from(&message);
        self.community.warning = Some(message);
    }

    /// Warns, in a community map, that a monster just placed is not an approved archetype.
    pub(crate) fn check_monster_approved(&mut self, id: &str) {
        if self.community.meta.is_none() {
            return;
        }
        if Rules::default_community().allowed_monster(id).is_some() {
            self.community.warning = None;
            return;
        }
        let message = format!(
            "Approved library only: '{}' cannot go in a community map",
            theme::display_name(id)
        );
        self.status.clone_from(&message);
        self.community.warning = Some(message);
    }

    /// [`Editor::check_asset_approved`] or [`Editor::check_monster_approved`] for whatever the
    /// browser has chosen to place next.
    pub(crate) fn check_chosen_approved(&mut self) {
        if let Some(entry) = self.browser.selected_monster() {
            let id = entry.id.clone();
            self.check_monster_approved(&id);
        } else {
            let name = self.asset().name().to_owned();
            self.check_asset_approved(&name);
        }
    }

    /// Warns about the first selected monster whose type is not approved.
    pub(crate) fn check_selected_monsters_approved(&mut self, scene: &EnvironmentScene) {
        let unapproved = self
            .selected
            .iter()
            .filter_map(|what| what.monster())
            .filter_map(|index| scene.monsters.get(index))
            .find(|monster| {
                Rules::default_community()
                    .allowed_monster(&monster.monster)
                    .is_none()
            })
            .map(|monster| monster.monster.clone());
        match unapproved {
            Some(id) => self.check_monster_approved(&id),
            None if self.community.meta.is_some() => self.community.warning = None,
            None => {}
        }
    }

    // -----------------------------------------------------------------------------------
    // Typing into the tabs
    // -----------------------------------------------------------------------------------

    /// Starts typing into `target`.
    pub(crate) fn begin_level_edit(&mut self, scene: &EnvironmentScene, target: EditTarget) {
        let meta = self.community.meta.as_ref();
        let field = match target {
            EditTarget::Number(field) => {
                let Some(value) = field_value(scene, meta, field) else {
                    return;
                };
                TextField::new(&format_value(value), Charset::Number)
            }
            EditTarget::Text(text) => {
                let Some(current) = text_value(scene, meta, text) else {
                    return;
                };
                let charset = Charset::ByKey { max: text.limit() };
                if text == TextTarget::Description {
                    // Prose is added to, not replaced by the first key.
                    TextField::appending(&current, charset)
                } else {
                    TextField::new(&current, charset)
                }
            }
        };
        self.community.edit = Some(LevelEdit { target, field });
        self.status = "Type: Enter sets it, Esc cancels".to_owned();
    }

    /// One frame of typing into a tab's field. Returns whether the level changed.
    pub(crate) fn update_level_edit(
        &mut self,
        input: &engine_core::input::InputState,
        scene: &mut EnvironmentScene,
    ) -> bool {
        let Some(edit) = self.community.edit.as_mut() else {
            return false;
        };
        match edit.field.update(input) {
            FieldAnswer::Editing => false,
            FieldAnswer::Cancel => {
                self.community.edit = None;
                self.status = "not changed".to_owned();
                false
            }
            FieldAnswer::Commit | FieldAnswer::CommitNext => self.commit_level_edit(scene),
        }
    }

    /// Applies what was typed, as one undo step. Returns whether the level changed.
    pub(crate) fn commit_level_edit(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(edit) = self.community.edit.take() else {
            return false;
        };
        let before = self.capture(scene);
        let applied = match edit.target {
            EditTarget::Number(field) => {
                let Some(value) = edit.field.number() else {
                    self.status = format!("'{}' is not a number", edit.field.text());
                    return false;
                };
                set_field(scene, &mut self.community.meta, field, value)
            }
            EditTarget::Text(text) => {
                set_text(scene, &mut self.community.meta, text, edit.field.text())
            }
        };
        if !applied {
            return false;
        }
        self.push_undo(before);
        self.water_touched = true;
        self.status = "set".to_owned();
        true
    }

    // -----------------------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------------------

    /// Runs `change` on the level as one undo step. Returns whether it changed anything.
    fn edit_level(
        &mut self,
        scene: &mut EnvironmentScene,
        change: impl FnOnce(&mut EnvironmentScene) -> bool,
    ) -> bool {
        let before = self.capture(scene);
        if !change(scene) {
            return false;
        }
        self.push_undo(before);
        self.water_touched = true;
        true
    }

    /// The ground at the middle of the viewport, snapped: where the panels' "here" buttons put
    /// things.
    fn centre_point(&self, scene: &EnvironmentScene) -> Vec3 {
        let centre = (self.viewport.0 as f32 * 0.5, self.viewport.1 as f32 * 0.5);
        let ground = crate::ground_under(centre, &self.view_projection, self.viewport, scene)
            .unwrap_or_else(|| {
                let ahead = self.camera.position + self.camera.forward() * 12.0;
                Vec3::new(ahead.x, scene.ground_at(ahead), ahead.z)
            });
        let snapped = self.snap(ground);
        Vec3::new(snapped.x, scene.ground_at(snapped), snapped.z)
    }

    /// Carries out the commands of the community panels. Returns whether the level changed.
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per button of the panels, each a few lines"
    )]
    pub(crate) fn execute_community(
        &mut self,
        command: Command,
        scene: &mut EnvironmentScene,
    ) -> bool {
        match command {
            Command::SetTab(tab) => {
                self.community.tab = tab;
                self.inspector_scroll = 0.0;
                false
            }
            Command::MakeCommunity => {
                if self.community.meta.is_some() {
                    return false;
                }
                let before = self.capture(scene);
                self.community.meta = Some(default_meta(scene));
                self.push_undo(before);
                self.community.dirty_at = Some(self.clock);
                self.community.tab = Tab::Community;
                self.status = "This level is now a community map".to_owned();
                false
            }
            Command::NewCommunityMap => {
                self.new_community_map();
                false
            }
            Command::Validate => {
                let report = self.validate_now(scene);
                let (errors, warnings) = (report.errors.len(), report.warnings.len());
                self.status = format!("Validated: {errors} error(s), {warnings} warning(s)");
                self.community.tab = Tab::Validate;
                false
            }
            Command::ExportCommunity => {
                if self.community.meta.is_none() {
                    self.status = "Make this a community map first (Listing tab)".to_owned();
                    self.community.tab = Tab::Community;
                } else {
                    self.begin_export_prompt();
                }
                false
            }
            Command::SelectIssue(index) => {
                let _ = self.select_issue(scene, index);
                false
            }
            Command::AddDungeon => {
                if scene.dungeon.is_some() {
                    return false;
                }
                let changed = self.edit_level(scene, |scene| {
                    scene.dungeon = Some(new_dungeon(scene));
                    true
                });
                self.status = "Added a dungeon block".to_owned();
                changed
            }
            Command::CycleBossPack(forward) => {
                let packs = pack_names(scene);
                if packs.is_empty() {
                    self.status =
                        "Place some monsters first: a boss pack is one of their packs".to_owned();
                    return false;
                }
                let changed = self.edit_level(scene, |scene| {
                    let dungeon = ensure_dungeon(scene);
                    let current = dungeon
                        .boss_pack
                        .as_ref()
                        .and_then(|pack| packs.iter().position(|known| known == pack));
                    let next = match (current, forward) {
                        (None, true) => 0,
                        (None, false) => packs.len() - 1,
                        (Some(at), true) => (at + 1) % packs.len(),
                        (Some(at), false) => (at + packs.len() - 1) % packs.len(),
                    };
                    dungeon.boss_pack = Some(packs[next].clone());
                    true
                });
                if let Some(pack) = scene
                    .dungeon
                    .as_ref()
                    .and_then(|dungeon| dungeon.boss_pack.clone())
                {
                    self.status = format!("Boss pack: {pack}");
                }
                changed
            }
            Command::MarkBossPack => {
                let Some(Selected::Monster(index)) = self.selection() else {
                    self.status = "Select a monster of the pack first".to_owned();
                    return false;
                };
                let Some(pack) = scene
                    .monsters
                    .get(index)
                    .and_then(|monster| monster.pack.clone())
                else {
                    self.status = "That monster has no pack".to_owned();
                    return false;
                };
                let changed = self.edit_level(scene, |scene| {
                    let dungeon = ensure_dungeon(scene);
                    let changed = dungeon.boss_pack.as_deref() != Some(pack.as_str());
                    dungeon.boss_pack = Some(pack.clone());
                    changed
                });
                self.status = format!("Boss pack: {pack}");
                changed
            }
            Command::AssignBossPack => {
                let Some(pack) = scene
                    .dungeon
                    .as_ref()
                    .and_then(|dungeon| dungeon.boss_pack.clone())
                else {
                    self.status = "Choose a boss pack first".to_owned();
                    return false;
                };
                let monsters: Vec<usize> = self
                    .selected
                    .iter()
                    .filter_map(|what| what.monster())
                    .collect();
                if monsters.is_empty() {
                    self.status = "Select the monsters to move into the boss pack".to_owned();
                    return false;
                }
                let changed = self.edit_level(scene, |scene| {
                    let mut changed = false;
                    for index in &monsters {
                        if let Some(monster) = scene.monsters.get_mut(*index)
                            && monster.pack.as_deref() != Some(pack.as_str())
                        {
                            monster.pack = Some(pack.clone());
                            changed = true;
                        }
                    }
                    changed
                });
                self.status = format!("{} monster(s) now in {pack}", monsters.len());
                changed
            }
            Command::AddGate => {
                let centre = self.centre_point(scene);
                let changed = self.edit_level(scene, |scene| {
                    let dungeon = ensure_dungeon(scene);
                    // A gate that exists is moved and keeps its size and turn.
                    match dungeon.boss_gate.as_mut() {
                        Some(gate) => gate.center = centre,
                        None => {
                            dungeon.boss_gate = Some(BossGate {
                                center: centre,
                                half_extents: Vec3::new(
                                    GATE_HALF_EXTENTS[0],
                                    GATE_HALF_EXTENTS[1],
                                    GATE_HALF_EXTENTS[2],
                                ),
                                yaw: 0.0,
                            });
                        }
                    }
                    true
                });
                self.status = format!("Gate at {:.0} {:.0}", centre.x, centre.z);
                changed
            }
            Command::RemoveGate => self.edit_level(scene, |scene| {
                scene
                    .dungeon
                    .as_mut()
                    .is_some_and(|dungeon| dungeon.boss_gate.take().is_some())
            }),
            Command::AddSpawnHere => {
                let at = self.centre_point(scene);
                let yaw = scene.spawn_points.last().map_or(0.0, |last| last.yaw);
                let changed = self.edit_level(scene, |scene| {
                    scene.push_spawn(SpawnPoint { position: at, yaw });
                    true
                });
                self.selected.clear();
                self.selected
                    .push(Selected::Spawn(scene.spawn_points.len() - 1));
                self.status = format!("Spawn point at {:.0} {:.0}", at.x, at.z);
                changed
            }
            Command::AddPoolType => {
                let Some(Selected::Monster(index)) = self.selection() else {
                    self.status = "Select a monster of the type to add".to_owned();
                    return false;
                };
                let Some(id) = scene.monsters.get(index).map(|m| m.monster.clone()) else {
                    return false;
                };
                let rules = Rules::default_community();
                match rules.allowed_monster(&id) {
                    None => {
                        self.status = format!("'{id}' is not an approved monster");
                        return false;
                    }
                    Some(kind) if kind.is_boss() => {
                        self.status = "A boss cannot be one of the boss's adds".to_owned();
                        return false;
                    }
                    Some(_) => {}
                }
                let changed = self.edit_level(scene, |scene| {
                    let dungeon = ensure_dungeon(scene);
                    if dungeon.add_pool.contains(&id) {
                        return false;
                    }
                    dungeon.add_pool.push(id.clone());
                    true
                });
                self.status = format!("{} can be called as an add", theme::display_name(&id));
                changed
            }
            Command::ClearPool => self.edit_level(scene, |scene| {
                scene.dungeon.as_mut().is_some_and(|dungeon| {
                    let had = !dungeon.add_pool.is_empty();
                    dungeon.add_pool.clear();
                    had
                })
            }),
            Command::ToggleDuplicateAdds => self.edit_level(scene, |scene| {
                scene.dungeon.as_mut().is_some_and(|dungeon| {
                    dungeon.allow_duplicate_adds = !dungeon.allow_duplicate_adds;
                    true
                })
            }),
            Command::AddPatrolPoint => {
                let Some(Selected::Monster(index)) = self.selection() else {
                    self.status = "Select the monster that walks the patrol".to_owned();
                    return false;
                };
                let at = self.centre_point(scene);
                let full = scene
                    .monsters
                    .get(index)
                    .is_none_or(|monster| monster.patrol.len() >= patrol_limit());
                if full {
                    self.status = format!("A patrol has at most {} waypoints", patrol_limit());
                    return false;
                }
                let changed = self.edit_level(scene, |scene| {
                    scene.monsters.get_mut(index).is_some_and(|monster| {
                        monster.patrol.push([at.x, at.z]);
                        true
                    })
                });
                self.status = format!("Waypoint at {:.0} {:.0}", at.x, at.z);
                changed
            }
            Command::ClearPatrol => {
                let Some(Selected::Monster(index)) = self.selection() else {
                    return false;
                };
                self.edit_level(scene, |scene| {
                    scene.monsters.get_mut(index).is_some_and(|monster| {
                        let had = !monster.patrol.is_empty();
                        monster.patrol.clear();
                        had
                    })
                })
            }
            _ => false,
        }
    }
}
