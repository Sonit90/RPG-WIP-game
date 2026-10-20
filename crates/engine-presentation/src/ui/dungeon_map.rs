//! The dungeon map: a circular minimap in the top-right corner, and a full-screen tactical map
//! with an encounter journal beside it.
//!
//! ```text
//!  EnvironmentScene ──► DungeonMapInfo      static: frame, encounters, their monsters   (load)
//!  Simulation       ──► MapMarkers          dynamic: player, party, view, run progress  (frame)
//!                            │
//!    DungeonMap::observe / update / handle_keys / advance   input, selection, motion
//!                            │
//!    push_minimap / push_map_screen  ──►  UiDrawList: one MapImage, panels, insignias, text
//! ```
//!
//! # What is baked and what is not
//!
//! The level's floor and walls are a picture (`UiCommand::MapImage`), baked once from the real
//! geometry. Everything over it is data: encounter insignias come from the level's monster
//! placements through [`engine_environment::map::encounters`], so they are hoverable and
//! selectable and follow the editor without a re-bake; the player's and the party's arrows,
//! each pack's cleared state and the dungeon's completion come from the simulation every frame.
//!
//! # One mapping
//!
//! Every conversion between the world and the screen goes through [`MapFrame`] and one of two
//! projections — the minimap's (player-centred, optionally turned with the view) and the
//! canvas's (panned and zoomed). The picture and every marker use the same one.
//!
//! # Look and feel
//!
//! Drawn entirely from [`super::map_style`]: layered blue-black surfaces with rounded corners
//! and soft shadows (`UiCommand::Panel`), restrained bronze, Inter for everything read and
//! Cinzel for titles (`UiCommand::Label`, distance-field fonts), custom insignias. Transitions
//! — hover emphasis, the journal's reveal, zoom — ease over 120-180 ms and stop when they
//! arrive; with reduced motion they jump.
//!
//! # The journal reads the game's own tables
//!
//! A monster's name, role, summary, health and abilities come from `engine_game::enemy`; every
//! number on an ability is read off its [`EnemyAbilityDef`] row, and its description and
//! counterplay are the row's own columns. A boss's loot is its archetype's [`LootEntry`] rows
//! resolved through the item table. An empty loot table reads as a deliberate empty state.

use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_environment::map::{EncounterKind, MapFrame};
use engine_game::DamageType;
use engine_game::character::{ItemFamily, ItemQuality, Stat, item};
use engine_game::enemy::{
    EnemyAbility, EnemyAbilityDef, EnemyAim, EnemyEffect, EnemyKind, EnemyRider, EnemyTarget,
    LootEntry, MonsterRole, MonsterTier, TelegraphShape, level_scale,
};
use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiClip, UiColor, UiDrawList, UiIcon, UiRect, UiScale};
use engine_settings::MinimapOrientation;

use super::character_menu::{icon_for_item, quality_color};
use super::input::UiInput;
use super::map_style::{MapStyle, TextStyle, color, motion, radius, space, title_case, type_scale};

/// How far the minimap sees from its centre, in metres.
pub const MINIMAP_RANGE: f32 = 48.0;

/// The most the full-screen map zooms in, as a multiple of "the whole level fits".
pub const MAX_ZOOM: f32 = 8.0;

/// How much one wheel notch zooms by.
pub const ZOOM_STEP: f32 = 1.2;

// ------------------------------------------------------------------------------ the model

/// One kind of monster in an encounter, resolved against the monster table.
#[derive(Debug, Clone, PartialEq)]
pub struct MapMember {
    /// The monster, if the id names one this build has.
    pub kind: Option<EnemyKind>,
    /// The id the level spells it with.
    pub id: String,
    /// How many of it stand in the pack.
    pub count: u32,
    /// The highest level any of them is placed at.
    pub level: u8,
}

impl MapMember {
    /// Its display name: the monster's own, or the id for one this build does not know.
    #[must_use]
    pub fn name(&self) -> String {
        self.kind.map_or_else(
            || title_case(&self.id.replace('_', " ")),
            |kind| title_case(kind.name()),
        )
    }
}

/// One encounter as the map shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct MapEncounterView {
    /// Its pack key: stable for a named pack, which is what selection is kept by.
    pub key: String,
    /// What the marker's tooltip and the sidebar call it.
    pub title: String,
    /// Pack or boss. A pack with a boss-tier monster in it is a boss even if the dungeon
    /// settings forgot to say so.
    pub kind: EncounterKind,
    /// Where its members stand, on average.
    pub position: Vec3,
    /// How spread out they are, in metres.
    pub radius: f32,
    /// Who is in it.
    pub members: Vec<MapMember>,
}

impl MapEncounterView {
    /// How many monsters stand in it.
    #[must_use]
    pub fn monster_count(&self) -> u32 {
        self.members.iter().map(|member| member.count).sum()
    }

    /// The boss-tier member, for a boss encounter.
    #[must_use]
    pub fn boss(&self) -> Option<&MapMember> {
        self.members
            .iter()
            .find(|member| member.kind.is_some_and(EnemyKind::is_boss))
    }

    /// What the tooltip's second line says: "Monster pack · 3 enemies", "Boss encounter".
    #[must_use]
    pub fn kind_label(&self) -> String {
        let class = MarkerClass::of(self);
        match class {
            MarkerClass::Boss => class.label().to_owned(),
            _ => format!(
                "{} \u{00B7} {}",
                class.label(),
                enemies(self.monster_count())
            ),
        }
    }
}

/// The adds a boss calls, as the level configures them.
#[derive(Debug, Clone, PartialEq)]
pub struct BossAdds {
    /// How many it calls.
    pub count: u8,
    /// The monsters they are drawn from, empowered.
    pub pool: Vec<MapMember>,
}

/// Everything static about a level's map.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonMapInfo {
    /// The dungeon's name, for the minimap's banner and the map's header.
    pub name: String,
    /// The rectangle of the world the picture shows.
    pub frame: MapFrame,
    /// Whether the renderer has a picture for it. Without one the map is markers on a plain
    /// panel, which is still a map.
    pub has_picture: bool,
    /// One per pack, in the order the level lists them.
    pub encounters: Vec<MapEncounterView>,
    /// The boss's adds, if the dungeon settings configure any.
    pub boss_adds: Option<BossAdds>,
}

impl DungeonMapInfo {
    /// The map of `scene`, drawn over a picture of `frame`.
    #[must_use]
    pub fn from_scene(scene: &EnvironmentScene, frame: MapFrame, has_picture: bool) -> Self {
        let name = scene
            .dungeon
            .as_ref()
            .map(|dungeon| dungeon.name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| scene.name.replace('_', " "));
        let boss_adds = scene.dungeon.as_ref().and_then(|dungeon| {
            (dungeon.add_count > 0).then(|| BossAdds {
                count: dungeon.add_count,
                pool: dungeon
                    .add_pool
                    .iter()
                    .map(|id| {
                        let kind = EnemyKind::from_id(id);
                        MapMember {
                            kind: kind.map(|kind| kind.empowered().unwrap_or(kind)),
                            id: id.clone(),
                            count: 1,
                            level: 1,
                        }
                    })
                    .collect(),
            })
        });
        let mut info = Self {
            name,
            frame,
            has_picture,
            encounters: Vec::new(),
            boss_adds,
        };
        info.refresh_encounters(scene);
        info
    }

    /// Re-derives the encounters from `scene`'s placements: what the editor calls after a
    /// monster is placed, moved or removed. The frame and the picture are untouched.
    pub fn refresh_encounters(&mut self, scene: &EnvironmentScene) {
        self.encounters = engine_environment::map::encounters(scene)
            .into_iter()
            .map(|encounter| {
                let members: Vec<MapMember> = encounter
                    .members
                    .iter()
                    .map(|member| MapMember {
                        kind: EnemyKind::from_id(&member.monster),
                        id: member.monster.clone(),
                        count: member.count,
                        level: member.level,
                    })
                    .collect();
                let has_boss = members
                    .iter()
                    .any(|member| member.kind.is_some_and(EnemyKind::is_boss));
                let kind = if has_boss {
                    EncounterKind::Boss
                } else {
                    encounter.kind
                };
                let title = match (&encounter.name, kind) {
                    (Some(name), _) => title_case(name),
                    (None, EncounterKind::Boss) => members
                        .iter()
                        .find(|member| member.kind.is_some_and(EnemyKind::is_boss))
                        .map_or_else(|| encounter.key.clone(), MapMember::name),
                    (None, EncounterKind::Pack) if !encounter.key.starts_with('#') => {
                        title_case(&encounter.key)
                    }
                    (None, EncounterKind::Pack) => members
                        .first()
                        .map_or_else(|| encounter.key.clone(), MapMember::name),
                };
                MapEncounterView {
                    key: encounter.key,
                    title,
                    kind,
                    position: encounter.position,
                    radius: encounter.radius,
                    members,
                }
            })
            .collect();
    }

    /// How many packs, monsters and bosses the level has.
    #[must_use]
    pub fn totals(&self) -> (usize, u32, usize) {
        let bosses = self
            .encounters
            .iter()
            .filter(|encounter| encounter.kind == EncounterKind::Boss)
            .count();
        let monsters = self
            .encounters
            .iter()
            .map(MapEncounterView::monster_count)
            .sum();
        (self.encounters.len() - bosses, monsters, bosses)
    }
}

/// A character on the map: where they stand and which way they face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapUnit {
    /// Where their feet are.
    pub position: Vec3,
    /// Which way they face, radians about +Y (zero faces −Z, north). `None` draws a dot.
    pub facing: Option<f32>,
}

/// The dynamic half of the map, refreshed every frame: the only things on it that move.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapMarkers {
    /// The local player.
    pub player: Option<MapUnit>,
    /// Which way the camera looks, radians about +Y: "up" on a rotating minimap.
    pub view_yaw: f32,
    /// Everyone else in the party.
    pub party: Vec<MapUnit>,
    /// Each encounter's state in the run, in [`DungeonMapInfo::encounters`] order. Empty where
    /// the level has no run.
    pub statuses: Vec<EncounterStatus>,
    /// The dungeon's completion, if the level is a dungeon.
    pub progress: Option<DungeonProgress>,
}

// ----------------------------------------------------------------------------- projection

/// A turn on screen, clockwise by `angle` (screen y grows downward).
fn turn(angle: f32, x: f32, y: f32) -> (f32, f32) {
    let (sin, cos) = angle.sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

/// The minimap's projection: the player at the centre, `pixels_per_metre`, and — rotating —
/// the view direction turned to the top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapProjection {
    /// The circle's centre on screen.
    pub center: (f32, f32),
    /// Its radius, in pixels.
    pub radius: f32,
    /// The world point at the centre (`x`, `z`).
    pub focus: (f32, f32),
    /// Pixels per metre.
    pub pixels_per_metre: f32,
    /// How far the map is turned clockwise on screen: the view yaw when rotating, zero when
    /// north is up.
    pub rotation: f32,
}

impl MinimapProjection {
    /// Where a world point lands on screen.
    #[must_use]
    pub fn to_screen(&self, position: Vec3) -> (f32, f32) {
        let (dx, dz) = (position.x - self.focus.0, position.z - self.focus.1);
        let (x, y) = turn(self.rotation, dx, dz);
        (
            self.center.0 + x * self.pixels_per_metre,
            self.center.1 + y * self.pixels_per_metre,
        )
    }

    /// The world `(x, z)` under a screen point: the inverse of [`MinimapProjection::to_screen`].
    #[must_use]
    pub fn to_world(&self, screen: (f32, f32)) -> (f32, f32) {
        let (x, y) = (
            (screen.0 - self.center.0) / self.pixels_per_metre,
            (screen.1 - self.center.1) / self.pixels_per_metre,
        );
        let (dx, dz) = turn(-self.rotation, x, y);
        (self.focus.0 + dx, self.focus.1 + dz)
    }

    /// How a unit facing `yaw` is drawn: the arrow's clockwise turn from pointing up.
    #[must_use]
    pub fn arrow_rotation(&self, yaw: f32) -> f32 {
        self.rotation - yaw
    }

    /// The picture's affine mapping over `rect` (the circle's bounding square): the
    /// texture coordinate at its top-left corner, and per pixel right and down.
    #[must_use]
    pub fn picture_mapping(
        &self,
        frame: &MapFrame,
        rect: UiRect,
    ) -> ((f32, f32), (f32, f32), (f32, f32)) {
        let (wx, wz) = self.to_world((rect.x, rect.y));
        let origin = frame.world_to_uv(Vec3::new(wx, 0.0, wz));
        let (rx, rz) = turn(-self.rotation, 1.0, 0.0);
        let (dx, dz) = turn(-self.rotation, 0.0, 1.0);
        let per = 1.0 / self.pixels_per_metre;
        (
            origin,
            (rx * per / frame.width(), rz * per / frame.depth()),
            (dx * per / frame.width(), dz * per / frame.depth()),
        )
    }
}

/// The full-screen canvas's projection: north up, panned to `center` and zoomed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasProjection {
    /// The canvas on screen.
    pub canvas: UiRect,
    /// The world point at the canvas's centre (`x`, `z`).
    pub center: (f32, f32),
    /// Pixels per metre.
    pub pixels_per_metre: f32,
}

impl CanvasProjection {
    /// Where a world point lands on screen.
    #[must_use]
    pub fn to_screen(&self, position: Vec3) -> (f32, f32) {
        (
            self.canvas.x
                + self.canvas.width * 0.5
                + (position.x - self.center.0) * self.pixels_per_metre,
            self.canvas.y
                + self.canvas.height * 0.5
                + (position.z - self.center.1) * self.pixels_per_metre,
        )
    }

    /// The world `(x, z)` under a screen point.
    #[must_use]
    pub fn to_world(&self, screen: (f32, f32)) -> (f32, f32) {
        (
            self.center.0
                + (screen.0 - self.canvas.x - self.canvas.width * 0.5) / self.pixels_per_metre,
            self.center.1
                + (screen.1 - self.canvas.y - self.canvas.height * 0.5) / self.pixels_per_metre,
        )
    }

    /// The picture's affine mapping over the canvas.
    #[must_use]
    pub fn picture_mapping(&self, frame: &MapFrame) -> ((f32, f32), (f32, f32), (f32, f32)) {
        let (wx, wz) = self.to_world((self.canvas.x, self.canvas.y));
        let origin = frame.world_to_uv(Vec3::new(wx, 0.0, wz));
        let per = 1.0 / self.pixels_per_metre;
        (
            origin,
            (per / frame.width(), 0.0),
            (0.0, per / frame.depth()),
        )
    }
}

// ------------------------------------------------------------------------- game-side state

/// Where one encounter stands in the run, as the simulation says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncounterStatus {
    /// The simulation has no encounter by this pack's name (a level with no dungeon run).
    #[default]
    Unknown,
    /// Nobody has pulled it.
    Waiting,
    /// It is fighting.
    Engaged,
    /// Every member is dead.
    Cleared,
}

/// The dungeon's progress, as the run says.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DungeonProgress {
    /// Completion, `0..=100`, rounded down.
    pub percent: u32,
    /// Completion as a fraction of the requirement.
    pub fraction: f32,
    /// Whether the boss's seal is broken.
    pub unlocked: bool,
    /// Whether the boss and its adds are dead.
    pub completed: bool,
}

/// How a marker is drawn: ordinary, elite, or boss — from the monsters in the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerClass {
    /// Ordinary monsters.
    Pack,
    /// A pack holding an elite or an empowered monster.
    Elite,
    /// The boss.
    Boss,
}

impl MarkerClass {
    /// The class of `encounter`.
    #[must_use]
    pub fn of(encounter: &MapEncounterView) -> Self {
        if encounter.kind == EncounterKind::Boss {
            return Self::Boss;
        }
        let elite = encounter.members.iter().any(|member| {
            member.kind.is_some_and(|kind| {
                let archetype = kind.archetype();
                archetype.role == MonsterRole::Elite || archetype.tier == MonsterTier::Empowered
            })
        });
        if elite { Self::Elite } else { Self::Pack }
    }

    /// Its insignia.
    #[must_use]
    pub fn icon(self) -> UiIcon {
        match self {
            Self::Pack => UiIcon::MapPack,
            Self::Elite => UiIcon::CrestElite,
            Self::Boss => UiIcon::MapBoss,
        }
    }

    /// Its size on the full-screen map, logical pixels.
    fn size(self) -> f32 {
        match self {
            Self::Pack => 30.0,
            Self::Elite => 34.0,
            Self::Boss => 46.0,
        }
    }

    /// What the tooltip and journal call the kind.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pack => "Monster pack",
            Self::Elite => "Elite pack",
            Self::Boss => "Boss encounter",
        }
    }
}

/// The insignia a marker of `kind` draws with. Kept for callers that know only the kind.
#[must_use]
pub fn marker_icon(kind: EncounterKind) -> UiIcon {
    match kind {
        EncounterKind::Pack => UiIcon::MapPack,
        EncounterKind::Boss => UiIcon::MapBoss,
    }
}

// --------------------------------------------------------------------------------- layout

/// Where the minimap sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinimapLayout {
    /// The dungeon's name plate, above the circle.
    pub banner: UiRect,
    /// The circle's centre.
    pub center: (f32, f32),
    /// The map's radius inside the frame.
    pub radius: f32,
    /// The frame's width outside `radius`.
    pub ring: f32,
}

impl MinimapLayout {
    /// The whole footprint: plate, frame and all. What the HUD keeps clear.
    #[must_use]
    pub fn bounds(&self) -> UiRect {
        let outer = self.radius + self.ring;
        let top = self.banner.y;
        UiRect::new(
            self.center.0 - outer,
            top,
            outer * 2.0,
            self.center.1 + outer - top,
        )
    }

    /// Whether a point is on the minimap.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        dx * dx + dy * dy <= (self.radius + self.ring) * (self.radius + self.ring)
    }
}

/// Where the full-screen map's parts sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapScreenLayout {
    /// The whole window.
    pub panel: UiRect,
    /// The title bar.
    pub header: UiRect,
    /// The close button.
    pub close: UiRect,
    /// The dungeon's completion, in the title bar.
    pub progress: UiRect,
    /// The map itself.
    pub canvas: UiRect,
    /// The encounter journal.
    pub sidebar: UiRect,
    /// The journal's fixed header: the selection's title and roster.
    pub sidebar_head: UiRect,
    /// The journal's scrolling body.
    pub sidebar_body: UiRect,
    /// Zoom in.
    pub zoom_in: UiRect,
    /// Zoom out.
    pub zoom_out: UiRect,
    /// Fit the whole level.
    pub fit: UiRect,
    /// Centre on the player.
    pub recenter: UiRect,
    /// The controls' shared plate.
    pub controls: UiRect,
    /// The mouse-and-keyboard hint strip.
    pub help: UiRect,
}

/// One of the map's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapControl {
    /// Close the map.
    Close,
    /// Zoom in a step.
    ZoomIn,
    /// Zoom out a step.
    ZoomOut,
    /// Show the whole level.
    Fit,
    /// Centre on the player.
    Recenter,
}

impl MapControl {
    /// What its tooltip says.
    #[must_use]
    pub fn tooltip(self) -> (&'static str, &'static str) {
        match self {
            Self::Close => ("Close map", "M or Esc"),
            Self::ZoomIn => ("Zoom in", "Wheel up or Up arrow"),
            Self::ZoomOut => ("Zoom out", "Wheel down or Down arrow"),
            Self::Fit => ("Show the whole dungeon", "Home"),
            Self::Recenter => ("Centre on your character", "Full stop"),
        }
    }
}

/// The keys the map answers while it is open, read by the application from its bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MapKeys {
    /// Zoom in a step.
    pub zoom_in: bool,
    /// Zoom out a step.
    pub zoom_out: bool,
    /// Show the whole level.
    pub fit: bool,
    /// Centre on the player.
    pub recenter: bool,
    /// Select the next encounter.
    pub next: bool,
    /// Select the previous encounter.
    pub previous: bool,
}

/// Kept for the old name: the map's colours now live in [`super::map_style`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MapTheme;

// ------------------------------------------------------------------------- ability reading

/// Seconds, as the journal prints them: `1.5 s`, `12 s`.
fn seconds(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{} s", value.round() as i32)
    } else {
        format!("{value:.1} s")
    }
}

/// Metres, as the journal prints them.
fn metres(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{} m", value.round() as i32)
    } else {
        format!("{value:.1} m")
    }
}

/// A number with thousands separated: `16,000`.
fn thousands(value: f32) -> String {
    let whole = value.round().max(0.0) as u64;
    let digits = whole.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A damage school in words.
fn school(damage: DamageType) -> &'static str {
    match damage {
        DamageType::Physical => "physical",
        DamageType::Fire => "fire",
        DamageType::Arcane => "arcane",
        DamageType::Nature => "nature",
    }
}

/// The icon an ability draws with: its school, or a heal.
#[must_use]
pub fn ability_icon(def: &EnemyAbilityDef) -> UiIcon {
    if def.effect.is_healing() {
        return UiIcon::AbilityHeal;
    }
    match def.damage_type {
        DamageType::Physical => UiIcon::AbilityPhysical,
        DamageType::Fire => UiIcon::AbilityFire,
        DamageType::Arcane => UiIcon::AbilityArcane,
        DamageType::Nature => UiIcon::AbilityNature,
    }
}

/// The accent an ability's icon frame is drawn in: its school.
fn ability_accent(def: &EnemyAbilityDef) -> UiColor {
    if def.effect.is_healing() {
        return color::VERDANT;
    }
    match def.damage_type {
        DamageType::Physical => color::EMBER,
        DamageType::Fire => color::EMBER,
        DamageType::Arcane => color::ARCANE,
        DamageType::Nature => color::VERDANT,
    }
}

/// What an ability is, for its tags and the journal's filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbilityTag {
    /// A cast a player interrupt can stop.
    Interruptible,
    /// It hits an area rather than one target.
    Area,
    /// It slows or charges its target.
    Control,
}

impl AbilityTag {
    /// Its icon.
    #[must_use]
    pub fn icon(self) -> UiIcon {
        match self {
            Self::Interruptible => UiIcon::TagInterrupt,
            Self::Area => UiIcon::TagArea,
            Self::Control => UiIcon::TagControl,
        }
    }

    /// Its colour.
    fn color(self) -> UiColor {
        match self {
            Self::Interruptible => color::AMBER,
            Self::Area => color::EMBER,
            Self::Control => color::FROST,
        }
    }
}

/// The tags an ability's row carries, read off its row in the table.
#[must_use]
pub fn ability_tags(def: &EnemyAbilityDef) -> Vec<AbilityTag> {
    let mut tags = Vec::new();
    if def.interruptible {
        tags.push(AbilityTag::Interruptible);
    }
    if def.shape.is_area() {
        tags.push(AbilityTag::Area);
    }
    let control = matches!(
        def.rider,
        EnemyRider::Slow { .. } | EnemyRider::Afflict { .. }
    ) || matches!(def.effect, EnemyEffect::Charge { slow, .. } if slow > 0.0);
    if control {
        tags.push(AbilityTag::Control);
    }
    tags
}

/// The journal's ability filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AbilityFilter {
    /// Everything.
    #[default]
    All,
    /// Abilities that deal damage.
    Damage,
    /// Abilities that slow or charge.
    Control,
    /// Casts that can be interrupted.
    Interrupts,
}

impl AbilityFilter {
    /// Every filter, in tab order.
    pub const ALL: [Self; 4] = [Self::All, Self::Damage, Self::Control, Self::Interrupts];

    /// Its tab's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Damage => "Damage",
            Self::Control => "Control",
            Self::Interrupts => "Interrupts",
        }
    }

    /// Whether `def` passes it.
    #[must_use]
    pub fn accepts(self, def: &EnemyAbilityDef) -> bool {
        match self {
            Self::All => true,
            Self::Damage => def.damage_per_use() > 0.0,
            Self::Control => ability_tags(def).contains(&AbilityTag::Control),
            Self::Interrupts => def.interruptible,
        }
    }
}

/// The line of numbers under an ability's name — cast, cooldown, range — read off its row.
/// Only what the row has is printed.
#[must_use]
pub fn ability_timing(def: &EnemyAbilityDef, cooldown_scale: f32) -> String {
    let mut parts: Vec<String> = Vec::new();
    match def.effect {
        EnemyEffect::Channel { .. } => parts.push(format!("{} channel", seconds(def.cast_time))),
        _ if def.is_cast() => parts.push(format!("{} cast", seconds(def.cast_time))),
        _ => parts.push("Instant".to_owned()),
    }
    if def.cooldown > 0.0 {
        parts.push(format!(
            "{} cooldown",
            seconds(def.cooldown * cooldown_scale)
        ));
    }
    if def.range > 0.0 {
        parts.push(format!("{} range", metres(def.range)));
    }
    parts.join(" \u{00B7} ")
}

/// What an ability does, in the game's own numbers, as sentences: damage and school, area and
/// aim, targets, zones, delay, hazards, charges, frenzy and riders. Every clause is read off the
/// row; none is written for the map.
#[must_use]
pub fn ability_effects(def: &EnemyAbilityDef, damage_scale: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let damage = def.damage * damage_scale;
    match def.effect {
        EnemyEffect::Heal { amount } => {
            lines.push(format!(
                "Heals its most wounded ally for {}",
                thousands(amount * damage_scale)
            ));
        }
        EnemyEffect::Channel { interval, ramp } => {
            let ticks = engine_game::enemy::channel_ticks(def.cast_time, interval);
            let total = def.damage_per_use() * damage_scale;
            if ramp > 0.0 {
                lines.push(format!(
                    "{ticks} growing ticks, {} {} damage in all",
                    thousands(total),
                    school(def.damage_type)
                ));
            } else {
                lines.push(format!(
                    "{ticks} ticks of {} {} damage",
                    thousands(damage),
                    school(def.damage_type)
                ));
            }
        }
        EnemyEffect::Frenzy { .. } => {}
        _ if damage > 0.0 => {
            lines.push(format!(
                "{} {} damage",
                thousands(damage),
                school(def.damage_type)
            ));
        }
        _ => {}
    }

    let whom = match def.select {
        EnemyTarget::Victim if def.effect.is_self() => "itself",
        EnemyTarget::Victim => "its target",
        EnemyTarget::WoundedAlly => "its most wounded ally",
        EnemyTarget::Farthest => "the farthest player",
        EnemyTarget::Random => "a random player",
    };
    match def.shape {
        TelegraphShape::Single => lines.push(if def.effect.is_self() {
            "Used on itself".to_owned()
        } else {
            format!("Hits {whom}")
        }),
        TelegraphShape::Circle { radius } => lines.push(match def.aim {
            EnemyAim::Caster => format!("{} circle around itself", metres(radius)),
            EnemyAim::Target => format!("{} circle on {whom}", metres(radius)),
        }),
        TelegraphShape::Cone {
            range, half_angle, ..
        } => lines.push(format!(
            "{} cone, {}\u{00B0} wide, in front of itself",
            metres(range),
            (half_angle.to_degrees() * 2.0).round() as i32
        )),
        TelegraphShape::Line { length, width, .. } => lines.push(format!(
            "{} \u{00D7} {} line toward {whom}",
            metres(length),
            metres(width)
        )),
    }
    if def.targets > 1 {
        lines.push(format!("Up to {} targets at once", def.targets));
    }
    if def.zones > 1 {
        lines.push(format!("{} impact zones per target", def.zones));
    }
    if def.delay > 0.0 {
        lines.push(format!(
            "Lands {} after the cast{}",
            seconds(def.delay),
            if def.follow {
                ", following its target"
            } else {
                ""
            }
        ));
    }
    if def.chain {
        lines.push("Each detonation marks everyone it catches".to_owned());
    }
    match def.effect {
        EnemyEffect::Hazard {
            duration,
            damage_per_second,
            grow_to,
            ..
        } => lines.push(format!(
            "Leaves ground dealing {} {} damage a second for {}{}",
            thousands(damage_per_second * damage_scale),
            school(def.damage_type),
            seconds(duration),
            if grow_to > 0.0 {
                format!(", spreading to {}", metres(grow_to))
            } else {
                String::new()
            }
        )),
        EnemyEffect::Charge {
            slow,
            slow_duration,
            impact_radius,
            shockwave_radius,
            trail_damage_per_second,
            trail_duration,
            ..
        } => {
            lines.push(format!(
                "Charges its target, slowing them {}% for {}",
                (slow * 100.0).round() as i32,
                seconds(slow_duration)
            ));
            if impact_radius > 0.0 {
                lines.push(format!("Lands in a {} circle", metres(impact_radius)));
            }
            if shockwave_radius > 0.0 {
                lines.push(format!(
                    "Then sends out a {} shockwave",
                    metres(shockwave_radius)
                ));
            }
            if trail_damage_per_second > 0.0 {
                lines.push(format!(
                    "Leaves a trail dealing {} a second for {}",
                    thousands(trail_damage_per_second * damage_scale),
                    seconds(trail_duration)
                ));
            }
        }
        EnemyEffect::Frenzy { duration, haste } => lines.push(format!(
            "Attacks {}% faster for {}",
            (haste * 100.0).round() as i32,
            seconds(duration)
        )),
        EnemyEffect::Hymn {
            heal_fraction,
            haste,
            duration,
        } => {
            lines.push(format!(
                "If it finishes, heals itself for {}% of its health",
                (heal_fraction * 100.0).round() as i32
            ));
            lines.push(format!(
                "And every ally attacks {}% faster for {}",
                (haste * 100.0).round() as i32,
                seconds(duration)
            ));
        }
        EnemyEffect::Undertow {
            pull_speed,
            pool_radius,
            pool_damage_per_second,
            pool_duration,
            ..
        } => {
            lines.push(format!(
                "Drags every player towards itself at {} a second",
                metres(pull_speed)
            ));
            lines.push(format!(
                "Opens {} pools dealing {} a second for {}",
                metres(pool_radius),
                thousands(pool_damage_per_second * damage_scale),
                seconds(pool_duration)
            ));
        }
        _ => {}
    }
    match def.rider {
        EnemyRider::Bleed {
            damage_per_stack,
            duration,
            stacks,
            max_stacks,
        } => lines.push(format!(
            "Bleed: {} damage a second per stack, {stacks} stack{} a hit, up to {max_stacks}, for {}",
            thousands(damage_per_stack * damage_scale),
            if stacks == 1 { "" } else { "s" },
            seconds(duration)
        )),
        EnemyRider::Slow { fraction, duration } => lines.push(format!(
            "Slows its victim {}% for {}",
            (fraction * 100.0).round() as i32,
            seconds(duration)
        )),
        EnemyRider::Afflict {
            kind,
            duration,
            max_stacks,
        } => lines.push(format!(
            "Leaves {} on its victim for {}, stacking up to {max_stacks}{}",
            kind.display_name(),
            seconds(duration),
            if kind.is_dispellable() {
                " (dispellable)"
            } else {
                ""
            }
        )),
        EnemyRider::None => {}
    }
    lines
}

/// Every ability a monster uses in any phase, each once, in first-use order, with the phase
/// it first appears in.
#[must_use]
pub fn abilities_of(kind: EnemyKind) -> Vec<(EnemyAbility, usize)> {
    let archetype = kind.archetype();
    let mut out: Vec<(EnemyAbility, usize)> = Vec::new();
    for phase in 0..archetype.phase_count() {
        for ability in archetype.phase_abilities(phase) {
            if !out.iter().any(|(held, _)| *held == ability) {
                out.push((ability, phase));
            }
        }
    }
    out
}

/// Which heading a loot item is listed under.
#[must_use]
pub fn loot_category(family: ItemFamily) -> &'static str {
    match family {
        ItemFamily::Necklace | ItemFamily::Ring => "Jewelry",
        ItemFamily::Trinket => "Trinkets",
        _ => "Armor",
    }
}

/// One loot row, resolved through the item table.
#[derive(Debug, Clone, PartialEq)]
pub struct LootLine {
    /// The item's picture.
    pub icon: UiIcon,
    /// Its name, title case.
    pub name: String,
    /// Its quality.
    pub quality: ItemQuality,
    /// "Uncommon ring · Item level 10".
    pub meta: String,
    /// "+10 Stamina", or empty.
    pub stats: String,
    /// "25% drop chance".
    pub chance: String,
    /// Which heading it is listed under.
    pub category: &'static str,
}

/// A loot row for `entry`, or `None` for an item this build does not have — never invented.
#[must_use]
pub fn loot_detail(entry: &LootEntry) -> Option<LootLine> {
    let def = item(entry.item)?;
    let stats: Vec<String> = Stat::ALL
        .into_iter()
        .filter_map(|stat| {
            let value = def.stats.get(stat);
            (value != 0.0).then(|| format!("+{} {}", value.round() as i32, title_case(stat.name())))
        })
        .collect();
    Some(LootLine {
        icon: icon_for_item(def.icon),
        name: title_case(def.name),
        quality: def.quality,
        meta: format!(
            "{} {} \u{00B7} Item level {}",
            title_case(def.quality.name()),
            def.family.name().to_lowercase(),
            def.item_level
        ),
        stats: stats.join(", "),
        chance: format!("{}% drop chance", (entry.chance * 100.0).round() as i32),
        category: loot_category(def.family),
    })
}

// ------------------------------------------------------------------------------ the journal

/// Something a journal row does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Open or fold section `n`.
    Toggle(usize),
    /// Select encounter `n`.
    Select(usize),
    /// Show only abilities that pass a filter.
    Filter(AbilityFilter),
}

/// One laid-out row of the journal, computed when the selection, a fold or the width changes,
/// and drawn every frame from the cache.
#[derive(Debug, Clone, PartialEq)]
struct Block {
    kind: BlockKind,
    /// Top, relative to the body's content origin before scrolling.
    y: f32,
    height: f32,
    action: Option<Action>,
}

/// A roster chip: a portrait and "2 × Skeleton Warrior".
#[derive(Debug, Clone, PartialEq)]
struct Chip {
    tint: UiColor,
    text: String,
    x: f32,
    y: f32,
    width: f32,
}

/// What a journal row is.
#[derive(Debug, Clone, PartialEq)]
enum BlockKind {
    /// A small upper-case section label, with an optional accent rule.
    Label { text: String, accent: UiColor },
    /// Wrapped lines of one style.
    Lines {
        lines: Vec<String>,
        style: TextStyle,
        color: UiColor,
        indent: f32,
    },
    /// A monster card's header.
    Monster {
        tint: UiColor,
        name: String,
        meta: String,
        count: u32,
        open: bool,
    },
    /// An ability's row.
    Ability {
        icon: UiIcon,
        accent: UiColor,
        name: String,
        meta: String,
        summary: String,
        tags: Vec<AbilityTag>,
        open: bool,
    },
    /// An effect list.
    Bullets {
        lines: Vec<(String, bool)>,
        dot: UiColor,
    },
    /// The counterplay callout.
    Counterplay { lines: Vec<String> },
    /// The filter tabs.
    Filters {
        tabs: Vec<(AbilityFilter, usize, f32, f32)>,
        active: AbilityFilter,
    },
    /// A clickable encounter in the overview.
    Encounter {
        class: MarkerClass,
        name: String,
        detail: String,
        status: EncounterStatus,
    },
    /// A legend entry.
    Legend {
        icon: UiIcon,
        tint: UiColor,
        label: String,
    },
    /// A deliberate empty state.
    Empty { icon: UiIcon, lines: Vec<String> },
    /// A loot row.
    Loot { line: LootLine },
    /// The overview's progress card.
    Progress,
    /// Chips laid out in rows.
    Chips { chips: Vec<Chip> },
    /// A guest panel's heading.
    Guests { title: String },
    /// Space.
    Gap,
}

/// The journal's fixed header.
#[derive(Debug, Clone, PartialEq, Default)]
struct Head {
    icon: Option<UiIcon>,
    title: Vec<String>,
    subtitle: String,
    badge: Option<(String, UiColor)>,
    chips: Vec<Chip>,
    height: f32,
}

/// A background span behind a run of blocks: a card.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Card {
    top: f32,
    bottom: f32,
    tone: CardTone,
}

/// How a card is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardTone {
    /// An ordinary raised card.
    Raised,
    /// A quiet inset: empty states, guests.
    Inset,
}

// ------------------------------------------------------------------------------ the state

/// What the application should do about what the map just did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapOutcome {
    /// Nothing the application has to answer.
    Idle,
    /// The full-screen map opened: release the camera, show the cursor.
    Opened,
    /// It closed: give the camera back.
    Closed,
    /// The pointer did something on the map; nothing behind it should see this frame's click.
    Consumed,
}

/// A drag on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    start: (f32, f32),
    center: (f32, f32),
    moved: bool,
}

/// What the pointer is resting on, for the tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hover {
    Marker(usize),
    Minimap(usize),
    Control(MapControl),
}

/// The minimap and the full-screen map: their state, their input, their animation and their
/// drawing.
#[derive(Debug, Clone)]
pub struct DungeonMap {
    info: Option<DungeonMapInfo>,
    open: bool,
    scale: UiScale,
    style: MapStyle,
    orientation: MinimapOrientation,
    reduced_motion: bool,
    /// The canvas's centre, world `(x, z)`.
    center: (f32, f32),
    /// Multiple of "the whole level fits", as drawn.
    zoom: f32,
    /// Where the zoom is heading, and the world point held still on the way there.
    zoom_target: f32,
    zoom_anchor: Option<((f32, f32), (f32, f32))>,
    drag: Option<Drag>,
    selected: Option<usize>,
    hovered: Option<Hover>,
    hover_time: f32,
    /// Each marker's hover emphasis, `0..1`, eased.
    emphasis: Vec<f32>,
    /// The journal body's fade-in after a selection change, `0..1`.
    reveal: f32,
    /// The whole window's fade-in after opening, `0..1`.
    appear: f32,
    filter: AbilityFilter,
    expanded: Vec<bool>,
    scroll: f32,
    head: Head,
    blocks: Vec<Block>,
    cards: Vec<Card>,
    stale: bool,
    laid_out_width: f32,
    pointer: (f32, f32),
    /// The player's last position, for Recenter.
    player: Option<(f32, f32)>,
    /// The last progress seen, for the overview's card.
    progress: Option<DungeonProgress>,
    statuses: Vec<EncounterStatus>,
    /// The minimap banner's title, title-cased and fitted, and the `(width, size)` it was
    /// fitted for. Rebuilt by [`DungeonMap::advance`] only when one of those changes: building
    /// it in `push_minimap` was two `String`s every frame of every level with a map.
    minimap_title: String,
    minimap_title_key: Option<(u32, u32)>,
}

impl Default for DungeonMap {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

impl DungeonMap {
    /// A map with no level yet, drawn at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            info: None,
            open: false,
            scale,
            style: MapStyle::new(scale),
            orientation: MinimapOrientation::default(),
            reduced_motion: false,
            center: (0.0, 0.0),
            zoom: 1.0,
            zoom_target: 1.0,
            zoom_anchor: None,
            drag: None,
            selected: None,
            hovered: None,
            hover_time: 0.0,
            emphasis: Vec::new(),
            reveal: 1.0,
            appear: 1.0,
            filter: AbilityFilter::All,
            expanded: Vec::new(),
            scroll: 0.0,
            head: Head::default(),
            blocks: Vec::new(),
            cards: Vec::new(),
            stale: true,
            laid_out_width: 0.0,
            pointer: (0.0, 0.0),
            player: None,
            progress: None,
            statuses: Vec::new(),
            minimap_title: String::new(),
            minimap_title_key: None,
        }
    }

    /// The banner title's cache key: the width it is fitted to and the size it is set at.
    fn minimap_title_key(&self, viewport: (u32, u32)) -> (f32, (u32, u32)) {
        let s = self.style;
        let width = self.minimap_layout(viewport).banner.width - s.px(24.0);
        (width, (width.to_bits(), s.size(type_scale::UI).to_bits()))
    }

    /// Rebuilds the cached banner title if the level, the viewport or the scale changed.
    fn refresh_minimap_title(&mut self, viewport: (u32, u32)) {
        let Some(info) = &self.info else {
            return;
        };
        let (width, key) = self.minimap_title_key(viewport);
        if self.minimap_title_key == Some(key) {
            return;
        }
        self.minimap_title = self
            .style
            .fit(type_scale::UI, &title_case(&info.name), width);
        self.minimap_title_key = Some(key);
    }

    /// Replaces the level. Closes the map and forgets the selection.
    pub fn set_info(&mut self, info: Option<DungeonMapInfo>) {
        self.minimap_title_key = None;
        self.emphasis = vec![0.0; info.as_ref().map_or(0, |info| info.encounters.len())];
        self.info = info;
        self.open = false;
        self.selected = None;
        self.hovered = None;
        self.drag = None;
        self.filter = AbilityFilter::All;
        self.expanded.clear();
        self.fit();
        self.stale = true;
    }

    /// Re-derives the encounters after the level's monsters changed, keeping the selection on
    /// the same pack if it still exists.
    pub fn refresh_encounters(&mut self, scene: &EnvironmentScene) {
        let selected_key = self
            .selected_encounter()
            .map(|encounter| encounter.key.clone());
        if let Some(info) = &mut self.info {
            info.refresh_encounters(scene);
            self.selected = selected_key.and_then(|key| {
                info.encounters
                    .iter()
                    .position(|encounter| encounter.key == key)
            });
            self.emphasis = vec![0.0; info.encounters.len()];
        }
        self.hovered = None;
        self.stale = true;
    }

    /// The level, if one is set.
    #[must_use]
    pub fn info(&self) -> Option<&DungeonMapInfo> {
        self.info.as_ref()
    }

    /// Sets the interface scale.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.scale = scale;
        self.style = MapStyle::new(scale);
        self.stale = true;
    }

    /// Sets how the minimap turns. Takes effect on the next frame drawn.
    pub fn set_orientation(&mut self, orientation: MinimapOrientation) {
        self.orientation = orientation;
    }

    /// How the minimap turns.
    #[must_use]
    pub fn orientation(&self) -> MinimapOrientation {
        self.orientation
    }

    /// Turns the map's transitions off (or on): every animated value jumps to where it is going.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    /// Whether the full-screen map is up.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the full-screen map on the whole level. Does nothing without a level.
    pub fn open(&mut self) -> bool {
        if self.info.is_none() || self.open {
            return false;
        }
        self.open = true;
        self.drag = None;
        self.appear = if self.reduced_motion { 1.0 } else { 0.0 };
        self.fit();
        true
    }

    /// Closes the full-screen map.
    pub fn close(&mut self) -> bool {
        let was = self.open;
        self.open = false;
        self.drag = None;
        self.hovered = None;
        was
    }

    /// Opens it if closed, closes it if open. Returns what happened.
    pub fn toggle(&mut self) -> MapOutcome {
        if self.open {
            self.close();
            MapOutcome::Closed
        } else if self.open() {
            MapOutcome::Opened
        } else {
            MapOutcome::Idle
        }
    }

    /// The selected encounter's index.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The selected encounter.
    #[must_use]
    pub fn selected_encounter(&self) -> Option<&MapEncounterView> {
        let index = self.selected?;
        self.info.as_ref()?.encounters.get(index)
    }

    /// Selects encounter `index`, or clears the selection.
    pub fn select(&mut self, index: Option<usize>) {
        let count = self.info.as_ref().map_or(0, |info| info.encounters.len());
        let index = index.filter(|index| *index < count);
        if index != self.selected {
            self.selected = index;
            self.scroll = 0.0;
            self.expanded.clear();
            self.filter = AbilityFilter::All;
            self.reveal = if self.reduced_motion { 1.0 } else { 0.0 };
            self.stale = true;
        }
    }

    /// The canvas's zoom, as a multiple of "the whole level fits".
    #[must_use]
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The canvas's centre, world `(x, z)`.
    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        self.center
    }

    /// Back to the whole level, centred.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.zoom_target = 1.0;
        self.zoom_anchor = None;
        if let Some(info) = &self.info {
            self.center = info.frame.center();
        }
    }

    /// Takes in this frame's game state: where the player and the party are, how far the run
    /// is, and which packs are cleared. Cheap; called every frame the map could be drawn.
    pub fn observe(&mut self, markers: &MapMarkers) {
        self.player = markers
            .player
            .map(|player| (player.position.x, player.position.z));
        if self.progress != markers.progress || self.statuses != markers.statuses {
            self.progress = markers.progress;
            self.statuses.clone_from(&markers.statuses);
            self.stale = true;
        }
    }

    /// The status of encounter `index`.
    fn status(&self, index: usize) -> EncounterStatus {
        self.statuses.get(index).copied().unwrap_or_default()
    }

    /// Advances the map's transitions by `dt` seconds. Nothing moves once everything has
    /// arrived, so an idle map costs nothing here.
    pub fn advance(&mut self, dt: f32, viewport: (u32, u32)) {
        self.refresh_minimap_title(viewport);
        let ease = |value: f32, target: f32, duration: f32| -> f32 {
            if self.reduced_motion || duration <= 0.0 {
                return target;
            }
            let step = 1.0 - (-dt.max(0.0) * 4.0 / duration).exp();
            let next = value + (target - value) * step;
            if (next - target).abs() < 1e-3 {
                target
            } else {
                next
            }
        };
        self.reveal = ease(self.reveal, 1.0, motion::BASE);
        self.appear = ease(self.appear, 1.0, motion::BASE);
        let hovered_marker = match self.hovered {
            Some(Hover::Marker(index) | Hover::Minimap(index)) => Some(index),
            _ => None,
        };
        for index in 0..self.emphasis.len() {
            let target = if hovered_marker == Some(index) {
                1.0
            } else {
                0.0
            };
            self.emphasis[index] = ease(self.emphasis[index], target, motion::FAST);
        }
        if self.hovered.is_some() {
            self.hover_time += dt.max(0.0);
        }
        // Zoom glides to its target, holding the anchored world point under the cursor.
        if (self.zoom - self.zoom_target).abs() > 1e-4 {
            self.zoom = ease(self.zoom, self.zoom_target, motion::BASE * 0.8);
            if let (Some((world, screen)), Some(projection)) =
                (self.zoom_anchor, self.canvas_projection(viewport))
            {
                let canvas = projection.canvas;
                let ppm = projection.pixels_per_metre;
                self.center = (
                    world.0 - (screen.0 - canvas.x - canvas.width * 0.5) / ppm,
                    world.1 - (screen.1 - canvas.y - canvas.height * 0.5) / ppm,
                );
            }
            self.clamp_center();
        }
    }

    // ------------------------------------------------------------------ layout & projection

    /// Where the minimap sits in a viewport.
    #[must_use]
    pub fn minimap_layout(&self, viewport: (u32, u32)) -> MinimapLayout {
        let style = self.style;
        let margin = style.px(20.0);
        let radius = style.px(88.0);
        let ring = style.px(8.0);
        let banner_height = style.px(26.0);
        let outer = radius + ring;
        let center_x = viewport.0 as f32 - margin - outer;
        let banner = UiRect::new(center_x - outer, margin, outer * 2.0, banner_height);
        MinimapLayout {
            banner,
            center: (center_x, banner.bottom() + style.px(8.0) + outer),
            radius,
            ring,
        }
    }

    /// The minimap's projection this frame, if there is a level and a player.
    #[must_use]
    pub fn minimap_projection(
        &self,
        viewport: (u32, u32),
        markers: &MapMarkers,
    ) -> Option<MinimapProjection> {
        self.info.as_ref()?;
        let player = markers.player?;
        let layout = self.minimap_layout(viewport);
        Some(MinimapProjection {
            center: layout.center,
            radius: layout.radius,
            focus: (player.position.x, player.position.z),
            pixels_per_metre: layout.radius / MINIMAP_RANGE,
            rotation: match self.orientation {
                MinimapOrientation::Rotating => markers.view_yaw,
                MinimapOrientation::NorthUp => 0.0,
            },
        })
    }

    /// Where the full-screen map's parts sit in a viewport.
    #[must_use]
    pub fn screen_layout(&self, viewport: (u32, u32)) -> MapScreenLayout {
        let s = self.style;
        let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
        let outer = s.px(space::XXL);
        let width = (vw - outer * 2.0)
            .min(s.px(1840.0))
            .max(s.px(640.0))
            .min(vw);
        let height = (vh - outer * 2.0)
            .min(s.px(1080.0))
            .max(s.px(420.0))
            .min(vh);
        let panel = UiRect::new(
            ((vw - width) * 0.5).round(),
            ((vh - height) * 0.5).round(),
            width,
            height,
        );
        let header = UiRect::new(panel.x, panel.y, panel.width, s.px(76.0));
        let close_size = s.px(40.0);
        let close = UiRect::new(
            header.right() - close_size - s.px(space::XL),
            header.y + (header.height - close_size) * 0.5,
            close_size,
            close_size,
        );
        let progress_width = s.px(300.0);
        let progress = UiRect::new(
            close.x - s.px(space::XL) - progress_width,
            header.y + s.px(18.0),
            progress_width,
            header.height - s.px(36.0),
        );
        let pad = s.px(space::XL);
        let body_top = header.bottom() + s.px(space::LG);
        let body_bottom = panel.bottom() - pad;
        let sidebar_width = (panel.width * 0.29).clamp(s.px(380.0), s.px(520.0));
        let sidebar = UiRect::new(
            panel.right() - pad - sidebar_width,
            body_top,
            sidebar_width,
            body_bottom - body_top,
        );
        let canvas = UiRect::new(
            panel.x + pad,
            body_top,
            sidebar.x - s.px(space::LG) - (panel.x + pad),
            body_bottom - body_top,
        );
        let head_height = self.head.height.max(s.px(96.0)).min(sidebar.height * 0.5);
        let sidebar_head = UiRect::new(sidebar.x, sidebar.y, sidebar.width, head_height);
        let sidebar_body = UiRect::new(
            sidebar.x,
            sidebar_head.bottom(),
            sidebar.width,
            sidebar.bottom() - sidebar_head.bottom(),
        );

        let button = s.px(40.0);
        let gap = s.px(2.0);
        let controls_height = button * 4.0 + gap * 3.0 + s.px(8.0);
        let controls = UiRect::new(
            canvas.right() - s.px(space::LG) - button - s.px(8.0),
            canvas.bottom() - s.px(space::LG) - controls_height,
            button + s.px(8.0),
            controls_height,
        );
        let at = |index: f32| {
            UiRect::new(
                controls.x + s.px(4.0),
                controls.y + s.px(4.0) + index * (button + gap),
                button,
                button,
            )
        };
        let help_width = {
            let glyphs =
                s.px(16.0 + 6.0) * 3.0 + s.width(type_scale::LABEL, "Esc") + s.px(12.0 + 6.0);
            let words: f32 = ["Pan", "Zoom", "Inspect", "Close"]
                .iter()
                .map(|word| s.width(type_scale::UI, word) + s.px(space::LG))
                .sum();
            glyphs + words + s.px(space::LG)
        };
        let help = UiRect::new(
            canvas.x + s.px(space::LG),
            canvas.bottom() - s.px(space::LG) - s.px(38.0),
            help_width.min(canvas.width - s.px(120.0)),
            s.px(38.0),
        );
        MapScreenLayout {
            panel,
            header,
            close,
            progress,
            canvas,
            sidebar,
            sidebar_head,
            sidebar_body,
            zoom_in: at(0.0),
            zoom_out: at(1.0),
            fit: at(2.0),
            recenter: at(3.0),
            controls,
            help,
        }
    }

    /// The canvas's projection at the current pan and zoom. The whole level is framed with a
    /// margin so no room touches the canvas's edge.
    #[must_use]
    pub fn canvas_projection(&self, viewport: (u32, u32)) -> Option<CanvasProjection> {
        let info = self.info.as_ref()?;
        let canvas = self.screen_layout(viewport).canvas;
        let margin = self.style.px(space::XL) * 2.0;
        let fit = ((canvas.width - margin).max(1.0) / info.frame.width())
            .min((canvas.height - margin).max(1.0) / info.frame.depth());
        Some(CanvasProjection {
            canvas,
            center: self.center,
            pixels_per_metre: fit * self.zoom,
        })
    }

    /// A marker's size on the canvas: its class's, growing gently with zoom so it stays
    /// readable without swallowing the room.
    fn marker_size(&self, class: MarkerClass) -> f32 {
        let growth = (1.0 + (self.zoom - 1.0) * 0.06).min(1.3);
        self.style.px(class.size() * growth)
    }

    /// The encounter whose marker is under `point` on the canvas. Generous: the hit area is
    /// larger than the insignia, so no marker is a small target.
    fn encounter_at_canvas(
        &self,
        projection: &CanvasProjection,
        point: (f32, f32),
    ) -> Option<usize> {
        let info = self.info.as_ref()?;
        let mut best: Option<(f32, usize)> = None;
        for (index, encounter) in info.encounters.iter().enumerate() {
            let (x, y) = projection.to_screen(encounter.position);
            let reach = self.marker_size(MarkerClass::of(encounter)) * 0.75;
            let distance = ((x - point.0).powi(2) + (y - point.1).powi(2)).sqrt();
            if distance <= reach && best.is_none_or(|(held, _)| distance < held) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// The encounter whose marker is under `point` on the minimap.
    fn encounter_at_minimap(
        &self,
        projection: &MinimapProjection,
        point: (f32, f32),
    ) -> Option<usize> {
        let info = self.info.as_ref()?;
        let mut best: Option<(f32, usize)> = None;
        for (index, encounter) in info.encounters.iter().enumerate() {
            let Some((x, y)) = self.minimap_marker(projection, encounter) else {
                continue;
            };
            let reach = self.minimap_marker_size(MarkerClass::of(encounter)) * 0.7;
            let distance = ((x - point.0).powi(2) + (y - point.1).powi(2)).sqrt();
            if distance <= reach && best.is_none_or(|(held, _)| distance < held) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// A marker's size on the minimap.
    fn minimap_marker_size(&self, class: MarkerClass) -> f32 {
        self.style.px(match class {
            MarkerClass::Boss => 24.0,
            MarkerClass::Elite => 19.0,
            MarkerClass::Pack => 17.0,
        })
    }

    /// Where an encounter's marker is drawn on the minimap, if it is: packs inside the circle;
    /// the boss pinned to the rim when out of range, so the way to it is always shown.
    fn minimap_marker(
        &self,
        projection: &MinimapProjection,
        encounter: &MapEncounterView,
    ) -> Option<(f32, f32)> {
        let (x, y) = projection.to_screen(encounter.position);
        let (dx, dy) = (x - projection.center.0, y - projection.center.1);
        let distance = (dx * dx + dy * dy).sqrt();
        let class = MarkerClass::of(encounter);
        let inner = projection.radius - self.minimap_marker_size(class) * 0.4;
        if distance <= inner {
            return Some((x, y));
        }
        // Pinned a little further in than a marker in range, so it never sits under the north
        // badge on the rim.
        let pinned = projection.radius - self.minimap_marker_size(class) * 1.1;
        (class == MarkerClass::Boss && distance > 0.0).then(|| {
            (
                projection.center.0 + dx / distance * pinned,
                projection.center.1 + dy / distance * pinned,
            )
        })
    }

    /// Whether the pointer is over any part of the map this frame.
    #[must_use]
    pub fn wants_pointer(&self, pointer: (f32, f32), viewport: (u32, u32)) -> bool {
        if self.info.is_none() {
            return false;
        }
        self.open || self.minimap_layout(viewport).contains(pointer.0, pointer.1)
    }

    /// Whether a control can act right now.
    fn control_enabled(&self, control: MapControl) -> bool {
        match control {
            MapControl::ZoomIn => self.zoom_target < MAX_ZOOM - 1e-3,
            MapControl::ZoomOut => self.zoom_target > 1.0 + 1e-3,
            MapControl::Fit => {
                let centred = self.info.as_ref().is_some_and(|info| {
                    let (cx, cz) = info.frame.center();
                    (self.center.0 - cx).abs() < 0.5 && (self.center.1 - cz).abs() < 0.5
                });
                self.zoom_target > 1.0 + 1e-3 || !centred
            }
            MapControl::Recenter => self.player.is_some(),
            MapControl::Close => true,
        }
    }

    fn control_rect(layout: &MapScreenLayout, control: MapControl) -> UiRect {
        match control {
            MapControl::Close => layout.close,
            MapControl::ZoomIn => layout.zoom_in,
            MapControl::ZoomOut => layout.zoom_out,
            MapControl::Fit => layout.fit,
            MapControl::Recenter => layout.recenter,
        }
    }

    /// Carries out a control.
    fn press(&mut self, control: MapControl, projection: &CanvasProjection) -> MapOutcome {
        if !self.control_enabled(control) {
            return MapOutcome::Consumed;
        }
        match control {
            MapControl::Close => {
                self.close();
                return MapOutcome::Closed;
            }
            MapControl::ZoomIn => self.zoom_about(projection, None, ZOOM_STEP * ZOOM_STEP),
            MapControl::ZoomOut => self.zoom_about(projection, None, 1.0 / (ZOOM_STEP * ZOOM_STEP)),
            MapControl::Fit => self.fit(),
            MapControl::Recenter => {
                if let Some(player) = self.player {
                    self.center = player;
                    if self.zoom_target < 2.5 {
                        self.zoom_target = 2.5;
                        self.zoom_anchor = None;
                        self.zoom = if self.reduced_motion { 2.5 } else { self.zoom };
                    }
                    self.clamp_center();
                }
            }
        }
        MapOutcome::Consumed
    }

    // -------------------------------------------------------------------------------- input

    /// Answers this frame's keys, while the map is open.
    pub fn handle_keys(&mut self, keys: MapKeys, viewport: (u32, u32)) {
        if !self.open {
            return;
        }
        let Some(projection) = self.canvas_projection(viewport) else {
            return;
        };
        if keys.zoom_in {
            self.press(MapControl::ZoomIn, &projection);
        }
        if keys.zoom_out {
            self.press(MapControl::ZoomOut, &projection);
        }
        if keys.fit {
            self.press(MapControl::Fit, &projection);
        }
        if keys.recenter {
            self.press(MapControl::Recenter, &projection);
        }
        let count = self.info.as_ref().map_or(0, |info| info.encounters.len());
        if count > 0 && (keys.next || keys.previous) {
            let next = match (self.selected, keys.next) {
                (None, true) => 0,
                (None, false) => count - 1,
                (Some(index), true) => (index + 1) % count,
                (Some(index), false) => (index + count - 1) % count,
            };
            self.select(Some(next));
        }
    }

    /// Answers this frame's pointer and wheel. `wheel` is notches, positive away from the
    /// player.
    pub fn update(
        &mut self,
        input: &UiInput,
        wheel: f32,
        viewport: (u32, u32),
        markers: &MapMarkers,
    ) -> MapOutcome {
        self.pointer = input.pointer;
        if self.info.is_none() {
            return MapOutcome::Idle;
        }
        self.observe(markers);
        if !self.open {
            return self.update_minimap(input, viewport, markers);
        }
        let layout = self.screen_layout(viewport);
        if self.stale || (self.laid_out_width - layout.sidebar.width).abs() > 0.5 {
            self.layout_sidebar(layout.sidebar.width);
        }
        let layout = self.screen_layout(viewport);
        let pointer = input.pointer;
        let Some(projection) = self.canvas_projection(viewport) else {
            return MapOutcome::Consumed;
        };

        // Controls: hover for the tooltip, press to act. They sit on the canvas and must not
        // start a drag.
        let mut hover = None;
        for control in [
            MapControl::Close,
            MapControl::ZoomIn,
            MapControl::ZoomOut,
            MapControl::Fit,
            MapControl::Recenter,
        ] {
            if Self::control_rect(&layout, control).contains(pointer.0, pointer.1) {
                hover = Some(Hover::Control(control));
                if input.pointer_pressed {
                    return self.press(control, &projection);
                }
            }
        }
        let on_plate = layout.controls.contains(pointer.0, pointer.1)
            || layout.help.contains(pointer.0, pointer.1);

        // The canvas: wheel zooms about the cursor, a drag pans, a click selects.
        let on_canvas =
            layout.canvas.contains(pointer.0, pointer.1) && !on_plate && hover.is_none();
        if on_canvas && wheel != 0.0 {
            self.zoom_about(&projection, Some(pointer), ZOOM_STEP.powf(wheel));
        }
        if input.pointer_pressed && on_canvas {
            self.drag = Some(Drag {
                start: pointer,
                center: self.center,
                moved: false,
            });
        }
        if let Some(mut drag) = self.drag {
            let (dx, dy) = (pointer.0 - drag.start.0, pointer.1 - drag.start.1);
            if !drag.moved && dx * dx + dy * dy > self.style.px(4.0).powi(2) {
                drag.moved = true;
            }
            if drag.moved && input.pointer_down {
                self.zoom_anchor = None;
                self.center = (
                    drag.center.0 - dx / projection.pixels_per_metre,
                    drag.center.1 - dy / projection.pixels_per_metre,
                );
                self.clamp_center();
            }
            self.drag = Some(drag);
            if input.pointer_released || !input.pointer_down {
                self.drag = None;
                if !drag.moved {
                    let hit = self.encounter_at_canvas(&projection, pointer);
                    self.select(hit);
                }
            }
        }
        if hover.is_none() && on_canvas && self.drag.is_none_or(|drag| !drag.moved) {
            hover = self
                .encounter_at_canvas(&projection, pointer)
                .map(Hover::Marker);
        }
        self.set_hover(hover);

        // The journal scrolls on its own; its rows open, filter and select.
        if layout.sidebar_body.contains(pointer.0, pointer.1) {
            if wheel != 0.0 {
                self.scroll -= wheel * self.style.px(56.0);
            }
            if input.pointer_pressed
                && let Some(action) = self.action_at(&layout, pointer)
            {
                match action {
                    Action::Toggle(section) => self.toggle_section(section),
                    Action::Select(index) => self.select(Some(index)),
                    Action::Filter(filter) => {
                        if self.filter != filter {
                            self.filter = filter;
                            self.stale = true;
                        }
                    }
                }
            }
        }
        if self.stale {
            self.layout_sidebar(layout.sidebar.width);
        }
        self.clamp_scroll(&self.screen_layout(viewport));
        MapOutcome::Consumed
    }

    /// Changes what the pointer rests on, restarting the tooltip's delay when it moves.
    fn set_hover(&mut self, hover: Option<Hover>) {
        if hover != self.hovered {
            self.hovered = hover;
            self.hover_time = 0.0;
        }
    }

    /// The minimap's share of an update: hover, and a click that opens the map.
    fn update_minimap(
        &mut self,
        input: &UiInput,
        viewport: (u32, u32),
        markers: &MapMarkers,
    ) -> MapOutcome {
        let layout = self.minimap_layout(viewport);
        let pointer = input.pointer;
        let over = layout.contains(pointer.0, pointer.1);
        let hit = if over {
            self.minimap_projection(viewport, markers)
                .and_then(|projection| self.encounter_at_minimap(&projection, pointer))
        } else {
            None
        };
        self.set_hover(hit.map(Hover::Minimap));
        if over && input.pointer_pressed {
            self.open();
            if hit.is_some() {
                self.select(hit);
            }
            return MapOutcome::Opened;
        }
        MapOutcome::Idle
    }

    /// Zooms by `factor`, keeping the world under `anchor` (or the canvas centre) still.
    fn zoom_about(
        &mut self,
        projection: &CanvasProjection,
        anchor: Option<(f32, f32)>,
        factor: f32,
    ) {
        let target = (self.zoom_target * factor).clamp(1.0, MAX_ZOOM);
        if (target - self.zoom_target).abs() < f32::EPSILON {
            return;
        }
        self.zoom_target = target;
        let screen = anchor.unwrap_or((
            projection.canvas.x + projection.canvas.width * 0.5,
            projection.canvas.y + projection.canvas.height * 0.5,
        ));
        self.zoom_anchor = Some((projection.to_world(screen), screen));
        if self.reduced_motion {
            let ppm = projection.pixels_per_metre / self.zoom * target;
            self.zoom = target;
            let (world, canvas) = (projection.to_world(screen), projection.canvas);
            self.center = (
                world.0 - (screen.0 - canvas.x - canvas.width * 0.5) / ppm,
                world.1 - (screen.1 - canvas.y - canvas.height * 0.5) / ppm,
            );
            self.clamp_center();
        }
    }

    /// Keeps the canvas's centre inside the level's frame.
    fn clamp_center(&mut self) {
        if let Some(info) = &self.info {
            let frame = info.frame;
            self.center = (
                self.center.0.clamp(frame.min_x, frame.max_x),
                self.center.1.clamp(frame.min_z, frame.max_z),
            );
        }
    }

    // ------------------------------------------------------------------------------ journal

    fn toggle_section(&mut self, section: usize) {
        if self.expanded.len() <= section {
            self.expanded.resize(section + 1, false);
        }
        self.expanded[section] = !self.expanded[section];
        self.stale = true;
    }

    /// Whether section `section` is open, given whether it starts open.
    fn is_open_section(&self, section: usize, default: bool) -> bool {
        self.expanded.get(section).copied().unwrap_or(false) != default
    }

    /// The journal row under the pointer that does something, if any.
    fn action_at(&self, layout: &MapScreenLayout, pointer: (f32, f32)) -> Option<Action> {
        let origin_y = layout.sidebar_body.y + self.style.px(space::LG) - self.scroll;
        let (left, pad) = (layout.sidebar.x, self.style.px(space::XL));
        for block in &self.blocks {
            let top = origin_y + block.y;
            if pointer.1 < top || pointer.1 >= top + block.height {
                continue;
            }
            if let BlockKind::Filters { tabs, .. } = &block.kind {
                return tabs.iter().find_map(|(filter, _, x, width)| {
                    let x = left + pad + x;
                    (pointer.0 >= x && pointer.0 < x + width).then_some(Action::Filter(*filter))
                });
            }
            if block.action.is_some() {
                return block.action;
            }
        }
        None
    }

    fn content_height(&self) -> f32 {
        self.blocks
            .last()
            .map_or(0.0, |block| block.y + block.height)
    }

    fn clamp_scroll(&mut self, layout: &MapScreenLayout) {
        let visible = layout.sidebar_body.height - self.style.px(space::LG) * 2.0;
        let limit = (self.content_height() - visible).max(0.0);
        self.scroll = self.scroll.clamp(0.0, limit);
    }

    /// Lays out chips `text` in rows no wider than `width`, starting at `(0, y)`.
    fn lay_chips(&self, entries: &[(UiColor, String)], width: f32, y: f32) -> (Vec<Chip>, f32) {
        let s = self.style;
        let (height, gap) = (s.px(30.0), s.px(space::SM));
        let mut chips = Vec::new();
        let (mut x, mut row_y) = (0.0, y);
        for (tint, text) in entries {
            // Portrait (24) and its insets, the text, then the right padding.
            let chip_width = s.px(35.0) + s.width(type_scale::META, text) + s.px(space::MD);
            if x > 0.0 && x + chip_width > width {
                x = 0.0;
                row_y += height + gap;
            }
            chips.push(Chip {
                tint: *tint,
                text: text.clone(),
                x,
                y: row_y,
                width: chip_width.min(width),
            });
            x += chip_width + gap;
        }
        let bottom = if chips.is_empty() { y } else { row_y + height };
        (chips, bottom - y)
    }

    /// Lays the journal out for the current selection: the cache every frame draws from.
    fn layout_sidebar(&mut self, width: f32) {
        let s = self.style;
        let pad = s.px(space::XL);
        let inner = width - pad * 2.0;
        let mut blocks: Vec<(BlockKind, f32, Option<Action>)> = Vec::new();
        let mut cards: Vec<(usize, usize, CardTone)> = Vec::new();
        let mut head = Head::default();
        let Some(info) = self.info.clone() else {
            self.blocks.clear();
            self.cards.clear();
            return;
        };
        let lines = |style: TextStyle, text: &str, color: UiColor, indent: f32| {
            let wrapped = s.wrap(style, text, inner - indent);
            let height = s.line(style) * wrapped.len() as f32;
            (
                BlockKind::Lines {
                    lines: wrapped,
                    style,
                    color,
                    indent,
                },
                height,
                None,
            )
        };
        let gap = |height: f32| (BlockKind::Gap, s.px(height), None);
        let label = |text: &str, accent: UiColor| {
            (
                BlockKind::Label {
                    text: text.to_uppercase(),
                    accent,
                },
                s.px(28.0),
                None,
            )
        };

        match self.selected.and_then(|index| info.encounters.get(index)) {
            None => {
                let (packs, monsters, bosses) = info.totals();
                head.icon = Some(UiIcon::EmblemCrypt);
                head.title = vec![title_case(&info.name)];
                head.subtitle = format!(
                    "{packs} packs \u{00B7} {monsters} enemies \u{00B7} {bosses} boss{}",
                    if bosses == 1 { "" } else { "es" }
                );
                if self.progress.is_some() {
                    blocks.push(label("Progress", color::BRONZE));
                    let start = blocks.len();
                    blocks.push((BlockKind::Progress, s.px(92.0), None));
                    cards.push((start, blocks.len(), CardTone::Raised));
                    blocks.push(gap(space::XL));
                }
                blocks.push(label("Encounters", color::BRONZE));
                for (index, encounter) in info.encounters.iter().enumerate() {
                    let class = MarkerClass::of(encounter);
                    let detail = match class {
                        MarkerClass::Boss => "Boss encounter".to_owned(),
                        _ => enemies(encounter.monster_count()),
                    };
                    blocks.push((
                        BlockKind::Encounter {
                            class,
                            name: encounter.title.clone(),
                            detail,
                            status: self.status(index),
                        },
                        s.px(48.0),
                        Some(Action::Select(index)),
                    ));
                }
                blocks.push(gap(space::XL));
                blocks.push(label("Legend", color::BRONZE));
                for (icon, tint, text) in [
                    (UiIcon::MapPack, [1.0; 4], "Monster pack"),
                    (UiIcon::CrestElite, [1.0; 4], "Elite pack"),
                    (UiIcon::MapBoss, [1.0; 4], "Boss encounter"),
                    (UiIcon::MapPlayer, color::BRONZE_BRIGHT, "Your character"),
                    (UiIcon::MapPlayer, color::VERDANT, "Party member"),
                ] {
                    blocks.push((
                        BlockKind::Legend {
                            icon,
                            tint,
                            label: text.to_owned(),
                        },
                        s.px(32.0),
                        None,
                    ));
                }
            }
            Some(encounter) => {
                let index = self.selected.unwrap_or(0);
                let class = MarkerClass::of(encounter);
                let boss = encounter.boss().cloned();
                head.icon = Some(class.icon());
                let title = boss
                    .as_ref()
                    .map_or_else(|| encounter.title.clone(), MapMember::name);
                head.title = s.wrap(type_scale::ENCOUNTER, &title, inner - s.px(64.0));
                head.subtitle = match &boss {
                    Some(_) if !encounter.key.starts_with('#') => {
                        format!("Boss encounter \u{00B7} {}", title_case(&encounter.key))
                    }
                    Some(_) => "Boss encounter".to_owned(),
                    None => format!(
                        "{} \u{00B7} {}",
                        class.label(),
                        enemies(encounter.monster_count())
                    ),
                };
                head.badge = match (self.status(index), class, self.progress) {
                    (EncounterStatus::Cleared, _, _) => {
                        Some(("Cleared".to_owned(), color::VERDANT))
                    }
                    (EncounterStatus::Engaged, _, _) => {
                        Some(("In combat".to_owned(), color::CRIMSON))
                    }
                    (_, MarkerClass::Boss, Some(progress)) if !progress.unlocked => {
                        Some(("Sealed".to_owned(), color::BRONZE_BRIGHT))
                    }
                    _ => None,
                };
                let roster: Vec<(UiColor, String)> = encounter
                    .members
                    .iter()
                    .map(|member| {
                        (
                            portrait_tint(member),
                            format!("{} \u{00D7} {}", member.count, member.name()),
                        )
                    })
                    .collect();
                if encounter.members.len() > 1 || encounter.monster_count() > 1 {
                    head.chips = self.lay_chips(&roster, inner, 0.0).0;
                }

                // A boss's own card: what it is, then its abilities by phase.
                let mut section = 0usize;
                if let Some(MapMember {
                    kind: Some(kind),
                    level,
                    ..
                }) = &boss
                {
                    let archetype = kind.archetype();
                    let (health, _) = level_scale(*level);
                    blocks.push(lines(
                        type_scale::BODY,
                        archetype.summary,
                        color::TEXT_SECONDARY,
                        0.0,
                    ));
                    blocks.push(gap(space::SM));
                    blocks.push(lines(
                        type_scale::META,
                        &format!(
                            "{} health \u{00B7} {} phase{} \u{00B7} Level {}",
                            thousands(archetype.max_health * health),
                            archetype.phase_count(),
                            if archetype.phase_count() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            level
                        ),
                        color::TEXT_MUTED,
                        0.0,
                    ));
                    blocks.push(gap(space::LG));
                    self.push_filters(&mut blocks, std::slice::from_ref(kind), inner);
                    blocks.push(label("Abilities", color::CRIMSON));
                    section = self.push_abilities(&mut blocks, *kind, *level, inner, section, true);
                    blocks.push(gap(space::MD));
                } else {
                    let kinds: Vec<EnemyKind> = encounter
                        .members
                        .iter()
                        .filter_map(|member| member.kind)
                        .collect();
                    self.push_filters(&mut blocks, &kinds, inner);
                }

                // Everyone else, one folding card each.
                let mut first = true;
                for member in &encounter.members {
                    if boss.as_ref().is_some_and(|boss| boss.id == member.id) {
                        continue;
                    }
                    let open = self.is_open_section(section, first && boss.is_none());
                    first = false;
                    let (role, meta) =
                        member
                            .kind
                            .map_or(("Unknown".to_owned(), String::new()), |kind| {
                                let archetype = kind.archetype();
                                let (health, _) = level_scale(member.level);
                                (
                                    title_case(archetype.role.as_str()),
                                    format!(
                                        " \u{00B7} Level {} \u{00B7} {} health",
                                        member.level,
                                        thousands(archetype.max_health * health)
                                    ),
                                )
                            });
                    let start = blocks.len();
                    blocks.push((
                        BlockKind::Monster {
                            tint: portrait_tint(member),
                            name: member.name(),
                            meta: format!("{role}{meta}"),
                            count: member.count,
                            open,
                        },
                        s.px(72.0),
                        Some(Action::Toggle(section)),
                    ));
                    section += 1;
                    if open {
                        match member.kind {
                            Some(kind) => {
                                blocks.push(lines(
                                    type_scale::BODY,
                                    kind.archetype().summary,
                                    color::TEXT_SECONDARY,
                                    s.px(space::LG),
                                ));
                                blocks.push(gap(space::SM));
                                section = self.push_abilities(
                                    &mut blocks,
                                    kind,
                                    member.level,
                                    inner,
                                    section,
                                    false,
                                );
                            }
                            None => blocks.push(lines(
                                type_scale::BODY,
                                "This build does not know this monster.",
                                color::TEXT_MUTED,
                                s.px(space::LG),
                            )),
                        }
                        blocks.push(gap(space::SM));
                    }
                    cards.push((start, blocks.len(), CardTone::Raised));
                    blocks.push(gap(space::MD));
                }

                // A boss's guests and its loot.
                if let Some(boss_member) = &boss {
                    if let Some(adds) = &info.boss_adds {
                        blocks.push(gap(space::SM));
                        blocks.push(label("Encounter mechanic", color::ARCANE));
                        let start = blocks.len();
                        blocks.push((
                            BlockKind::Guests {
                                title: "Summoned guests".to_owned(),
                            },
                            s.px(44.0),
                            None,
                        ));
                        blocks.push(lines(
                            type_scale::BODY,
                            &format!(
                                "When the fight begins, {} of these join it in their empowered form{}:",
                                adds.count,
                                if info.boss_adds.as_ref().is_some_and(|adds| adds.count > 1) { ", never the same one twice" } else { "" }
                            ),
                            color::TEXT_SECONDARY,
                            s.px(space::LG),
                        ));
                        blocks.push(gap(space::SM));
                        let entries: Vec<(UiColor, String)> = adds
                            .pool
                            .iter()
                            .map(|member| (portrait_tint(member), member.name()))
                            .collect();
                        let (chips, height) =
                            self.lay_chips(&entries, inner - s.px(space::LG) * 2.0, 0.0);
                        blocks.push((BlockKind::Chips { chips }, height + s.px(space::MD), None));
                        cards.push((start, blocks.len(), CardTone::Inset));
                        blocks.push(gap(space::XL));
                    }
                    blocks.push(label("Loot", color::BRONZE));
                    let loot = boss_member
                        .kind
                        .map_or(&[][..], |kind| kind.archetype().loot);
                    let rows: Vec<LootLine> = loot.iter().filter_map(loot_detail).collect();
                    if rows.is_empty() {
                        let start = blocks.len();
                        let text = s.wrap(
                            type_scale::BODY,
                            "No loot has been configured for this encounter.",
                            inner - s.px(96.0),
                        );
                        let height = s.px(40.0).max(s.line(type_scale::BODY) * text.len() as f32)
                            + s.px(space::LG) * 2.0;
                        blocks.push((
                            BlockKind::Empty {
                                icon: UiIcon::LootChest,
                                lines: text,
                            },
                            height,
                            None,
                        ));
                        cards.push((start, blocks.len(), CardTone::Inset));
                    } else {
                        for category in ["Armor", "Jewelry", "Trinkets"] {
                            let in_category: Vec<&LootLine> =
                                rows.iter().filter(|row| row.category == category).collect();
                            if in_category.is_empty() {
                                continue;
                            }
                            blocks.push((
                                BlockKind::Lines {
                                    lines: vec![category.to_owned()],
                                    style: type_scale::HEADING,
                                    color: color::TEXT_SECONDARY,
                                    indent: 0.0,
                                },
                                s.line(type_scale::HEADING) + s.px(4.0),
                                None,
                            ));
                            for row in in_category {
                                let start = blocks.len();
                                blocks.push((
                                    BlockKind::Loot { line: row.clone() },
                                    s.px(74.0),
                                    None,
                                ));
                                cards.push((start, blocks.len(), CardTone::Raised));
                                blocks.push(gap(space::SM));
                            }
                        }
                    }
                }
            }
        }

        // The fixed header's height.
        let title_height = s.line(type_scale::ENCOUNTER) * head.title.len().max(1) as f32;
        let mut height = s.px(space::XL)
            + title_height.max(s.px(52.0) - s.line(type_scale::UI))
            + s.line(type_scale::UI)
            + s.px(space::XL);
        if !head.chips.is_empty() {
            let chips_bottom = head
                .chips
                .iter()
                .map(|chip| chip.y + s.px(30.0))
                .fold(0.0, f32::max);
            height += chips_bottom + s.px(space::SM);
        }
        head.height = height;
        self.head = head;

        self.blocks.clear();
        let mut y = 0.0;
        for (kind, height, action) in blocks {
            self.blocks.push(Block {
                kind,
                y,
                height,
                action,
            });
            y += height;
        }
        self.cards = cards
            .into_iter()
            .filter(|(start, end, _)| end > start)
            .map(|(start, end, tone)| Card {
                top: self.blocks[start].y,
                bottom: self.blocks[end - 1].y + self.blocks[end - 1].height,
                tone,
            })
            .collect();
        self.stale = false;
        self.laid_out_width = width;
    }

    /// The ability filter tabs, when the encounter's abilities fall into more than one kind.
    fn push_filters(
        &self,
        blocks: &mut Vec<(BlockKind, f32, Option<Action>)>,
        kinds: &[EnemyKind],
        width: f32,
    ) {
        let s = self.style;
        let defs: Vec<&EnemyAbilityDef> = kinds
            .iter()
            .flat_map(|kind| abilities_of(*kind))
            .map(|(ability, _)| ability.definition())
            .collect();
        if defs.len() < 4 {
            return;
        }
        let mut tabs = Vec::new();
        let mut x = 0.0;
        for filter in AbilityFilter::ALL {
            let count = defs.iter().filter(|def| filter.accepts(def)).count();
            if count == 0 || (filter != AbilityFilter::All && count == defs.len()) {
                continue;
            }
            let text = format!("{} {count}", filter.label());
            let tab_width = s.width(type_scale::UI, &text) + s.px(space::LG) * 1.5;
            if x + tab_width > width {
                break;
            }
            tabs.push((filter, count, x, tab_width));
            x += tab_width + s.px(space::XS);
        }
        if tabs.len() > 1 {
            blocks.push((
                BlockKind::Filters {
                    tabs,
                    active: self.filter,
                },
                s.px(34.0),
                None,
            ));
            blocks.push((BlockKind::Gap, s.px(space::MD), None));
        }
    }

    /// `kind`'s abilities that pass the filter, as rows; each opens onto its details. Returns
    /// the next free section number.
    fn push_abilities(
        &self,
        blocks: &mut Vec<(BlockKind, f32, Option<Action>)>,
        kind: EnemyKind,
        level: u8,
        width: f32,
        mut section: usize,
        phases: bool,
    ) -> usize {
        let s = self.style;
        let archetype = kind.archetype();
        let (_, damage_scale) = level_scale(level);
        let indent = s.px(space::LG);
        let text_indent = indent + s.px(36.0) + s.px(space::MD);
        let text_width = width - text_indent - s.px(space::LG);
        let mut last_phase = usize::MAX;
        let mut shown = 0;
        for (ability, phase) in abilities_of(kind) {
            let def = ability.definition();
            if !self.filter.accepts(def) {
                section += 1;
                continue;
            }
            if phases && archetype.phase_count() > 1 && phase != last_phase {
                let heading = match phase
                    .checked_sub(1)
                    .and_then(|index| archetype.phases.get(index))
                {
                    Some(definition) => format!(
                        "{} \u{00B7} below {}% health",
                        title_case(definition.name),
                        (definition.at_or_below * 100.0).round() as i32
                    ),
                    None => title_case(archetype.phase_name(0)),
                };
                blocks.push((
                    BlockKind::Lines {
                        lines: vec![heading],
                        style: type_scale::META,
                        color: color::CRIMSON,
                        indent: 0.0,
                    },
                    s.line(type_scale::META) + s.px(space::SM),
                    None,
                ));
                last_phase = phase;
            }
            let open = self.is_open_section(section, false);
            let summary = if def.description.is_empty() {
                String::new()
            } else {
                s.fit(type_scale::META, def.description, text_width - s.px(60.0))
            };
            blocks.push((
                BlockKind::Ability {
                    icon: ability_icon(def),
                    accent: ability_accent(def),
                    name: title_case(def.name),
                    meta: ability_timing(def, archetype.phase_cooldown_scale(phase)),
                    summary: if open { String::new() } else { summary },
                    tags: ability_tags(def),
                    open,
                },
                s.px(if open { 56.0 } else { 70.0 }),
                Some(Action::Toggle(section)),
            ));
            section += 1;
            shown += 1;
            if open {
                if !def.description.is_empty() {
                    let wrapped = s.wrap(type_scale::BODY, def.description, text_width);
                    let height = s.line(type_scale::BODY) * wrapped.len() as f32;
                    blocks.push((
                        BlockKind::Lines {
                            lines: wrapped,
                            style: type_scale::BODY,
                            color: color::TEXT_PRIMARY,
                            indent: text_indent,
                        },
                        height + s.px(space::SM),
                        None,
                    ));
                }
                let mut bullets = Vec::new();
                for effect in ability_effects(def, damage_scale) {
                    for (index, line) in s
                        .wrap(type_scale::BODY, &effect, text_width - s.px(space::LG))
                        .into_iter()
                        .enumerate()
                    {
                        bullets.push((line, index > 0));
                    }
                }
                if !bullets.is_empty() {
                    let height = s.line(type_scale::BODY) * bullets.len() as f32;
                    blocks.push((
                        BlockKind::Bullets {
                            lines: bullets,
                            dot: ability_accent(def),
                        },
                        height + s.px(space::SM),
                        None,
                    ));
                }
                if !def.counter.is_empty() {
                    let counter = sentence(def.counter);
                    let wrapped = s.wrap(type_scale::BODY, &counter, text_width - s.px(space::XL));
                    let height = s.line(type_scale::LABEL)
                        + s.line(type_scale::BODY) * wrapped.len() as f32
                        + s.px(space::MD) * 2.0;
                    blocks.push((BlockKind::Counterplay { lines: wrapped }, height, None));
                    blocks.push((BlockKind::Gap, s.px(space::SM), None));
                }
                blocks.push((BlockKind::Gap, s.px(space::SM), None));
            }
        }
        if shown == 0 {
            blocks.push((
                BlockKind::Lines {
                    lines: vec![format!(
                        "No {} abilities.",
                        self.filter.label().to_lowercase()
                    )],
                    style: type_scale::META,
                    color: color::TEXT_MUTED,
                    indent,
                },
                s.line(type_scale::META) + s.px(space::SM),
                None,
            ));
        }
        section
    }

    /// What the journal says for the current selection, one string per line, laid out for
    /// `viewport` — the same cache the panel is drawn from. For tests and for a log line.
    pub fn sidebar_lines(&mut self, viewport: (u32, u32)) -> Vec<String> {
        let width = self.screen_layout(viewport).sidebar.width;
        if self.stale || (self.laid_out_width - width).abs() > 0.5 {
            self.layout_sidebar(width);
        }
        let mut out = Vec::new();
        if !self.head.title.is_empty() {
            out.push(format!(
                "{} | {}",
                self.head.title.join(" "),
                self.head.subtitle
            ));
        }
        for chip in &self.head.chips {
            out.push(format!("CHIP {}", chip.text));
        }
        for block in &self.blocks {
            match &block.kind {
                BlockKind::Label { text, .. } => out.push(format!("# {text}")),
                BlockKind::Lines { lines, .. } => out.push(lines.join(" ")),
                BlockKind::Monster {
                    name,
                    meta,
                    count,
                    open,
                    ..
                } => out.push(format!(
                    "[{}] {name} x{count} | {meta}",
                    if *open { "-" } else { "+" }
                )),
                BlockKind::Ability {
                    name, meta, open, ..
                } => out.push(format!(
                    "[{}] ABILITY {name} | {meta}",
                    if *open { "-" } else { "+" }
                )),
                BlockKind::Bullets { lines, .. } => {
                    out.extend(lines.iter().map(|(line, _)| format!("* {line}")))
                }
                BlockKind::Counterplay { lines } => {
                    out.push(format!("COUNTERPLAY: {}", lines.join(" ")))
                }
                BlockKind::Filters { tabs, .. } => out.push(format!(
                    "FILTERS {}",
                    tabs.iter()
                        .map(|(filter, count, _, _)| format!("{} {count}", filter.label()))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                BlockKind::Encounter { name, detail, .. } => {
                    out.push(format!("ENCOUNTER {name} | {detail}"))
                }
                BlockKind::Legend { label, .. } => out.push(format!("LEGEND {label}")),
                BlockKind::Empty { lines, .. } => out.push(format!("EMPTY {}", lines.join(" "))),
                BlockKind::Loot { line } => out.push(format!(
                    "LOOT {} | {} | {} | {}",
                    line.name, line.meta, line.stats, line.chance
                )),
                BlockKind::Progress => out.push("PROGRESS".to_owned()),
                BlockKind::Chips { chips } => {
                    out.extend(chips.iter().map(|chip| format!("CHIP {}", chip.text)))
                }
                BlockKind::Guests { title } => out.push(format!("GUESTS {title}")),
                BlockKind::Gap => {}
            }
        }
        out
    }

    /// Opens every folded section of the current selection: monster cards and abilities.
    pub fn expand_all(&mut self) {
        loop {
            let folded: Vec<usize> = self
                .blocks
                .iter()
                .filter_map(|block| match (&block.kind, block.action) {
                    (
                        BlockKind::Monster { open: false, .. }
                        | BlockKind::Ability { open: false, .. },
                        Some(Action::Toggle(section)),
                    ) => Some(section),
                    _ => None,
                })
                .collect();
            if folded.is_empty() {
                break;
            }
            for section in folded {
                self.toggle_section(section);
            }
            let width = self.laid_out_width.max(1.0);
            self.layout_sidebar(width);
        }
    }

    // ------------------------------------------------------------------------------ drawing

    /// Draws the minimap, if there is a level and a player.
    pub fn push_minimap(
        &self,
        list: &mut UiDrawList,
        markers: &MapMarkers,
        viewport: (u32, u32),
        _theme: &MapTheme,
    ) {
        let Some(info) = &self.info else {
            return;
        };
        let Some(projection) = self.minimap_projection(viewport, markers) else {
            return;
        };
        let s = self.style;
        let layout = self.minimap_layout(viewport);
        let (cx, cy) = layout.center;
        let outer = layout.radius + layout.ring;

        // The name plate.
        let plate = layout.banner;
        list.shadow(
            plate,
            plate.height * 0.5,
            s.px(10.0),
            color::alpha(color::SHADOW, 0.6),
        );
        list.panel(
            plate,
            plate.height * 0.5,
            color::SURFACE_1,
            color::SURFACE_0_BOTTOM,
        );
        list.panel_stroke(plate, plate.height * 0.5, s.px(1.0), color::BORDER_STRONG);
        // The title `advance` cached for this viewport and scale, or - drawn without an
        // `advance` first, as a test or a one-off render does - built here.
        let (width, key) = self.minimap_title_key(viewport);
        let built;
        let name: &str = if self.minimap_title_key == Some(key) {
            &self.minimap_title
        } else {
            built = s.fit(type_scale::UI, &title_case(&info.name), width);
            &built
        };
        list.label(
            (
                cx,
                s.centred_top(type_scale::UI, plate.y + plate.height * 0.5),
            ),
            name,
            color::TEXT_TITLE,
            type_scale::UI.font,
            s.size(type_scale::UI),
            TextAlign::Center,
        );

        // The frame: shadow, bronze ring, inner bevel, ground.
        let ring_rect =
            |radius: f32| UiRect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
        list.shadow(
            ring_rect(outer),
            outer,
            s.px(16.0),
            color::alpha(color::SHADOW, 0.8),
        );
        list.panel(
            ring_rect(outer),
            outer,
            color::BRONZE_BRIGHT,
            color::BRONZE_DIM,
        );
        list.panel(
            ring_rect(outer - s.px(2.0)),
            outer,
            color::BRONZE_DIM,
            color::SURFACE_0_BOTTOM,
        );
        list.panel(
            ring_rect(layout.radius + s.px(1.0)),
            layout.radius,
            color::MAP_GROUND,
            color::MAP_GROUND,
        );

        // The picture and everything on it, clipped to the circle.
        let square = ring_rect(layout.radius);
        list.clip(UiClip::circle((cx, cy), layout.radius));
        if info.has_picture {
            let (origin, du, dv) = projection.picture_mapping(&info.frame, square);
            list.map_image(square, origin, du, dv, [1.0; 4]);
        }
        for (index, encounter) in info.encounters.iter().enumerate() {
            let Some(at) = self.minimap_marker(&projection, encounter) else {
                continue;
            };
            let class = MarkerClass::of(encounter);
            let emphasis = self.emphasis.get(index).copied().unwrap_or(0.0);
            let size = self.minimap_marker_size(class) * (1.0 + 0.15 * emphasis);
            let tint = if self.status(index) == EncounterStatus::Cleared {
                [0.62, 0.62, 0.62, 0.7]
            } else {
                [1.0; 4]
            };
            list.icon(
                UiRect::new(at.0 - size * 0.5, at.1 - size * 0.5, size, size),
                class.icon(),
                tint,
            );
        }
        for member in &markers.party {
            let at = projection.to_screen(member.position);
            if (at.0 - cx).powi(2) + (at.1 - cy).powi(2) > layout.radius.powi(2) {
                continue;
            }
            push_unit(
                list,
                at,
                member.facing.map(|yaw| projection.arrow_rotation(yaw)),
                s.px(14.0),
                color::VERDANT,
            );
        }
        list.clip(UiClip::NONE);
        // A soft inner shadow along the rim, so the picture sits under the frame.
        list.panel_stroke(
            ring_rect(layout.radius),
            layout.radius,
            s.px(3.0),
            color::alpha(color::SHADOW, 0.45),
        );
        if let Some(player) = markers.player {
            let at = projection.to_screen(player.position);
            list.glow(
                UiRect::new(at.0 - s.px(9.0), at.1 - s.px(9.0), s.px(18.0), s.px(18.0)),
                s.px(9.0),
                s.px(8.0),
                color::alpha(color::BRONZE_BRIGHT, 0.35),
            );
            push_unit(
                list,
                at,
                Some(projection.arrow_rotation(player.facing.unwrap_or(markers.view_yaw))),
                s.px(18.0),
                [1.0; 4],
            );
        }

        // North, on the rim: turns with a rotating map.
        let (nx, ny) = turn(projection.rotation, 0.0, -(outer - s.px(2.0)));
        let north = (cx + nx, cy + ny);
        let badge = s.px(10.0);
        list.panel(
            UiRect::new(north.0 - badge, north.1 - badge, badge * 2.0, badge * 2.0),
            badge,
            color::SURFACE_1,
            color::SURFACE_0_BOTTOM,
        );
        list.panel_stroke(
            UiRect::new(north.0 - badge, north.1 - badge, badge * 2.0, badge * 2.0),
            badge,
            s.px(1.0),
            color::BRONZE,
        );
        list.label(
            (north.0, s.centred_top(type_scale::LABEL, north.1)),
            "N",
            color::BRONZE_BRIGHT,
            UiFont::SemiBold,
            s.size(type_scale::LABEL),
            TextAlign::Center,
        );

        if let Some(Hover::Minimap(index)) = self.hovered
            && self.hover_time >= motion::TOOLTIP_DELAY * 0.6
            && let Some(encounter) = info.encounters.get(index)
        {
            let (title, detail) = self.marker_tooltip(index, encounter);
            push_tooltip(list, self.pointer, &title, &detail, viewport, s);
        }
    }

    /// A marker's tooltip text.
    fn marker_tooltip(&self, index: usize, encounter: &MapEncounterView) -> (String, String) {
        let class = MarkerClass::of(encounter);
        let title = encounter
            .boss()
            .map_or_else(|| encounter.title.clone(), MapMember::name);
        let mut detail = match class {
            MarkerClass::Boss => class.label().to_owned(),
            _ => format!(
                "{} \u{00B7} {}",
                class.label(),
                enemies(encounter.monster_count())
            ),
        };
        match self.status(index) {
            EncounterStatus::Cleared => detail.push_str(" \u{00B7} Cleared"),
            EncounterStatus::Engaged => detail.push_str(" \u{00B7} In combat"),
            _ if class == MarkerClass::Boss
                && self.progress.is_some_and(|progress| !progress.unlocked) =>
            {
                detail.push_str(" \u{00B7} Sealed");
            }
            _ => {}
        }
        (title, detail)
    }

    /// Draws the full-screen map, if it is open.
    pub fn push_map_screen(
        &self,
        list: &mut UiDrawList,
        markers: &MapMarkers,
        viewport: (u32, u32),
        _theme: &MapTheme,
    ) {
        if !self.open {
            return;
        }
        let Some(info) = &self.info else {
            return;
        };
        let Some(projection) = self.canvas_projection(viewport) else {
            return;
        };
        let s = self.style;
        let layout = self.screen_layout(viewport);
        let appear = ease_out(self.appear);

        let panel = layout.panel;
        // The scrim around the window, not under it: the window is opaque, and a full-screen
        // translucent layer beneath it would be a whole screen of blending nobody sees.
        let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
        let scrim = color::alpha(color::SCRIM, appear);
        list.rect(UiRect::new(0.0, 0.0, vw, panel.y), scrim);
        list.rect(
            UiRect::new(0.0, panel.bottom(), vw, vh - panel.bottom()),
            scrim,
        );
        list.rect(UiRect::new(0.0, panel.y, panel.x, panel.height), scrim);
        list.rect(
            UiRect::new(panel.right(), panel.y, vw - panel.right(), panel.height),
            scrim,
        );
        // The window's rounded corners show a sliver of the game; the scrim covers it.
        let corner = s.px(radius::WINDOW);
        for (x, y) in [
            (panel.x, panel.y),
            (panel.right() - corner, panel.y),
            (panel.x, panel.bottom() - corner),
            (panel.right() - corner, panel.bottom() - corner),
        ] {
            list.rect(UiRect::new(x, y, corner, corner), scrim);
        }
        let window_radius = s.px(radius::WINDOW);
        list.shadow(
            panel,
            window_radius,
            s.px(48.0),
            color::alpha(color::SHADOW, appear),
        );
        // The window's surface only where it is seen: around and between the canvas and the
        // journal, which are opaque. The same gradient in every piece — each is the whole
        // window's panel clipped to its region — so the pieces meet invisibly; the canvas's and
        // the journal's rounded corners get a square of it each.
        let (canvas, journal) = (layout.canvas, layout.sidebar);
        let body_top = canvas.y.min(journal.y);
        let body_bottom = canvas.bottom().max(journal.bottom());
        let inner = s.px(radius::CARD);
        let mut regions = vec![
            UiRect::new(panel.x, panel.y, panel.width, body_top - panel.y),
            UiRect::new(
                panel.x,
                body_bottom,
                panel.width,
                panel.bottom() - body_bottom,
            ),
            UiRect::new(
                panel.x,
                body_top,
                canvas.x - panel.x,
                body_bottom - body_top,
            ),
            UiRect::new(
                canvas.right(),
                body_top,
                journal.x - canvas.right(),
                body_bottom - body_top,
            ),
            UiRect::new(
                journal.right(),
                body_top,
                panel.right() - journal.right(),
                body_bottom - body_top,
            ),
        ];
        for rect in [canvas, journal] {
            regions.extend([
                UiRect::new(rect.x, rect.y, inner, inner),
                UiRect::new(rect.right() - inner, rect.y, inner, inner),
                UiRect::new(rect.x, rect.bottom() - inner, inner, inner),
                UiRect::new(rect.right() - inner, rect.bottom() - inner, inner, inner),
            ]);
        }
        for region in regions {
            list.clip(UiClip::rect(region));
            list.panel(
                panel,
                window_radius,
                color::SURFACE_0_TOP,
                color::SURFACE_0_BOTTOM,
            );
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(panel, window_radius, s.px(1.0), color::BORDER_STRONG);

        self.push_header(list, &layout, info);
        self.push_canvas(list, &layout, &projection, info, markers);
        self.push_journal(list, &layout, info);

        // Tooltips last, so they are on top.
        match self.hovered {
            Some(Hover::Marker(index)) if self.hover_time >= motion::TOOLTIP_DELAY * 0.5 => {
                if let Some(encounter) = info.encounters.get(index) {
                    let (title, detail) = self.marker_tooltip(index, encounter);
                    push_tooltip(list, self.pointer, &title, &detail, viewport, s);
                }
            }
            Some(Hover::Control(control)) if self.hover_time >= motion::TOOLTIP_DELAY => {
                let (title, detail) = control.tooltip();
                push_tooltip(list, self.pointer, title, detail, viewport, s);
            }
            _ => {}
        }
    }

    /// The title bar: emblem, name, completion, close.
    fn push_header(&self, list: &mut UiDrawList, layout: &MapScreenLayout, info: &DungeonMapInfo) {
        let s = self.style;
        let header = layout.header;
        let pad = s.px(space::XL);
        let emblem = s.px(44.0);
        let mid = header.y + header.height * 0.5;
        list.icon(
            UiRect::new(header.x + pad, mid - emblem * 0.5, emblem, emblem),
            UiIcon::EmblemCrypt,
            [1.0; 4],
        );
        let text_x = header.x + pad + emblem + s.px(space::LG);
        let title_top = mid - (s.line(type_scale::TITLE) + s.line(type_scale::META)) * 0.5;
        list.label(
            (text_x, title_top),
            &title_case(&info.name),
            color::TEXT_TITLE,
            type_scale::TITLE.font,
            s.size(type_scale::TITLE),
            TextAlign::Left,
        );
        let (packs, monsters, bosses) = info.totals();
        let subtitle = match self.selected_encounter() {
            Some(encounter) => format!(
                "Dungeon map \u{00B7} Inspecting {}",
                encounter
                    .boss()
                    .map_or_else(|| encounter.title.clone(), MapMember::name)
            ),
            None => format!(
                "Dungeon map \u{00B7} {packs} packs \u{00B7} {monsters} enemies \u{00B7} {bosses} boss{}",
                if bosses == 1 { "" } else { "es" }
            ),
        };
        let subtitle = s.fit(
            type_scale::META,
            &subtitle,
            layout.progress.x - text_x - s.px(space::XL),
        );
        list.label(
            (text_x, title_top + s.line(type_scale::TITLE)),
            &subtitle,
            color::TEXT_MUTED,
            type_scale::META.font,
            s.size(type_scale::META),
            TextAlign::Left,
        );

        // Completion, bound to the run.
        if let Some(progress) = self.progress {
            let rect = layout.progress;
            let label_top = rect.y;
            list.label(
                (rect.x, label_top),
                "Dungeon completion",
                color::TEXT_SECONDARY,
                type_scale::META.font,
                s.size(type_scale::META),
                TextAlign::Left,
            );
            list.label(
                (rect.right(), label_top),
                &format!("{}%", progress.percent),
                color::TEXT_PRIMARY,
                UiFont::SemiBold,
                s.size(type_scale::UI),
                TextAlign::Right,
            );
            let bar = UiRect::new(
                rect.x,
                label_top + s.line(type_scale::UI) + s.px(2.0),
                rect.width,
                s.px(6.0),
            );
            list.panel(
                bar,
                bar.height * 0.5,
                [1.0, 1.0, 1.0, 0.08],
                [1.0, 1.0, 1.0, 0.08],
            );
            let fill = UiRect::new(
                bar.x,
                bar.y,
                (bar.width * progress.fraction.clamp(0.0, 1.0)).max(if progress.fraction > 0.0 {
                    bar.height
                } else {
                    0.0
                }),
                bar.height,
            );
            list.panel(fill, bar.height * 0.5, color::BRONZE_BRIGHT, color::BRONZE);
            let (icon, text, tint) = if progress.completed {
                (UiIcon::Check, "Dungeon complete", color::VERDANT)
            } else if progress.unlocked {
                (UiIcon::Check, "Boss unsealed", color::VERDANT)
            } else {
                (UiIcon::Lock, "Boss sealed until 100%", color::BRONZE_BRIGHT)
            };
            let status_top = bar.bottom() + s.px(6.0);
            let glyph = s.px(13.0);
            list.icon(
                UiRect::new(rect.x, status_top + s.px(1.0), glyph, glyph),
                icon,
                tint,
            );
            list.label(
                (rect.x + glyph + s.px(6.0), status_top - s.px(1.0)),
                text,
                color::TEXT_MUTED,
                type_scale::META.font,
                s.size(type_scale::META),
                TextAlign::Left,
            );
        }

        // Close.
        let close = layout.close;
        let hovered = self.hovered == Some(Hover::Control(MapControl::Close));
        list.panel(
            close,
            s.px(radius::CONTROL),
            if hovered {
                color::SURFACE_3
            } else {
                color::SURFACE_1
            },
            if hovered {
                color::SURFACE_3
            } else {
                color::SURFACE_1
            },
        );
        list.panel_stroke(close, s.px(radius::CONTROL), s.px(1.0), color::BORDER);
        let glyph = s.px(16.0);
        list.icon(
            UiRect::new(
                close.x + (close.width - glyph) * 0.5,
                close.y + (close.height - glyph) * 0.5,
                glyph,
                glyph,
            ),
            UiIcon::ControlClose,
            if hovered {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_SECONDARY
            },
        );

        // A hairline under the header.
        list.rect(
            UiRect::new(
                header.x + pad,
                header.bottom(),
                header.width - pad * 2.0,
                s.px(1.0),
            ),
            color::BORDER,
        );
    }

    /// The map: picture, encounter insignias, the party, and the canvas's own controls.
    fn push_canvas(
        &self,
        list: &mut UiDrawList,
        layout: &MapScreenLayout,
        projection: &CanvasProjection,
        info: &DungeonMapInfo,
        markers: &MapMarkers,
    ) {
        let s = self.style;
        let canvas = layout.canvas;
        let card_radius = s.px(radius::CARD);
        list.panel(canvas, card_radius, color::MAP_GROUND, color::MAP_GROUND);
        list.clip(UiClip::rect(canvas.inset(s.px(1.0))));
        if info.has_picture {
            let (origin, du, dv) = projection.picture_mapping(&info.frame);
            list.map_image(canvas, origin, du, dv, [1.0; 4]);
        }

        // Soft glows under the boss and the selection, then the insignias.
        for (index, encounter) in info.encounters.iter().enumerate() {
            if MarkerClass::of(encounter) == MarkerClass::Boss
                && self.status(index) != EncounterStatus::Cleared
            {
                let at = projection.to_screen(encounter.position);
                let glow = self.marker_size(MarkerClass::Boss) * 1.3;
                list.glow(
                    UiRect::new(at.0 - glow, at.1 - glow, glow * 2.0, glow * 2.0),
                    glow,
                    glow,
                    color::alpha(color::CRIMSON, 0.28),
                );
            }
        }
        let mut order: Vec<usize> = (0..info.encounters.len()).collect();
        // The selected and hovered markers are drawn last, over their neighbours.
        order.sort_by_key(|index| {
            (
                Some(*index) == self.selected,
                self.emphasis.get(*index).copied().unwrap_or(0.0) > 0.5,
            )
        });
        for index in order {
            let encounter = &info.encounters[index];
            self.push_marker(list, projection, index, encounter);
        }
        for member in &markers.party {
            let at = projection.to_screen(member.position);
            push_unit(
                list,
                at,
                member.facing.map(|yaw| -yaw),
                s.px(18.0),
                color::VERDANT,
            );
        }
        if let Some(player) = markers.player {
            let at = projection.to_screen(player.position);
            let glow = s.px(14.0);
            list.glow(
                UiRect::new(at.0 - glow, at.1 - glow, glow * 2.0, glow * 2.0),
                glow,
                s.px(10.0),
                color::alpha(color::BRONZE_BRIGHT, 0.4),
            );
            push_unit(
                list,
                at,
                Some(-player.facing.unwrap_or(markers.view_yaw)),
                s.px(24.0),
                [1.0; 4],
            );
        }

        // Compass.
        let compass = s.px(56.0);
        list.icon(
            UiRect::new(
                canvas.right() - compass - s.px(space::LG),
                canvas.y + s.px(space::LG),
                compass,
                compass,
            ),
            UiIcon::MapCompass,
            [1.0; 4],
        );

        // Controls: one plate, four buttons.
        let plate = layout.controls;
        list.shadow(
            plate,
            s.px(radius::CARD),
            s.px(14.0),
            color::alpha(color::SHADOW, 0.7),
        );
        list.panel(
            plate,
            s.px(radius::CARD),
            color::alpha(color::SURFACE_1, 0.96),
            color::alpha(color::SURFACE_0_BOTTOM, 0.96),
        );
        list.panel_stroke(plate, s.px(radius::CARD), s.px(1.0), color::BORDER);
        for (control, icon) in [
            (MapControl::ZoomIn, UiIcon::ControlZoomIn),
            (MapControl::ZoomOut, UiIcon::ControlZoomOut),
            (MapControl::Fit, UiIcon::ControlFit),
            (MapControl::Recenter, UiIcon::ControlRecenter),
        ] {
            let rect = Self::control_rect(layout, control);
            let enabled = self.control_enabled(control);
            let hovered = self.hovered == Some(Hover::Control(control));
            if hovered && enabled {
                list.panel(
                    rect,
                    s.px(radius::CONTROL),
                    color::SURFACE_3,
                    color::SURFACE_3,
                );
            }
            let glyph = s.px(18.0);
            let tint = if !enabled {
                color::alpha(color::TEXT_MUTED, 0.45)
            } else if hovered {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_SECONDARY
            };
            list.icon(
                UiRect::new(
                    rect.x + (rect.width - glyph) * 0.5,
                    rect.y + (rect.height - glyph) * 0.5,
                    glyph,
                    glyph,
                ),
                icon,
                tint,
            );
        }

        // The hint strip.
        let help = layout.help;
        list.shadow(
            help,
            help.height * 0.5,
            s.px(12.0),
            color::alpha(color::SHADOW, 0.6),
        );
        list.panel(
            help,
            help.height * 0.5,
            color::alpha(color::SURFACE_1, 0.94),
            color::alpha(color::SURFACE_0_BOTTOM, 0.94),
        );
        list.panel_stroke(help, help.height * 0.5, s.px(1.0), color::BORDER);
        let mut x = help.x + s.px(space::LG);
        let mid = help.y + help.height * 0.5;
        let glyph = s.px(16.0);
        for (icon, text) in [
            (Some(UiIcon::MouseDrag), "Pan"),
            (Some(UiIcon::MouseWheel), "Zoom"),
            (Some(UiIcon::MouseClick), "Inspect"),
            (None, "Close"),
        ] {
            match icon {
                Some(icon) => {
                    list.icon(
                        UiRect::new(x, mid - glyph * 0.5, glyph, glyph),
                        icon,
                        color::TEXT_SECONDARY,
                    );
                    x += glyph + s.px(6.0);
                }
                None => {
                    // A keycap.
                    let cap_width = s.width(type_scale::LABEL, "Esc") + s.px(12.0);
                    let cap = UiRect::new(x, mid - s.px(10.0), cap_width, s.px(20.0));
                    list.panel(cap, s.px(radius::CHIP), color::SURFACE_3, color::SURFACE_2);
                    list.panel_stroke(cap, s.px(radius::CHIP), s.px(1.0), color::BORDER);
                    list.label(
                        (
                            cap.x + cap.width * 0.5,
                            s.centred_top(type_scale::LABEL, mid),
                        ),
                        "Esc",
                        color::TEXT_SECONDARY,
                        type_scale::LABEL.font,
                        s.size(type_scale::LABEL),
                        TextAlign::Center,
                    );
                    x += cap_width + s.px(6.0);
                }
            }
            if x + s.width(type_scale::UI, text) > help.right() - s.px(space::SM) {
                break;
            }
            list.label(
                (x, s.centred_top(type_scale::UI, mid)),
                text,
                color::TEXT_SECONDARY,
                type_scale::UI.font,
                s.size(type_scale::UI),
                TextAlign::Left,
            );
            x += s.width(type_scale::UI, text) + s.px(space::LG);
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(canvas, card_radius, s.px(1.0), color::BORDER);
    }

    /// One encounter insignia, with its hover, selection and status treatments.
    fn push_marker(
        &self,
        list: &mut UiDrawList,
        projection: &CanvasProjection,
        index: usize,
        encounter: &MapEncounterView,
    ) {
        let s = self.style;
        let at = projection.to_screen(encounter.position);
        let class = MarkerClass::of(encounter);
        let emphasis = ease_out(self.emphasis.get(index).copied().unwrap_or(0.0));
        let selected = self.selected == Some(index);
        let status = self.status(index);
        let base = self.marker_size(class);
        let size = base * (1.0 + 0.12 * emphasis + if selected { 0.08 } else { 0.0 });

        // Selection: a bronze ring and a soft halo; hover: a faint halo.
        if selected {
            let halo = size * 0.95;
            list.glow(
                UiRect::new(at.0 - halo, at.1 - halo, halo * 2.0, halo * 2.0),
                halo,
                halo * 0.6,
                color::alpha(color::BRONZE_BRIGHT, 0.35),
            );
            let ring = size * 0.74;
            list.panel_stroke(
                UiRect::new(at.0 - ring, at.1 - ring, ring * 2.0, ring * 2.0),
                ring,
                s.px(2.5),
                color::BRONZE_BRIGHT,
            );
        } else if emphasis > 0.01 {
            let halo = size * 0.8;
            list.glow(
                UiRect::new(at.0 - halo, at.1 - halo, halo * 2.0, halo * 2.0),
                halo,
                halo * 0.6,
                color::alpha(color::TEXT_PRIMARY, 0.18 * emphasis),
            );
        }
        if status == EncounterStatus::Engaged {
            let ring = size * 0.7;
            list.panel_stroke(
                UiRect::new(at.0 - ring, at.1 - ring, ring * 2.0, ring * 2.0),
                ring,
                s.px(2.0),
                color::CRIMSON,
            );
        }
        let tint = if status == EncounterStatus::Cleared {
            [0.55, 0.55, 0.56, 0.75]
        } else {
            [1.0; 4]
        };
        list.icon(
            UiRect::new(at.0 - size * 0.5, at.1 - size * 0.5, size, size),
            class.icon(),
            tint,
        );

        // A count badge for packs of more than one, a check once cleared, a lock while sealed.
        let badge = s.px(9.0);
        let corner = (at.0 + size * 0.36, at.1 - size * 0.36);
        let badge_rect = UiRect::new(corner.0 - badge, corner.1 - badge, badge * 2.0, badge * 2.0);
        if status == EncounterStatus::Cleared {
            list.panel(
                badge_rect,
                badge,
                color::VERDANT,
                color::alpha(color::VERDANT, 0.85),
            );
            let glyph = badge * 1.2;
            list.icon(
                UiRect::new(corner.0 - glyph * 0.5, corner.1 - glyph * 0.5, glyph, glyph),
                UiIcon::Check,
                [0.05, 0.1, 0.06, 1.0],
            );
        } else if class == MarkerClass::Boss
            && self.progress.is_some_and(|progress| !progress.unlocked)
        {
            list.panel(badge_rect, badge, color::SURFACE_1, color::SURFACE_0_BOTTOM);
            list.panel_stroke(badge_rect, badge, s.px(1.0), color::BRONZE);
            let glyph = badge * 1.1;
            list.icon(
                UiRect::new(corner.0 - glyph * 0.5, corner.1 - glyph * 0.5, glyph, glyph),
                UiIcon::Lock,
                color::BRONZE_BRIGHT,
            );
        } else if encounter.monster_count() > 1 && class != MarkerClass::Boss {
            list.panel(badge_rect, badge, color::SURFACE_1, color::SURFACE_0_BOTTOM);
            list.panel_stroke(badge_rect, badge, s.px(1.0), color::BRONZE);
            list.label(
                (corner.0, s.centred_top(type_scale::BADGE, corner.1)),
                &encounter.monster_count().to_string(),
                color::TEXT_PRIMARY,
                type_scale::BADGE.font,
                s.size(type_scale::BADGE),
                TextAlign::Center,
            );
        }
    }

    /// The encounter journal: fixed header, then the scrolling body.
    fn push_journal(
        &self,
        list: &mut UiDrawList,
        layout: &MapScreenLayout,
        _info: &DungeonMapInfo,
    ) {
        let s = self.style;
        let sidebar = layout.sidebar;
        let card_radius = s.px(radius::CARD);
        // Opaque: the window's surface is not drawn beneath the journal.
        list.panel(sidebar, card_radius, color::SURFACE_1, color::SURFACE_1);
        list.panel_stroke(sidebar, card_radius, s.px(1.0), color::BORDER);
        let pad = s.px(space::XL);
        let x = sidebar.x + pad;
        let inner = sidebar.width - pad * 2.0;
        let reveal = ease_out(self.reveal);
        let fade = |c: UiColor| color::alpha(c, reveal);

        // The fixed header.
        let head = &self.head;
        let head_rect = layout.sidebar_head;
        let mut y = head_rect.y + pad;
        let icon_size = s.px(52.0);
        if let Some(icon) = head.icon {
            list.icon(
                UiRect::new(x, y, icon_size, icon_size),
                icon,
                fade([1.0; 4]),
            );
        }
        let text_x = x + icon_size + s.px(space::MD);
        let mut ty = y;
        for line in &head.title {
            list.label(
                (text_x, ty),
                line,
                fade(color::TEXT_TITLE),
                type_scale::ENCOUNTER.font,
                s.size(type_scale::ENCOUNTER),
                TextAlign::Left,
            );
            ty += s.line(type_scale::ENCOUNTER);
        }
        list.label(
            (text_x, ty),
            &s.fit(
                type_scale::UI,
                &head.subtitle,
                inner - icon_size - s.px(space::MD),
            ),
            fade(color::TEXT_SECONDARY),
            type_scale::UI.font,
            s.size(type_scale::UI),
            TextAlign::Left,
        );
        ty += s.line(type_scale::UI);
        if let Some((text, tint)) = &head.badge {
            let badge_width = s.width(type_scale::LABEL, &text.to_uppercase()) + s.px(space::LG);
            let badge = UiRect::new(
                sidebar.right() - pad - badge_width,
                y + s.px(2.0),
                badge_width,
                s.px(22.0),
            );
            list.panel(
                badge,
                s.px(radius::CHIP),
                color::alpha(*tint, 0.16 * reveal),
                color::alpha(*tint, 0.16 * reveal),
            );
            list.panel_stroke(
                badge,
                s.px(radius::CHIP),
                s.px(1.0),
                color::alpha(*tint, 0.55 * reveal),
            );
            list.label(
                (
                    badge.x + badge.width * 0.5,
                    s.centred_top(type_scale::LABEL, badge.y + badge.height * 0.5),
                ),
                &text.to_uppercase(),
                fade(*tint),
                type_scale::LABEL.font,
                s.size(type_scale::LABEL),
                TextAlign::Center,
            );
        }
        y = ty.max(y + icon_size) + s.px(space::MD);
        for chip in &head.chips {
            push_chip(
                list,
                s,
                UiRect::new(x + chip.x, y + chip.y, chip.width, s.px(30.0)),
                chip,
                reveal,
            );
        }
        list.rect(
            UiRect::new(
                sidebar.x + pad,
                head_rect.bottom() - s.px(1.0),
                inner,
                s.px(1.0),
            ),
            color::BORDER,
        );

        // The body, clipped and scrolled; it slides in a little when the selection changes.
        let body = layout.sidebar_body;
        list.clip(UiClip::rect(UiRect::new(
            sidebar.x,
            body.y,
            sidebar.width,
            body.height - s.px(2.0),
        )));
        let origin_y = body.y + s.px(space::LG) - self.scroll + (1.0 - reveal) * s.px(10.0);
        let pointer = self.pointer;
        let pointer_in_body = body.contains(pointer.0, pointer.1);
        for card in &self.cards {
            let top = origin_y + card.top;
            let rect = UiRect::new(x, top, inner, card.bottom - card.top);
            if rect.bottom() < body.y || rect.y > body.bottom() {
                continue;
            }
            match card.tone {
                CardTone::Raised => {
                    list.panel(
                        rect,
                        card_radius,
                        fade(color::SURFACE_2),
                        fade(color::alpha(color::SURFACE_2, 0.9)),
                    );
                    list.panel_stroke(rect, card_radius, s.px(1.0), fade(color::BORDER));
                }
                CardTone::Inset => {
                    list.panel(
                        rect,
                        card_radius,
                        fade([0.0, 0.0, 0.0, 0.18]),
                        fade([0.0, 0.0, 0.0, 0.18]),
                    );
                    list.panel_stroke(rect, card_radius, s.px(1.0), fade(color::BORDER));
                }
            }
        }
        for block in &self.blocks {
            let top = origin_y + block.y;
            if top + block.height < body.y || top > body.bottom() {
                continue;
            }
            let row = UiRect::new(x, top, inner, block.height);
            let hovered =
                pointer_in_body && block.action.is_some() && row.contains(pointer.0, pointer.1);
            self.push_block(list, block, row, hovered, reveal);
        }
        list.clip(UiClip::NONE);

        // Scroll bar.
        let content = self.content_height();
        let visible = body.height - s.px(space::LG) * 2.0;
        if content > visible {
            let track = UiRect::new(
                sidebar.right() - s.px(8.0),
                body.y + s.px(space::SM),
                s.px(3.0),
                body.height - s.px(space::LG),
            );
            list.panel(
                track,
                track.width * 0.5,
                [1.0, 1.0, 1.0, 0.05],
                [1.0, 1.0, 1.0, 0.05],
            );
            let thumb_height = (visible / content * track.height).max(s.px(32.0));
            let travel = track.height - thumb_height;
            let limit = (content - visible).max(1.0);
            let thumb = UiRect::new(
                track.x,
                track.y + travel * (self.scroll / limit),
                track.width,
                thumb_height,
            );
            list.panel(thumb, track.width * 0.5, color::BRONZE, color::BRONZE_DIM);
        }
    }

    /// One journal row.
    fn push_block(
        &self,
        list: &mut UiDrawList,
        block: &Block,
        row: UiRect,
        hovered: bool,
        reveal: f32,
    ) {
        let s = self.style;
        let fade = |c: UiColor| color::alpha(c, reveal);
        let label_at =
            |list: &mut UiDrawList, x: f32, y: f32, text: &str, style: TextStyle, c: UiColor| {
                list.label(
                    (x, y),
                    text,
                    fade(c),
                    style.font,
                    s.size(style),
                    TextAlign::Left,
                );
            };
        match &block.kind {
            BlockKind::Label { text, accent } => {
                let mid = row.y + row.height * 0.55;
                list.rect(
                    UiRect::new(row.x, mid - s.px(5.0), s.px(3.0), s.px(10.0)),
                    fade(*accent),
                );
                label_at(
                    list,
                    row.x + s.px(space::MD),
                    s.centred_top(type_scale::LABEL, mid),
                    text,
                    type_scale::LABEL,
                    color::TEXT_MUTED,
                );
            }
            BlockKind::Lines {
                lines,
                style,
                color: tint,
                indent,
            } => {
                let line = s.line(*style);
                for (index, text) in lines.iter().enumerate() {
                    label_at(
                        list,
                        row.x + indent,
                        row.y + line * index as f32,
                        text,
                        *style,
                        *tint,
                    );
                }
            }
            BlockKind::Monster {
                tint,
                name,
                meta,
                count,
                open,
            } => {
                if hovered {
                    list.panel(
                        row.inset(s.px(1.0)),
                        s.px(radius::CARD),
                        fade(color::SURFACE_3),
                        fade(color::SURFACE_3),
                    );
                }
                let pad = s.px(space::LG);
                let portrait = s.px(44.0);
                let frame = UiRect::new(
                    row.x + pad,
                    row.y + (row.height - portrait) * 0.5,
                    portrait,
                    portrait,
                );
                list.panel(
                    frame,
                    s.px(radius::CONTROL),
                    fade(color::SURFACE_0_TOP),
                    fade(color::SURFACE_0_BOTTOM),
                );
                list.icon(frame.inset(s.px(2.0)), UiIcon::Monster, fade(*tint));
                list.panel_stroke(
                    frame,
                    s.px(radius::CONTROL),
                    s.px(1.0),
                    fade(color::alpha(*tint, 0.6)),
                );
                let text_x = frame.right() + s.px(space::MD);
                let right = row.right() - pad - s.px(20.0);
                let name_top = row.y + row.height * 0.5
                    - (s.line(type_scale::NAME) + s.line(type_scale::META)) * 0.5;
                let count_text = format!("\u{00D7}{count}");
                let count_width = if *count > 1 {
                    s.width(type_scale::HEADING, &count_text) + s.px(space::MD)
                } else {
                    0.0
                };
                label_at(
                    list,
                    text_x,
                    name_top,
                    &s.fit(type_scale::NAME, name, right - text_x - count_width),
                    type_scale::NAME,
                    color::TEXT_PRIMARY,
                );
                label_at(
                    list,
                    text_x,
                    name_top + s.line(type_scale::NAME),
                    &s.fit(type_scale::META, meta, right - text_x),
                    type_scale::META,
                    color::TEXT_MUTED,
                );
                if *count > 1 {
                    list.label(
                        (right, name_top + s.px(1.0)),
                        &count_text,
                        fade(color::BRONZE_BRIGHT),
                        type_scale::HEADING.font,
                        s.size(type_scale::HEADING),
                        TextAlign::Right,
                    );
                }
                let chevron = s.px(14.0);
                list.icon_rotated(
                    (row.right() - pad - chevron * 0.3, row.y + row.height * 0.5),
                    chevron,
                    UiIcon::ControlChevron,
                    if *open {
                        std::f32::consts::FRAC_PI_2
                    } else {
                        0.0
                    },
                    fade(color::TEXT_MUTED),
                );
            }
            BlockKind::Ability {
                icon,
                accent,
                name,
                meta,
                summary,
                tags,
                open,
            } => {
                let pad = s.px(space::LG);
                let inner = row.inset(s.px(4.0));
                if hovered || *open {
                    list.panel(
                        UiRect::new(
                            row.x + s.px(space::SM),
                            row.y + s.px(2.0),
                            row.width - s.px(space::LG),
                            row.height - s.px(4.0),
                        ),
                        s.px(radius::CONTROL),
                        fade(if hovered {
                            color::SURFACE_3
                        } else {
                            color::alpha(color::SURFACE_3, 0.6)
                        }),
                        fade(if hovered {
                            color::SURFACE_3
                        } else {
                            color::alpha(color::SURFACE_3, 0.6)
                        }),
                    );
                }
                let size = s.px(36.0);
                let frame = UiRect::new(row.x + pad, row.y + s.px(space::MD), size, size);
                list.panel(
                    frame,
                    s.px(radius::CONTROL),
                    fade(color::alpha(*accent, 0.25)),
                    fade(color::alpha(*accent, 0.1)),
                );
                list.icon(frame.inset(s.px(2.0)), *icon, fade([1.0; 4]));
                list.panel_stroke(
                    frame,
                    s.px(radius::CONTROL),
                    s.px(1.0),
                    fade(color::alpha(*accent, 0.7)),
                );
                let text_x = frame.right() + s.px(space::MD);
                let tag_size = s.px(16.0);
                let tags_width = tags.len() as f32 * (tag_size + s.px(6.0));
                let right = inner.right() - pad - s.px(16.0) - tags_width;
                label_at(
                    list,
                    text_x,
                    row.y + s.px(10.0),
                    &s.fit(type_scale::ABILITY, name, right - text_x),
                    type_scale::ABILITY,
                    color::TEXT_PRIMARY,
                );
                label_at(
                    list,
                    text_x,
                    row.y + s.px(10.0) + s.line(type_scale::ABILITY),
                    &s.fit(type_scale::META, meta, inner.right() - text_x - pad),
                    type_scale::META,
                    color::TEXT_MUTED,
                );
                if !summary.is_empty() {
                    label_at(
                        list,
                        text_x,
                        row.y + s.px(10.0) + s.line(type_scale::ABILITY) + s.line(type_scale::META),
                        summary,
                        type_scale::META,
                        color::TEXT_SECONDARY,
                    );
                }
                let mut tx = inner.right() - pad - s.px(16.0) - tags_width;
                for tag in tags {
                    list.icon(
                        UiRect::new(tx, row.y + s.px(13.0), tag_size, tag_size),
                        tag.icon(),
                        fade(color::alpha(tag.color(), 0.9)),
                    );
                    tx += tag_size + s.px(6.0);
                }
                let chevron = s.px(12.0);
                list.icon_rotated(
                    (inner.right() - pad + s.px(2.0), row.y + s.px(21.0)),
                    chevron,
                    UiIcon::ControlChevron,
                    if *open {
                        std::f32::consts::FRAC_PI_2
                    } else {
                        0.0
                    },
                    fade(color::TEXT_MUTED),
                );
            }
            BlockKind::Bullets { lines, dot } => {
                let line = s.line(type_scale::BODY);
                let text_x = row.x + s.px(space::LG) + s.px(36.0) + s.px(space::MD);
                for (index, (text, continuation)) in lines.iter().enumerate() {
                    let y = row.y + line * index as f32;
                    if !continuation {
                        let r = s.px(2.5);
                        list.disc(
                            (text_x + r, y + line * 0.5),
                            r,
                            fade(color::alpha(*dot, 0.9)),
                        );
                    }
                    label_at(
                        list,
                        text_x + s.px(space::LG),
                        y,
                        text,
                        type_scale::BODY,
                        color::TEXT_SECONDARY,
                    );
                }
            }
            BlockKind::Counterplay { lines } => {
                let x = row.x + s.px(space::LG) + s.px(36.0) + s.px(space::MD);
                let rect = UiRect::new(x, row.y, row.right() - x - s.px(space::LG), row.height);
                list.panel(
                    rect,
                    s.px(radius::CONTROL),
                    fade(color::alpha(color::TEAL, 0.10)),
                    fade(color::alpha(color::TEAL, 0.06)),
                );
                list.panel(
                    UiRect::new(rect.x, rect.y, s.px(3.0), rect.height),
                    s.px(1.5),
                    fade(color::TEAL),
                    fade(color::alpha(color::TEAL, 0.6)),
                );
                let text_x = rect.x + s.px(space::MD) + s.px(3.0);
                let mut y = rect.y + s.px(space::MD);
                label_at(
                    list,
                    text_x,
                    y,
                    "COUNTERPLAY",
                    type_scale::LABEL,
                    color::TEAL,
                );
                y += s.line(type_scale::LABEL);
                for text in lines {
                    label_at(list, text_x, y, text, type_scale::BODY, color::TEXT_PRIMARY);
                    y += s.line(type_scale::BODY);
                }
            }
            BlockKind::Filters { tabs, active } => {
                for (filter, count, tab_x, width) in tabs {
                    let rect = UiRect::new(row.x + tab_x, row.y + s.px(2.0), *width, s.px(30.0));
                    let is_active = filter == active;
                    let hovered = rect.contains(self.pointer.0, self.pointer.1);
                    let fill = if is_active {
                        color::alpha(color::BRONZE, 0.22)
                    } else if hovered {
                        color::SURFACE_3
                    } else {
                        color::SURFACE_2
                    };
                    list.panel(rect, rect.height * 0.5, fade(fill), fade(fill));
                    list.panel_stroke(
                        rect,
                        rect.height * 0.5,
                        s.px(1.0),
                        fade(if is_active {
                            color::alpha(color::BRONZE_BRIGHT, 0.7)
                        } else {
                            color::BORDER
                        }),
                    );
                    let text = format!("{} {count}", filter.label());
                    list.label(
                        (
                            rect.x + rect.width * 0.5,
                            s.centred_top(type_scale::UI, rect.y + rect.height * 0.5),
                        ),
                        &text,
                        fade(if is_active {
                            color::BRONZE_BRIGHT
                        } else {
                            color::TEXT_SECONDARY
                        }),
                        type_scale::UI.font,
                        s.size(type_scale::UI),
                        TextAlign::Center,
                    );
                }
            }
            BlockKind::Encounter {
                class,
                name,
                detail,
                status,
            } => {
                if hovered {
                    list.panel(
                        row,
                        s.px(radius::CONTROL),
                        fade(color::SURFACE_3),
                        fade(color::SURFACE_3),
                    );
                }
                let size = s.px(30.0);
                let mid = row.y + row.height * 0.5;
                let tint = if *status == EncounterStatus::Cleared {
                    [0.6, 0.6, 0.6, 0.8]
                } else {
                    [1.0; 4]
                };
                list.icon(
                    UiRect::new(row.x + s.px(space::SM), mid - size * 0.5, size, size),
                    class.icon(),
                    fade(tint),
                );
                let text_x = row.x + s.px(space::SM) + size + s.px(space::MD);
                let top = mid - (s.line(type_scale::HEADING) + s.line(type_scale::META)) * 0.5;
                label_at(
                    list,
                    text_x,
                    top,
                    &s.fit(type_scale::HEADING, name, row.right() - text_x - s.px(90.0)),
                    type_scale::HEADING,
                    if *class == MarkerClass::Boss {
                        color::TEXT_TITLE
                    } else {
                        color::TEXT_PRIMARY
                    },
                );
                label_at(
                    list,
                    text_x,
                    top + s.line(type_scale::HEADING),
                    detail,
                    type_scale::META,
                    color::TEXT_MUTED,
                );
                let (text, tint) = match status {
                    EncounterStatus::Cleared => ("Cleared", color::VERDANT),
                    EncounterStatus::Engaged => ("In combat", color::CRIMSON),
                    _ => ("", color::TEXT_MUTED),
                };
                if !text.is_empty() {
                    list.label(
                        (
                            row.right() - s.px(space::SM),
                            s.centred_top(type_scale::META, mid),
                        ),
                        text,
                        fade(tint),
                        type_scale::META.font,
                        s.size(type_scale::META),
                        TextAlign::Right,
                    );
                }
            }
            BlockKind::Legend { icon, tint, label } => {
                let size = s.px(22.0);
                let mid = row.y + row.height * 0.5;
                list.icon(
                    UiRect::new(row.x + s.px(space::SM), mid - size * 0.5, size, size),
                    *icon,
                    fade(*tint),
                );
                label_at(
                    list,
                    row.x + s.px(space::SM) + size + s.px(space::MD),
                    s.centred_top(type_scale::UI, mid),
                    label,
                    type_scale::UI,
                    color::TEXT_SECONDARY,
                );
            }
            BlockKind::Empty { icon, lines } => {
                let size = s.px(40.0);
                let pad = s.px(space::LG);
                list.icon(
                    UiRect::new(row.x + pad, row.y + (row.height - size) * 0.5, size, size),
                    *icon,
                    fade(color::alpha(color::TEXT_MUTED, 0.8)),
                );
                let text_x = row.x + pad + size + s.px(space::LG);
                let line = s.line(type_scale::BODY);
                let top = row.y + (row.height - line * lines.len() as f32) * 0.5;
                for (index, text) in lines.iter().enumerate() {
                    label_at(
                        list,
                        text_x,
                        top + line * index as f32,
                        text,
                        type_scale::BODY,
                        color::TEXT_MUTED,
                    );
                }
            }
            BlockKind::Loot { line } => {
                let pad = s.px(space::MD);
                let size = s.px(48.0);
                let rarity = quality_color(line.quality);
                let frame = UiRect::new(row.x + pad, row.y + (row.height - size) * 0.5, size, size);
                list.panel(
                    frame,
                    s.px(radius::CONTROL),
                    fade(color::SURFACE_0_TOP),
                    fade(color::SURFACE_0_BOTTOM),
                );
                list.icon(frame.inset(s.px(2.0)), line.icon, fade([1.0; 4]));
                list.panel_stroke(frame, s.px(radius::CONTROL), s.px(1.5), fade(rarity));
                let text_x = frame.right() + s.px(space::MD);
                let mut y = row.y + s.px(space::MD);
                label_at(list, text_x, y, &line.name, type_scale::ABILITY, rarity);
                y += s.line(type_scale::ABILITY);
                label_at(
                    list,
                    text_x,
                    y,
                    &line.meta,
                    type_scale::META,
                    color::TEXT_MUTED,
                );
                y += s.line(type_scale::META);
                let tail = if line.stats.is_empty() {
                    line.chance.clone()
                } else {
                    format!("{} \u{00B7} {}", line.stats, line.chance)
                };
                label_at(
                    list,
                    text_x,
                    y,
                    &tail,
                    type_scale::META,
                    color::TEXT_SECONDARY,
                );
            }
            BlockKind::Progress => {
                let Some(progress) = self.progress else {
                    return;
                };
                let pad = s.px(space::LG);
                let x = row.x + pad;
                let width = row.width - pad * 2.0;
                list.label(
                    (x, row.y + pad - s.px(4.0)),
                    &format!("{}%", progress.percent),
                    fade(color::TEXT_TITLE),
                    type_scale::ENCOUNTER.font,
                    s.size(type_scale::ENCOUNTER),
                    TextAlign::Left,
                );
                let cleared = self
                    .statuses
                    .iter()
                    .filter(|status| **status == EncounterStatus::Cleared)
                    .count();
                let known = self
                    .statuses
                    .iter()
                    .filter(|status| **status != EncounterStatus::Unknown)
                    .count();
                list.label(
                    (row.right() - pad, row.y + pad),
                    &format!("{cleared} of {known} encounters cleared"),
                    fade(color::TEXT_SECONDARY),
                    type_scale::META.font,
                    s.size(type_scale::META),
                    TextAlign::Right,
                );
                let bar = UiRect::new(
                    x,
                    row.y + pad + s.line(type_scale::ENCOUNTER),
                    width,
                    s.px(6.0),
                );
                list.panel(
                    bar,
                    bar.height * 0.5,
                    fade([1.0, 1.0, 1.0, 0.08]),
                    fade([1.0, 1.0, 1.0, 0.08]),
                );
                let fill = UiRect::new(
                    bar.x,
                    bar.y,
                    bar.width * progress.fraction.clamp(0.0, 1.0),
                    bar.height,
                );
                list.panel(
                    fill,
                    bar.height * 0.5,
                    fade(color::BRONZE_BRIGHT),
                    fade(color::BRONZE),
                );
                let text = if progress.completed {
                    "The dungeon is complete."
                } else if progress.unlocked {
                    "The boss's seal is broken."
                } else {
                    "The boss is sealed until completion reaches 100%."
                };
                label_at(
                    list,
                    x,
                    bar.bottom() + s.px(space::SM),
                    text,
                    type_scale::META,
                    color::TEXT_MUTED,
                );
            }
            BlockKind::Chips { chips } => {
                let x = row.x + s.px(space::LG);
                for chip in chips {
                    push_chip(
                        list,
                        s,
                        UiRect::new(x + chip.x, row.y + chip.y, chip.width, s.px(30.0)),
                        chip,
                        reveal,
                    );
                }
            }
            BlockKind::Guests { title } => {
                let size = s.px(24.0);
                let x = row.x + s.px(space::LG);
                let mid = row.y + row.height * 0.5 + s.px(2.0);
                list.icon(
                    UiRect::new(x, mid - size * 0.5, size, size),
                    UiIcon::CrestElite,
                    fade([1.0; 4]),
                );
                label_at(
                    list,
                    x + size + s.px(space::MD),
                    s.centred_top(type_scale::HEADING, mid),
                    title,
                    type_scale::HEADING,
                    color::TEXT_PRIMARY,
                );
            }
            BlockKind::Gap => {}
        }
    }
}

/// "3 enemies".
fn enemies(count: u32) -> String {
    if count == 1 {
        "1 enemy".to_owned()
    } else {
        format!("{count} enemies")
    }
}

/// A design-table answer as a sentence: capitalized, with a full stop.
fn sentence(text: &str) -> String {
    // The tables write a dash as a hyphen for the bitmap font; the interface font has the
    // real thing.
    let dashed = text.trim().replace(" - ", " \u{2014} ");
    let trimmed = dashed.as_str();
    let mut out = String::with_capacity(trimmed.len() + 1);
    let mut characters = trimmed.chars();
    if let Some(first) = characters.next() {
        out.extend(first.to_uppercase());
        out.push_str(characters.as_str());
    }
    if !out.ends_with(['.', '!', '?', ')']) {
        out.push('.');
    }
    out
}

/// A member's portrait tint: its monster's look.
fn portrait_tint(member: &MapMember) -> UiColor {
    member.kind.map_or([0.7, 0.7, 0.7, 1.0], |kind| {
        let tint = kind.archetype().look.tint;
        [tint[0], tint[1], tint[2], 1.0]
    })
}

/// A quick ease-out curve for fades.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// A roster chip.
fn push_chip(list: &mut UiDrawList, s: MapStyle, rect: UiRect, chip: &Chip, reveal: f32) {
    list.panel(
        rect,
        s.px(radius::CHIP),
        color::alpha(color::SURFACE_3, reveal),
        color::alpha(color::SURFACE_2, reveal),
    );
    list.panel_stroke(
        rect,
        s.px(radius::CHIP),
        s.px(1.0),
        color::alpha(color::BORDER, reveal),
    );
    let portrait = rect.height - s.px(6.0);
    list.icon(
        UiRect::new(rect.x + s.px(3.0), rect.y + s.px(3.0), portrait, portrait),
        UiIcon::Monster,
        color::alpha(chip.tint, reveal),
    );
    let text_x = rect.x + portrait + s.px(space::SM) + s.px(3.0);
    let text = s.fit(
        type_scale::META,
        &chip.text,
        rect.right() - text_x - s.px(space::SM),
    );
    list.label(
        (
            text_x,
            s.centred_top(type_scale::META, rect.y + rect.height * 0.5),
        ),
        &text,
        color::alpha(color::TEXT_PRIMARY, reveal),
        type_scale::META.font,
        s.size(type_scale::META),
        TextAlign::Left,
    );
}

/// A character's marker: an arrow turned `rotation`, or a dot when the way they face is
/// unknown.
fn push_unit(
    list: &mut UiDrawList,
    at: (f32, f32),
    rotation: Option<f32>,
    size: f32,
    tint: UiColor,
) {
    match rotation {
        Some(rotation) => list.icon_rotated(at, size, UiIcon::MapPlayer, rotation, tint),
        None => {
            list.disc(at, size * 0.42, [0.0, 0.0, 0.0, 0.8]);
            list.disc(at, size * 0.32, tint);
        }
    }
}

/// A tooltip beside `pointer`, kept on screen.
fn push_tooltip(
    list: &mut UiDrawList,
    pointer: (f32, f32),
    title: &str,
    detail: &str,
    viewport: (u32, u32),
    s: MapStyle,
) {
    let pad = s.px(space::MD);
    let width = s
        .width(type_scale::HEADING, title)
        .max(s.width(type_scale::META, detail))
        + pad * 2.0;
    let height = s.line(type_scale::HEADING) + s.line(type_scale::META) + pad * 2.0 - s.px(2.0);
    let mut x = pointer.0 + s.px(18.0);
    let mut y = pointer.1 + s.px(14.0);
    if x + width > viewport.0 as f32 - s.px(8.0) {
        x = pointer.0 - s.px(12.0) - width;
    }
    if y + height > viewport.1 as f32 - s.px(8.0) {
        y = pointer.1 - height - s.px(10.0);
    }
    let rect = UiRect::new(x.max(s.px(8.0)), y.max(s.px(8.0)), width, height);
    list.shadow(
        rect,
        s.px(radius::CONTROL),
        s.px(14.0),
        color::alpha(color::SHADOW, 0.8),
    );
    list.panel(
        rect,
        s.px(radius::CONTROL),
        [0.10, 0.11, 0.14, 0.98],
        [0.07, 0.08, 0.10, 0.98],
    );
    list.panel_stroke(rect, s.px(radius::CONTROL), s.px(1.0), color::BORDER_STRONG);
    list.label(
        (rect.x + pad, rect.y + pad - s.px(2.0)),
        title,
        color::TEXT_PRIMARY,
        type_scale::HEADING.font,
        s.size(type_scale::HEADING),
        TextAlign::Left,
    );
    list.label(
        (
            rect.x + pad,
            rect.y + pad - s.px(2.0) + s.line(type_scale::HEADING),
        ),
        detail,
        color::TEXT_MUTED,
        type_scale::META.font,
        s.size(type_scale::META),
        TextAlign::Left,
    );
}
