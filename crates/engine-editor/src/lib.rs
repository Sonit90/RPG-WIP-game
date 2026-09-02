//! The level editor: placing, moving and deleting environment objects inside the running game.
//!
//! ```text
//!  F6 ──► Editing ───────────────► EditorOutcome
//!   ▲        │  left click object   select it, and never place anything
//!   │        │  left click ground   drop the selection
//!   │        │  N                   placing mode: left click puts one down, every time
//!   │        │  drag from browser   place that asset where it is dropped
//!   │        │  double click a row  place one in front of the camera
//!   │        │  drag an X/Y/Z arm   move the selection along that axis
//!   │        │  R / SHIFT+R         yaw, snapped to the rotation step
//!   │        │  G / SHIFT+G         grid step / rotation step
//!   │        │  T / SHIFT+T         scale
//!   │        │  ARROWS / SHIFT      nudge by the grid, in X and Z / in Y
//!   │        │  L                   nudge along world axes or the object's own
//!   │        │  0                   reset rotation and scale
//!   │        │  X                   delete
//!   │        │  C                   duplicate
//!   │        │  V                   pour a body of water where the cursor is
//!   │        │  M                   module snap: the cave grid, on and off
//!   │        │  P                   put a spawn point where the cursor is
//!   │        │  CTRL+click          add to the selection; CTRL+A takes everything
//!   │        │  Z                   undo
//!   │        │  TAB                 next asset in the browser
//!   │        │  F / F2              filter the browser / read assets/ again
//!   │        │  B                   sculpt the ground; TAB cycles the brush
//!   │        │  1 2 / 3 4           brush size / brush strength, while sculpting
//!   │        │  CTRL+S              write assets/scenes/<name>.json
//!   └────────┘  F6 / ESC            leave
//! ```
//!
//! # One gesture, one meaning
//!
//! Placing and selecting are different gestures, and neither can be mistaken for the other:
//!
//! 1. a drag on a gizmo arm transforms what is selected;
//! 2. a click on something in the world selects it — it never places anything;
//! 3. a drag from a browser row into the world places one there;
//! 4. a double click on a browser row places one in front of the camera;
//! 5. anything else is the camera's.
//!
//! `N` puts the editor in **placing mode**, where rule 2 is suspended: every left click in
//! the world puts an object down, on top of whatever is already there, and nothing selects or
//! moves. Laying out a row of pillars is a dozen clicks rather than a dozen drags out of the
//! browser, and an author doing that never wants a click to be interpreted as "you meant to
//! pick up the pillar you just placed". `N` again, or `ESC`, and the left button means select
//! once more.
//!
//! It used to be that a click on empty ground placed the browser's asset, which made one
//! button mean "place" or "select" depending on what happened to be under it — and a click
//! that missed the rock an author was aiming at left a second rock behind it. Placing is now
//! something an author does *from the browser*, deliberately, and the world's left button
//! only ever selects.
//!
//! # It edits data, and nothing else
//!
//! The editor's whole world is an [`EnvironmentScene`] — a `Vec` of placements. It never
//! touches a mesh, a device, a render entity or a collision world, and it cannot: this crate
//! depends on neither `engine-renderer` nor `engine-game`. What it returns is
//! [`EditorOutcome::SceneChanged`], and the application answers that by re-placing the scene
//! it already knows how to place.
//!
//! It does read a *directory*: [`browser::AssetBrowser`] lists what is under `assets/` so an
//! author can pick from it. That is names and extensions, not meshes — see that module.
//!
//! That is what makes the whole thing testable. Every verb below has a test that clicks,
//! drags or presses a key and asserts what the scene became, with no window open.
//!
//! # It is not in a player's build
//!
//! `engine-app` names this crate as an optional dependency behind its `editor` feature. A
//! shipping build compiles none of it — no asset browser, no fly camera, no scene writer.
//!
//! # Why the whole scene is the undo unit
//!
//! `Z` restores the previous `Vec<EnvironmentObject>` wholesale rather than inverting the last
//! verb. A scene is a few hundred 44-byte records, so a snapshot is a few kilobytes and a
//! memcpy; inverting verbs means a second implementation of every verb, written backwards,
//! which is where undo bugs live.

#![deny(missing_docs)]

pub mod browser;
pub mod camera;
pub mod gizmo;
pub mod pick;
pub mod ui;

use std::path::{Path, PathBuf};

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Mat4, Vec3};
use engine_core::transform::Transform;
use engine_environment::spawn::SpawnPoint;
use engine_environment::terrain::{BrushMode, Terrain};
use engine_environment::water::WaterBody;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene, scene_file};
use engine_presentation::ui::input::UiInput;
use engine_render_data::{Camera, RenderDataError, UiScale};

pub use camera::FlyCamera;

/// How many snapshots `Z` can walk back through.
const UNDO_DEPTH: usize = 64;

/// What `Z` puts back: everything an edit can change.
///
/// The placements, the ground *and* the water. It used to be the placements alone, which was
/// right while they were the only thing an author could change; a brush stroke is an edit like
/// any other and has to be undoable the same way, and so is pouring a pond. A terrain is 66 KB at the shipping size and a
/// snapshot is only taken when something changes, so sixty-four of them is a few megabytes in
/// the editor and nothing at all in a shipping build, which has none of this.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    objects: Vec<EnvironmentObject>,
    terrain: Option<Terrain>,
    water: Vec<WaterBody>,
    spawn_points: Vec<SpawnPoint>,
}

impl Snapshot {
    /// The scene as it is now.
    fn of(scene: &EnvironmentScene) -> Self {
        Self {
            objects: scene.objects.clone(),
            terrain: scene.terrain.clone(),
            water: scene.water.clone(),
            spawn_points: scene.spawn_points.clone(),
        }
    }

    /// Puts this snapshot back into `scene`.
    fn restore(self, scene: &mut EnvironmentScene) {
        scene.objects = self.objects;
        scene.terrain = self.terrain;
        scene.water = self.water;
        scene.spawn_points = self.spawn_points;
    }
}

/// The grid sizes `G` cycles through, in metres. Zero is off.
const GRID_STEPS: [f32; 5] = [1.0, 2.0, 4.0, 0.25, 0.0];

/// The rotation steps `SHIFT+G` cycles through, in degrees.
///
/// A blockout is built out of pieces that meet at right angles and at half-right angles, so
/// the coarse steps come first: an author reaches for 15° when they want it, and wants 90°
/// most of the time.
const YAW_STEPS: [f32; 5] = [90.0, 45.0, 22.5, 15.0, 5.0];

/// How much one press changes the brush's size or strength.
///
/// Multiplied one way and divided the other, so the two keys are inverses: an author who
/// presses "bigger" and then "smaller" gets the brush they started with, and one who scales by
/// 1.25 and then by 0.75 does not.
const BRUSH_SIZE_STEP: f32 = 1.25;

/// How much one press of `T` changes an object's size.
const SCALE_STEP: f32 = 1.1;

/// The grid the modular cave pieces are built on, in metres.
///
/// Read from the library that builds them rather than written down again here, so a change to
/// the module size moves the geometry and the grid an author aligns it on together.
const MODULE_GRID: f32 = engine_assets::procedural::starter::MODULE;

/// The rotation step module snap uses, in degrees.
///
/// A quarter turn, because a modular piece meets its neighbour on a face and there are four
/// of them. Anything finer produces a join with a gap in it.
const MODULE_YAW: f32 = 90.0;

/// Half-width and half-depth of the body `V` pours, in metres: an eight-metre square.
///
/// Big enough to see from the editor camera and small enough to be the thing an author grows
/// rather than the thing they immediately have to shrink.
const WATER_HALF_EXTENT: f32 = 4.0;

/// How deep the body `V` pours reaches below its surface, in metres.
const WATER_DEPTH: f32 = 2.0;

/// The smallest a body of water may be shrunk to, in metres.
///
/// A body with no extent is a volume nothing can be inside — refused by the scene loader —
/// so `SHIFT+T` stops here rather than producing a level that cannot be read back.
const WATER_MIN_HALF_EXTENT: f32 = 0.25;

/// What the application should do about the frame the editor just read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Nothing that the application has to answer for.
    Idle,
    /// The scene changed: re-place it.
    SceneChanged,
    /// The terrain changed: rebuild its mesh and its collision surface.
    ///
    /// Separate from [`EditorOutcome::SceneChanged`] because the answers are different sizes.
    /// Re-placing a scene respawns a few hundred render entities; rebuilding a terrain is one
    /// mesh upload. A brush stroke that reported "the scene changed" would pay the first cost
    /// for the second thing, on every frame of a drag.
    TerrainChanged,
    /// The water changed: rebuild its meshes.
    ///
    /// Separate from [`EditorOutcome::SceneChanged`] for the reason
    /// [`EditorOutcome::TerrainChanged`] is: re-placing a scene respawns a few hundred render
    /// entities, and resizing one quad is one upload. A drag that reported "the scene changed"
    /// would pay the first cost for the second thing on every frame of it.
    WaterChanged,
    /// The author asked to go back to playing.
    Left,
}

/// What is selected: a placement, or a body of water.
///
/// # Why a tag rather than two lists
///
/// The selection used to be a `Vec<usize>` into `scene.objects`, which was right while
/// placements were the only thing a level held that an author could point at. Water is the
/// second, and a spawn point will be the third — and every one of them wants the same verbs:
/// click to select, drag to move, `X` to delete, `Z` to take it back. Two parallel selections
/// would be two copies of each verb that can disagree about which one a key press meant.
///
/// A selection holds one kind at a time. Selecting a body of water clears a selection of
/// objects and the reverse, because a rule an author can predict beats a general one nobody
/// can: `T` on a mixed selection would mean "scale these and resize those", and the undo of
/// that is a sentence rather than a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selected {
    /// A placement, by its index into `scene.objects`.
    Object(usize),
    /// A body of water, by its index into `scene.water`.
    Water(usize),
    /// A spawn point, by its index into `scene.spawn_points`.
    Spawn(usize),
}

impl Selected {
    /// The object index, if this is an object.
    #[must_use]
    pub fn object(self) -> Option<usize> {
        match self {
            Self::Object(index) => Some(index),
            _ => None,
        }
    }

    /// The water index, if this is a body of water.
    #[must_use]
    pub fn water(self) -> Option<usize> {
        match self {
            Self::Water(index) => Some(index),
            _ => None,
        }
    }

    /// The spawn index, if this is a spawn point.
    #[must_use]
    pub fn spawn(self) -> Option<usize> {
        match self {
            Self::Spawn(index) => Some(index),
            _ => None,
        }
    }

    /// Whether two selections name the same kind of thing.
    #[must_use]
    fn same_kind(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Object(_), Self::Object(_))
                | (Self::Water(_), Self::Water(_))
                | (Self::Spawn(_), Self::Spawn(_))
        )
    }
}

/// A drag of one gizmo handle, and everything it needs to be absolute.
///
/// # Why the gizmo is remembered rather than re-projected
///
/// The handles are projected from the selection's own origin, so a gizmo re-projected each
/// frame moves with the thing being dragged — and then the metres one pixel of movement asks
/// for changes under the author's hand mid-drag, which reads as the object accelerating away
/// from the cursor. The projection taken at the moment of the press is the ruler for the whole
/// drag.
///
/// # Why the origins are remembered
///
/// The drag is absolute: every frame asks "where should this be, given where the pointer
/// started and where it is now", rather than accumulating a delta per frame. An accumulated
/// drag drifts, because a snapped position feeds the next frame's delta back into itself.
#[derive(Debug, Clone, PartialEq)]
struct GizmoDrag {
    /// Which handle was grabbed.
    axis: gizmo::Axis,
    /// Where the pointer went down, in physical pixels.
    from: (f32, f32),
    /// The gizmo as it stood at the press. See above.
    screen: gizmo::GizmoScreen,
    /// Where everything selected stood when the drag began.
    origins: Vec<(Selected, Vec3)>,
    /// Whether anything has actually moved yet, so a click on a handle that does not drag it
    /// does not push an undo snapshot.
    moved: bool,
}

/// A drag that began on a browser row: an asset on its way into the world.
///
/// It is only a *pending* placement until the button comes up somewhere that is not a panel.
/// A press that goes back up over the browser chose an asset and placed nothing, which is what
/// makes choosing and placing two gestures rather than one with a side effect.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlacementDrag {
    /// Where the press happened, so a release that never left the row can be told from one
    /// that was carried out into the level.
    from: (f32, f32),
}

/// How long after a click a second one on the same row still counts as a double click.
///
/// A third of a second: the interval Windows itself defaults to, rounded to something a
/// person types rather than to a platform constant this crate cannot read — it has no
/// `windows` dependency and must not gain one.
const DOUBLE_CLICK_SECONDS: f32 = 0.35;

/// How far the pointer may wander between the press and the release and still be a click.
///
/// In logical pixels. A drag out of the panel is a placement; a hand that moved three pixels
/// while double-clicking a row is still a double click.
const CLICK_SLOP_PIXELS: f32 = 6.0;

/// The editor.
#[derive(Debug)]
pub struct Editor {
    active: bool,
    camera: FlyCamera,
    /// What is in `assets/`, and which of it the next click places.
    browser: browser::AssetBrowser,
    /// What is selected, in the order it was added. The last is the primary one — what the
    /// inspector describes and what a verb reports on. One kind at a time; see [`Selected`].
    selected: Vec<Selected>,
    /// What a drag on a handle means. One mode today; see [`gizmo::GizmoMode`].
    gizmo_mode: gizmo::GizmoMode,
    /// The handle being dragged, if one is.
    gizmo_drag: Option<GizmoDrag>,
    /// Which handle the cursor is over, so it can be drawn as the one that would be grabbed.
    gizmo_hover: Option<gizmo::Axis>,
    /// An asset dragged out of the browser and not yet dropped.
    placement: Option<PlacementDrag>,
    /// Whether a double click on a browser row asked for a placement in front of the camera.
    ///
    /// Set where the row is answered and read where the scene is in hand, because the browser
    /// panel has no scene: it knows what was clicked and nothing about the level.
    place_at_centre: bool,
    /// The last browser row that was clicked and when, for the double click that places one.
    last_row_click: Option<(browser::BrowserRow, f32)>,
    /// Seconds since the editor was opened. The clock the double click is measured on: this
    /// crate has no access to a wall clock and must not gain one — the frame time the
    /// application already hands it is the only time it knows.
    clock: f32,
    /// This frame's view-projection, kept so the interface can draw the gizmo and the
    /// silhouette from the same matrix the pointer was answered with.
    view_projection: Mat4,
    /// This frame's viewport, kept for the same reason.
    viewport: (u32, u32),
    /// Index into [`GRID_STEPS`].
    grid: usize,
    /// Index into [`YAW_STEPS`].
    yaw_step: usize,
    /// Whether the terrain brush has the pointer instead of placement and selection.
    sculpting: bool,
    /// Whether a click in the world puts an object down rather than selecting one.
    ///
    /// Off by default, because a click that places is a click that cannot be taken back
    /// without noticing it happened; on while an author is laying out a row of something.
    /// Mutually exclusive with sculpting: both want the whole world pointer, and a mode that
    /// is two modes is one nobody can predict.
    placing: bool,
    /// What the brush does.
    brush: BrushMode,
    /// How wide the brush is, in metres.
    brush_radius: f32,
    /// How much one stroke moves the ground, in metres.
    brush_strength: f32,
    /// Whether a nudge runs along the object's own axes rather than the world's.
    local_space: bool,
    /// Whether the grid is the cave module rather than whatever `G` last chose.
    ///
    /// Modular pieces meet on a face: two tunnels laid four metres apart on a quarter-turn
    /// grid join, and two laid 3.7 m apart on a 22.5° grid leave a gap somebody has to see
    /// from inside the cave to notice. So the pieces bring their own grid with them.
    module_snap: bool,
    /// Whether the author has pressed `M` and taken the decision off the editor.
    ///
    /// Until they do, module snap follows what is about to be placed: choosing a cave piece
    /// turns it on and choosing a rock turns it off, which is right nearly always and is the
    /// kind of automatic behaviour that must stop the moment somebody disagrees with it.
    module_snap_pinned: bool,
    /// Whether what changed this frame was water or a spawn point rather than a placement.
    ///
    /// Which of the two "something changed" answers the application gets. A selection holds
    /// one kind at a time, so the frame's edit reached one list or the other and never both —
    /// see [`Selected`].
    water_touched: bool,
    /// Snapshots for `Z`, oldest first.
    undo: Vec<Snapshot>,
    /// Where `CTRL+S` writes.
    save_path: PathBuf,
    /// The last thing that happened, printed under the browser.
    status: String,
    /// The first object shown in the hierarchy list.
    hierarchy_scroll: usize,
    /// How long the last frame took, so a brush stroke moves the ground at the same rate
    /// whatever the frame rate is.
    last_seconds: f32,
    scale: UiScale,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// A closed editor with an empty browser.
    ///
    /// Built once, like every other screen in this project: entering the editor allocates
    /// nothing: the directory is read when the editor is opened, not when it is built.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: false,
            camera: FlyCamera::default(),
            browser: browser::AssetBrowser::new(),
            selected: Vec::new(),
            gizmo_mode: gizmo::GizmoMode::Translate,
            gizmo_drag: None,
            gizmo_hover: None,
            placement: None,
            place_at_centre: false,
            last_row_click: None,
            clock: 0.0,
            view_projection: Mat4::IDENTITY,
            viewport: (0, 0),
            grid: 0,
            yaw_step: 2,
            sculpting: false,
            placing: false,
            brush: BrushMode::Raise,
            brush_radius: 6.0,
            brush_strength: 0.5,
            local_space: false,
            module_snap: false,
            module_snap_pinned: false,
            water_touched: false,
            undo: Vec::new(),
            save_path: PathBuf::from("assets/scenes/untitled.json"),
            status: String::new(),
            hierarchy_scroll: 0,
            last_seconds: 1.0 / 60.0,
            scale: UiScale::default(),
        }
    }

    /// Whether the editor has the input and the camera.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The asset browser, for drawing and for asking what is selected.
    #[must_use]
    pub fn browser(&self) -> &browser::AssetBrowser {
        &self.browser
    }

    /// The asset browser, to open a folder or choose an asset from outside the editor's own
    /// input handling. What a test uses, and what a future "reveal this asset" command would.
    pub fn browser_mut(&mut self) -> &mut browser::AssetBrowser {
        &mut self.browser
    }

    /// Which asset the next click places.
    #[must_use]
    pub fn asset(&self) -> EnvironmentAsset {
        self.browser.selected_asset()
    }

    /// The primary selection: what the inspector describes.
    ///
    /// The *last* one added rather than the first, because the thing an author just clicked
    /// on is the thing they are looking at.
    #[must_use]
    pub fn selection(&self) -> Option<Selected> {
        self.selected.last().copied()
    }

    /// Everything selected, in the order it was added.
    #[must_use]
    pub fn selected(&self) -> &[Selected] {
        &self.selected
    }

    /// Whether something is selected.
    #[must_use]
    pub fn is_selected(&self, what: Selected) -> bool {
        self.selected.contains(&what)
    }

    /// Whether a placement is selected.
    #[must_use]
    pub fn is_object_selected(&self, index: usize) -> bool {
        self.is_selected(Selected::Object(index))
    }

    /// Whether a body of water is selected.
    #[must_use]
    pub fn is_water_selected(&self, index: usize) -> bool {
        self.is_selected(Selected::Water(index))
    }

    /// Whether a spawn point is selected.
    #[must_use]
    pub fn is_spawn_selected(&self, index: usize) -> bool {
        self.is_selected(Selected::Spawn(index))
    }

    /// The primary selection, if it is a spawn point.
    #[must_use]
    pub fn selected_spawn(&self) -> Option<usize> {
        self.selection().and_then(Selected::spawn)
    }

    /// The primary selection, if it is a placement.
    #[must_use]
    pub fn selected_object(&self) -> Option<usize> {
        self.selection().and_then(Selected::object)
    }

    /// The primary selection, if it is a body of water.
    #[must_use]
    pub fn selected_water(&self) -> Option<usize> {
        self.selection().and_then(Selected::water)
    }

    /// Every selected placement, in the order it was added.
    ///
    /// Empty when a body of water is selected, because a selection holds one kind at a time —
    /// which is what stops an object verb from ever reaching water.
    #[must_use]
    fn selected_objects(&self) -> Vec<usize> {
        self.selected
            .iter()
            .filter_map(|what| what.object())
            .collect()
    }

    /// Every selected body of water, in the order it was added.
    #[must_use]
    fn selected_waters(&self) -> Vec<usize> {
        self.selected
            .iter()
            .filter_map(|what| what.water())
            .collect()
    }

    /// Every selected spawn point, in the order it was added.
    #[must_use]
    fn selected_spawns(&self) -> Vec<usize> {
        self.selected
            .iter()
            .filter_map(|what| what.spawn())
            .collect()
    }

    /// How far one press of `R` turns an object, in degrees.
    #[must_use]
    pub fn yaw_step(&self) -> f32 {
        if self.module_snap {
            return MODULE_YAW;
        }
        YAW_STEPS[self.yaw_step % YAW_STEPS.len()]
    }

    /// Whether the grid is the cave module rather than the one `G` chose.
    #[must_use]
    pub fn is_module_snap(&self) -> bool {
        self.module_snap
    }

    /// Whether a nudge runs along the object's own axes rather than the world's.
    #[must_use]
    pub fn is_local_space(&self) -> bool {
        self.local_space
    }

    /// Whether the terrain brush has the pointer.
    #[must_use]
    pub fn is_sculpting(&self) -> bool {
        self.sculpting
    }

    /// Whether a click in the world puts an object down rather than selecting one.
    ///
    /// Not to be confused with [`Editor::is_placing`], which is one gesture in progress: an
    /// asset dragged out of the browser and not yet dropped.
    #[must_use]
    pub fn is_place_mode(&self) -> bool {
        self.placing
    }

    /// What the brush does.
    #[must_use]
    pub fn brush(&self) -> BrushMode {
        self.brush
    }

    /// How wide the brush is, in metres.
    #[must_use]
    pub fn brush_radius(&self) -> f32 {
        self.brush_radius
    }

    /// How far one stroke moves the ground, in metres.
    #[must_use]
    pub fn brush_strength(&self) -> f32 {
        self.brush_strength
    }

    /// The snap step in metres. Zero means no snapping.
    #[must_use]
    pub fn grid(&self) -> f32 {
        if self.module_snap {
            return MODULE_GRID;
        }
        GRID_STEPS[self.grid % GRID_STEPS.len()]
    }

    /// Whether `name` is one of the modular cave pieces.
    ///
    /// By where it lives and what it is called, because that is all the editor knows about an
    /// asset — it reads a directory, not a library. A piece somebody adds to `caves/`
    /// tomorrow gets the module grid without a code change, which is the point.
    #[must_use]
    pub fn is_module_piece(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        name.contains("/caves/")
            || name
                .rsplit('/')
                .next()
                .is_some_and(|leaf| leaf.starts_with("cave_"))
    }

    /// Follows the browser's selection with the module grid, until somebody presses `M`.
    fn follow_module_snap(&mut self) {
        if self.module_snap_pinned {
            return;
        }
        let wanted = Self::is_module_piece(self.browser.selected_name());
        if wanted != self.module_snap {
            self.module_snap = wanted;
            self.status = if wanted {
                format!("module snap {MODULE_GRID} m / {MODULE_YAW} deg")
            } else {
                "module snap off".to_owned()
            };
        }
    }

    /// Where `CTRL+S` writes.
    #[must_use]
    pub fn save_path(&self) -> &Path {
        &self.save_path
    }

    /// The last thing that happened, for the status line.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// Which object is at the top of the hierarchy list.
    #[must_use]
    pub fn hierarchy_scroll(&self) -> usize {
        self.hierarchy_scroll
    }

    /// Where `CTRL+S` writes from now on.
    ///
    /// The application sets this when a level was opened from a named file, so saving writes
    /// back to the file it was opened from rather than to one named after the scene.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.status = format!("editing {}", path.display());
        self.save_path = path;
    }

    /// The interface scale the panels are laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.scale
    }

    /// Sets the interface scale the panels are laid out at.
    pub fn set_scale(&mut self, scale: UiScale, _viewport: (u32, u32)) {
        self.scale = scale;
    }

    /// Enters the editor, looking at `focus` and saving to a file named after the scene.
    pub fn enter(&mut self, focus: Vec3, scene_name: &str, viewport: (u32, u32)) {
        self.active = true;
        self.camera = FlyCamera::overlooking(focus);
        self.placing = false;
        self.sculpting = false;
        self.selected.clear();
        self.cancel_manipulation();
        self.hierarchy_scroll = 0;
        self.undo.clear();
        self.save_path = PathBuf::from(format!("assets/scenes/{scene_name}.json"));
        // The directory is read here rather than at startup, so an asset added between
        // launching the game and pressing F6 is in the browser the first time it is opened.
        let root = browser::default_root();
        self.browser.scan(&root);
        let counts = self.browser.catalogue().counts();
        self.status = format!("editing '{scene_name}'");
        log::info!(
            "editor: {} holds {} model(s), {} material(s), {} texture(s), {} level(s)",
            root.display(),
            counts[0],
            counts[1],
            counts[2],
            counts[3]
        );
        for problem in &self.browser.catalogue().report().unreadable {
            log::warn!("editor: could not read {problem}");
        }
        let _ = viewport;
    }

    /// Leaves the editor. The scene keeps whatever was done to it.
    pub fn leave(&mut self) {
        self.active = false;
        self.cancel_manipulation();
    }

    /// Writes this camera into the renderer's.
    ///
    /// # Errors
    ///
    /// [`RenderDataError::InvalidConfig`] for a degenerate aspect ratio.
    pub fn apply_camera(&self, camera: &mut Camera, aspect: f32) -> Result<(), RenderDataError> {
        self.camera.apply(camera, aspect)
    }

    /// Applies one frame of input to `scene`.
    ///
    /// `view_projection` must be this frame's, from the camera [`Editor::apply_camera`]
    /// wrote — a ray built from last frame's matrix places objects where the cursor was, not
    /// where it is.
    pub fn update(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        seconds: f32,
    ) -> EditorOutcome {
        if !self.active {
            return EditorOutcome::Idle;
        }
        if input.was_key_pressed(KeyCode::F6) {
            self.leave();
            return EditorOutcome::Left;
        }
        self.water_touched = false;
        // Remembered so the interface can draw the gizmo and the silhouette from the same
        // matrix the pointer was answered with, and so the double click has a clock to
        // measure itself on.
        self.view_projection = *view_projection;
        self.viewport = viewport;
        self.clock += seconds;

        // The browser reads the keyboard first, because while its filter is open every key
        // belongs to the filter — including the ones that would otherwise fly the camera.
        let browser_took_the_keys = self.browser.update_keys(input);
        if !self.browser.is_typing() {
            self.last_seconds = seconds;
            self.camera.update(input, seconds);
        }

        // The panels own the pointer over themselves: a click on a browser row chooses an
        // asset and must never also place one in the world behind the panel.
        let over_browser = self.update_browser(input, ui_input, viewport);
        // What is about to be placed decides the grid, until somebody says otherwise.
        self.follow_module_snap();
        let over_hierarchy = self.update_hierarchy(input, ui_input, scene, viewport);

        // Sculpting takes the world pointer and most of the keyboard; the panels keep theirs.
        // A key the brush answered is *not* passed on: `B` cycles the brush while sculpting
        // and switches modes while not, and one press must not do both.
        if self.sculpting && !self.browser.is_typing() {
            let (outcome, handled) = self.update_terrain(
                input,
                ui_input,
                scene,
                view_projection,
                viewport,
                over_browser || over_hierarchy,
            );
            if outcome != EditorOutcome::Idle || handled {
                self.reveal_selection(scene, viewport);
                return outcome;
            }
        }

        // `V` pours water where the cursor is, and it needs the ray the pointer verbs use
        // rather than only the keyboard — so it is answered here, before the keyboard verbs,
        // and its key is not passed on to them.
        if !browser_took_the_keys && !self.browser.is_typing() && !self.sculpting {
            let over_panel = over_browser || over_hierarchy;
            // Both of these need the ray the pointer verbs use rather than only the keyboard,
            // so they are answered here — before the keyboard verbs, and their key is not
            // passed on to them.
            let placed = if input.was_key_pressed(KeyCode::V) {
                Some(self.pour_water(ui_input, scene, view_projection, viewport, over_panel))
            } else if input.was_key_pressed(KeyCode::P) {
                Some(self.put_spawn(ui_input, scene, view_projection, viewport, over_panel))
            } else {
                None
            };
            if let Some(placed) = placed {
                self.reveal_selection(scene, viewport);
                return if placed {
                    EditorOutcome::WaterChanged
                } else {
                    EditorOutcome::Idle
                };
            }
        }

        let mut changed = if browser_took_the_keys || self.browser.is_typing() {
            false
        } else {
            self.update_keys(input, scene)
        };
        // An asset carried out of the browser, or asked for by a double click on a row. The
        // only two ways anything is placed — see [`Editor::update_placement`].
        if self.update_placement(
            ui_input,
            scene,
            view_projection,
            viewport,
            over_browser || over_hierarchy,
        ) {
            changed = true;
        }

        let add_to_selection =
            input.is_key_down(KeyCode::LeftControl) || input.is_key_down(KeyCode::RightControl);
        if self.update_pointer(
            ui_input,
            scene,
            view_projection,
            viewport,
            over_browser || over_hierarchy,
            add_to_selection,
        ) {
            changed = true;
        }

        // Whatever just happened, what is selected is worth seeing in the list.
        self.reveal_selection(scene, viewport);

        match (changed, self.water_touched) {
            // Water changed and no placement did: one or two quads to rebuild, and none of
            // the level's render entities to respawn.
            (true, true) => EditorOutcome::WaterChanged,
            (true, false) => EditorOutcome::SceneChanged,
            (false, _) => EditorOutcome::Idle,
        }
    }

    /// Puts a spawn point on the ground under the cursor. Returns whether one arrived.
    ///
    /// Facing whatever the last one was turned to, or -Z on the first: an author laying out a
    /// row of starts is nearly always laying out a row that faces the same way, and `R` turns
    /// the one that does not.
    fn put_spawn(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
    ) -> bool {
        if over_panel {
            return false;
        }
        let Some(ground) = engine_presentation::ui::targeting::ground_under_cursor(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            self.status = "point at the ground to put a spawn point".to_owned();
            return false;
        };
        self.snapshot(scene);
        let yaw = scene.spawn_points.last().map_or(0.0, |last| last.yaw);
        // On the ground the level actually has under the cursor, so a spawn on a plateau
        // belongs to the plateau — the same rule water is poured with.
        let standing = self.snap(ground);
        let position = Vec3::new(standing.x, scene.ground_at(standing), standing.z);
        scene.push_spawn(SpawnPoint { position, yaw });
        self.selected.clear();
        self.selected
            .push(Selected::Spawn(scene.spawn_points.len() - 1));
        self.water_touched = true;
        self.status = format!("spawn point at {:.0} {:.0}", position.x, position.z);
        true
    }

    /// Pours a body of water on the ground under the cursor. Returns whether one arrived.
    ///
    /// Eight metres square and two deep, at the height of the ground it was poured onto — the
    /// size of a thing an author grows rather than one they immediately have to shrink. It is
    /// a snapshot like any other edit, so `Z` takes the pour back.
    fn pour_water(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
    ) -> bool {
        if over_panel {
            return false;
        }
        let Some(ground) = engine_presentation::ui::targeting::ground_under_cursor(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            self.status = "point at the ground to pour water".to_owned();
            return false;
        };
        self.snapshot(scene);
        // At the height of the ground under the cursor rather than at the level's floor: a
        // pond poured into a valley belongs to the valley, and one poured on a plateau to the
        // plateau.
        let surface = Vec3::new(ground.x, scene.ground_at(ground), ground.z);
        scene.push_water(WaterBody::new(
            self.snap(surface),
            engine_core::math::Vec2::new(WATER_HALF_EXTENT, WATER_HALF_EXTENT),
            WATER_DEPTH,
        ));
        self.selected.clear();
        self.selected.push(Selected::Water(scene.water.len() - 1));
        self.water_touched = true;
        self.status = format!("poured water at {:.0} {:.0}", surface.x, surface.z);
        true
    }

    /// One frame of the terrain brush.
    ///
    /// Returns [`EditorOutcome::TerrainChanged`] on a frame the ground moved, and `Idle`
    /// otherwise — including on the frames the author is only changing the brush, which must
    /// not cost a mesh rebuild — together with whether the brush answered this frame's key at
    /// all, which is what stops that key from also meaning something to the editor behind it.
    fn update_terrain(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
    ) -> (EditorOutcome, bool) {
        // The brush's own keys. Shared with nothing: while sculpting, `R` is not a rotation
        // and `X` is not a delete, because there is no selection to rotate or delete. `B` is
        // deliberately *not* one of them — it is the key that leaves, and a mode key that
        // means something else once you are inside the mode is a key nobody can find their
        // way out with. TAB cycles the brush, which is what TAB does in the other mode too:
        // the next thing.
        if input.was_key_pressed(KeyCode::Tab) {
            self.brush = self.brush.next();
            self.status = format!("brush {}", self.brush.as_str());
            return (EditorOutcome::Idle, true);
        }
        if input.was_key_pressed(KeyCode::Digit1) {
            self.brush_radius = (self.brush_radius / BRUSH_SIZE_STEP).max(1.0);
            self.status = format!("brush {:.1} m", self.brush_radius);
            return (EditorOutcome::Idle, true);
        }
        if input.was_key_pressed(KeyCode::Digit2) {
            self.brush_radius = (self.brush_radius * BRUSH_SIZE_STEP).min(80.0);
            self.status = format!("brush {:.1} m", self.brush_radius);
            return (EditorOutcome::Idle, true);
        }
        if input.was_key_pressed(KeyCode::Digit3) {
            self.brush_strength = (self.brush_strength / BRUSH_SIZE_STEP).max(0.05);
            self.status = format!("strength {:.2} m", self.brush_strength);
            return (EditorOutcome::Idle, true);
        }
        if input.was_key_pressed(KeyCode::Digit4) {
            self.brush_strength = (self.brush_strength * BRUSH_SIZE_STEP).min(8.0);
            self.status = format!("strength {:.2} m", self.brush_strength);
            return (EditorOutcome::Idle, true);
        }
        if input.was_key_pressed(KeyCode::Z) {
            // Undo can put back a scene whose objects *and* terrain are different, so the
            // whole level is rebuilt rather than guessing which half moved.
            let outcome = if self.undo(scene) {
                EditorOutcome::SceneChanged
            } else {
                EditorOutcome::Idle
            };
            return (outcome, true);
        }

        // Nothing below is a key, so nothing below has answered one: a frame in which the
        // brush is not painting leaves the keyboard to the editor.
        if over_panel || !ui_input.pointer_down {
            return (EditorOutcome::Idle, false);
        }
        let Some(ground) = engine_presentation::ui::targeting::ground_under_cursor(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            return (EditorOutcome::Idle, false);
        };

        // A level with no terrain gets one the first time somebody paints on it. Sized to the
        // level's own playable square, so the ground reaches as far as the walls do.
        if scene.terrain.is_none() {
            let cell = 2.0;
            let resolution = ((scene.half_extent * 2.0 / cell).round() as usize + 1).max(9);
            scene.terrain = Some(Terrain::flat(resolution, cell, scene.ground_height));
            self.snapshot(scene);
            self.status = format!("terrain created, {resolution} points a side");
        }

        // The snapshot is taken when a stroke *starts*, not on every frame of it: a drag is
        // one edit, and an undo stack with sixty entries for one stroke is an undo stack
        // nobody can walk back through.
        if ui_input.pointer_pressed {
            self.snapshot(scene);
        }

        let Some(terrain) = scene.terrain.as_mut() else {
            return (EditorOutcome::Idle, false);
        };
        // Per second rather than per frame, so a stroke moves the ground by the same amount on
        // a fast machine and a slow one.
        let amount = self.brush_strength * self.last_seconds.clamp(0.0, 0.1) * 30.0;
        if terrain.apply_brush(ground, self.brush_radius, amount, self.brush) {
            self.status = format!("{} at {:.0} {:.0}", self.brush.as_str(), ground.x, ground.z);
            return (EditorOutcome::TerrainChanged, true);
        }
        (EditorOutcome::Idle, true)
    }

    /// The asset browser: scrolling it, opening folders and choosing what to place.
    ///
    /// Returns whether the pointer is over the browser panel, which is what stops the same
    /// click from also placing an object in the world behind it.
    fn update_browser(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        viewport: (u32, u32),
    ) -> bool {
        let panel = ui::browser_rect(self.scale, viewport);
        let visible = ui::browser_visible_rows(self.scale, viewport);
        self.browser.clamp_scroll(visible);

        let (x, y) = ui_input.pointer;
        if !panel.contains(x, y) {
            return false;
        }

        let wheel = input.wheel_delta();
        if wheel != 0.0 {
            let step = wheel.abs().max(1.0) as i32;
            self.browser
                .scroll_by(if wheel > 0.0 { -step } else { step }, visible);
        }

        if ui_input.pointer_pressed {
            for slot in 0..visible {
                if !ui::browser_row_rect(self.scale, viewport, slot).contains(x, y) {
                    continue;
                }
                let Some(row) = self.browser.row(self.browser.scroll() + slot) else {
                    break;
                };
                match row {
                    browser::BrowserRow::Folder(_) | browser::BrowserRow::BuiltinFolder => {
                        self.browser.toggle(row);
                    }
                    _ => {
                        if self.browser.choose(row) {
                            self.status = format!("placing {}", self.browser.selected_name());
                        }
                        // A press on a row is the *start* of a placement, and nothing has been
                        // placed yet: it becomes one only when the button comes up over the
                        // world. A second press on the same row inside the double-click
                        // interval means "put one in front of me" instead.
                        let doubled = self.last_row_click.is_some_and(|(last, when)| {
                            last == row && self.clock - when <= DOUBLE_CLICK_SECONDS
                        });
                        if doubled {
                            self.place_at_centre = true;
                            self.placement = None;
                            self.last_row_click = None;
                        } else {
                            self.placement = Some(PlacementDrag { from: (x, y) });
                            self.last_row_click = Some((row, self.clock));
                        }
                    }
                }
                break;
            }
        }
        true
    }

    /// The hierarchy list: scrolling it, and selecting out of it.
    ///
    /// Returns whether the pointer is over the hierarchy or the inspector, which is what
    /// stops the same click from also placing an object in the world behind them.
    fn update_hierarchy(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &EnvironmentScene,
        viewport: (u32, u32),
    ) -> bool {
        let hierarchy = ui::hierarchy_rect(self.scale, viewport);
        let inspector = ui::inspector_rect(self.scale, viewport);
        let (x, y) = ui_input.pointer;
        let over_hierarchy = hierarchy.contains(x, y);
        if !over_hierarchy && !inspector.contains(x, y) {
            return false;
        }

        let visible = ui::hierarchy_visible_rows(self.scale, viewport);
        let rows = hierarchy_len(scene);
        // Scrolled by whole rows, and never past the end: a list that can be scrolled into
        // emptiness looks like a list that has lost its contents.
        let wheel = input.wheel_delta();
        if over_hierarchy && wheel != 0.0 {
            let last = rows.saturating_sub(visible);
            let step = wheel.abs().max(1.0) as usize;
            self.hierarchy_scroll = if wheel > 0.0 {
                self.hierarchy_scroll.saturating_sub(step)
            } else {
                (self.hierarchy_scroll + step).min(last)
            };
        }
        self.hierarchy_scroll = self.hierarchy_scroll.min(rows.saturating_sub(visible));

        if over_hierarchy && ui_input.pointer_pressed {
            for slot in 0..visible {
                if !ui::hierarchy_row_rect(self.scale, viewport, slot).contains(x, y) {
                    continue;
                }
                // The list is the objects and then the water, so one row index answers for
                // both — which is what keeps the hit test one rectangle per row rather than
                // two lists with a boundary between them.
                let Some(what) = hierarchy_row_at(self.hierarchy_scroll + slot, scene) else {
                    break;
                };
                let control = input.is_key_down(KeyCode::LeftControl)
                    || input.is_key_down(KeyCode::RightControl);
                self.select(what, control);
                self.cancel_manipulation();
                self.status = match what {
                    Selected::Object(index) => format!(
                        "selected {} ({index}){}",
                        scene.objects[index].asset.name(),
                        if self.selected.len() > 1 {
                            format!(" of {}", self.selected.len())
                        } else {
                            String::new()
                        }
                    ),
                    Selected::Water(index) => format!("selected water ({index})"),
                    Selected::Spawn(index) => format!("selected spawn ({index})"),
                };
                break;
            }
        }
        true
    }

    /// Adds `index` to the selection, or replaces the selection with it.
    ///
    /// `add` is what CTRL does: it *toggles*, so control-clicking a selected object takes it
    /// out again, which is the one gesture every editor agrees on.
    fn select(&mut self, what: Selected, add: bool) {
        // A selection holds one kind at a time: adding water to a selection of objects
        // replaces it rather than mixing the two. See [`Selected`] for why.
        let mixing = self
            .selected
            .first()
            .is_some_and(|first| !first.same_kind(what));
        if !add || mixing {
            self.selected.clear();
            self.selected.push(what);
            return;
        }
        if let Some(position) = self.selected.iter().position(|known| *known == what) {
            self.selected.remove(position);
        } else {
            self.selected.push(what);
        }
    }

    /// Drops selected indices that are past the end of the scene.
    ///
    /// Undo replaces the object list wholesale, so a selection made before it can point past
    /// the end afterwards.
    fn prune_selection(&mut self, scene: &EnvironmentScene) {
        self.selected.retain(|what| match what {
            Selected::Object(index) => *index < scene.objects.len(),
            Selected::Water(index) => *index < scene.water.len(),
            Selected::Spawn(index) => *index < scene.spawn_points.len(),
        });
    }

    /// Keeps the selected thing in view, so a keyboard verb's result is visible.
    fn reveal_selection(&mut self, scene: &EnvironmentScene, viewport: (u32, u32)) {
        let Some(index) = self.selection().map(|what| hierarchy_row_of(what, scene)) else {
            return;
        };
        let visible = ui::hierarchy_visible_rows(self.scale, viewport).max(1);
        if index < self.hierarchy_scroll {
            self.hierarchy_scroll = index;
        } else if index >= self.hierarchy_scroll + visible {
            self.hierarchy_scroll = index + 1 - visible;
        }
    }

    /// The keyboard verbs. Returns whether the scene changed.
    fn update_keys(&mut self, input: &InputState, scene: &mut EnvironmentScene) -> bool {
        let control =
            input.is_key_down(KeyCode::LeftControl) || input.is_key_down(KeyCode::RightControl);
        let shift = input.is_key_down(KeyCode::LeftShift) || input.is_key_down(KeyCode::RightShift);

        if input.was_key_pressed(KeyCode::Escape) {
            // The way out of everything in progress, not only the selection: an asset picked
            // up from the browser and not yet dropped is a gesture an author has to be able
            // to abandon, and a mode where every click puts something down is another. The
            // key that abandons things is this one.
            self.selected.clear();
            self.cancel_manipulation();
            if self.placing {
                self.placing = false;
                self.status = "select and move".to_owned();
            }
            return false;
        }
        if input.was_key_pressed(KeyCode::G) {
            if shift {
                self.yaw_step = (self.yaw_step + 1) % YAW_STEPS.len();
                self.status = format!("rotate by {} deg", self.yaw_step());
            } else {
                self.grid = (self.grid + 1) % GRID_STEPS.len();
                let step = self.grid();
                self.status = if step > 0.0 {
                    format!("grid {step} m")
                } else {
                    "grid off".to_owned()
                };
            }
            return false;
        }
        if input.was_key_pressed(KeyCode::M) {
            // Pressing it takes the decision off the editor for the rest of the session: an
            // author who has said what they want must not have it changed under them by the
            // next asset they click on.
            self.module_snap = !self.module_snap;
            self.module_snap_pinned = true;
            self.status = if self.module_snap {
                format!("module snap {MODULE_GRID} m / {MODULE_YAW} deg")
            } else {
                "module snap off".to_owned()
            };
            return false;
        }
        if input.was_key_pressed(KeyCode::B) {
            self.sculpting = !self.sculpting;
            // The two world modes are exclusive: the brush and the placing click both want
            // every press in the viewport, and an editor in both is an editor whose next
            // click nobody can predict.
            self.placing = false;
            self.selected.clear();
            self.cancel_manipulation();
            self.status = if self.sculpting {
                "sculpting: drag to shape the ground".to_owned()
            } else {
                "select and move".to_owned()
            };
            return false;
        }
        if input.was_key_pressed(KeyCode::N) {
            self.placing = !self.placing;
            self.sculpting = false;
            self.cancel_manipulation();
            self.status = if self.placing {
                format!(
                    "placing {}: click to put one down",
                    self.browser.selected_name()
                )
            } else {
                "select and move".to_owned()
            };
            return false;
        }
        if input.was_key_pressed(KeyCode::L) {
            self.local_space = !self.local_space;
            self.status = if self.local_space {
                "local axes".to_owned()
            } else {
                "world axes".to_owned()
            };
            return false;
        }
        if input.was_key_pressed(KeyCode::Tab) {
            self.browser.select_next();
            self.status = format!("placing {}", self.browser.selected_name());
            return false;
        }
        if control && input.was_key_pressed(KeyCode::A) {
            // Every object, and no water: `CTRL+A` on a level with a lake would otherwise
            // put two kinds in one selection, which is the one thing the tag forbids.
            self.selected = (0..scene.objects.len()).map(Selected::Object).collect();
            self.status = format!("selected {}", self.selected.len());
            return false;
        }
        if control && input.was_key_pressed(KeyCode::S) {
            self.save(scene);
            return false;
        }
        if input.was_key_pressed(KeyCode::Z) {
            return self.undo(scene);
        }

        // Everything below acts on the selection, and every one of them acts on *all* of it.
        self.prune_selection(scene);
        if self.selected.is_empty() {
            return false;
        }
        // A selection holds one kind, so whatever the verb below turns out to be, this is the
        // list it will reach — which is how the application is told a quad moved rather than
        // the level.
        self.water_touched =
            !self.selected_waters().is_empty() || !self.selected_spawns().is_empty();

        if input.was_key_pressed(KeyCode::X) {
            self.snapshot(scene);
            let count = self.selected.len();
            // Highest index first, so removing one does not move the next one under it. Each
            // kind is removed from its own list, and a selection holds one kind, so a delete
            // never reaches the other.
            let mut objects = self.selected_objects();
            objects.sort_unstable();
            for index in objects.into_iter().rev() {
                scene.objects.remove(index);
            }
            let mut waters = self.selected_waters();
            waters.sort_unstable();
            for index in waters.into_iter().rev() {
                scene.water.remove(index);
            }
            let mut spawns = self.selected_spawns();
            spawns.sort_unstable();
            for index in spawns.into_iter().rev() {
                scene.spawn_points.remove(index);
            }
            self.status = format!("deleted {count}");
            self.selected.clear();
            return true;
        }
        if input.was_key_pressed(KeyCode::C) {
            self.snapshot(scene);
            // Offset by one grid step, or a metre when the grid is off, so the copy is
            // visible instead of hiding exactly inside the original.
            let step = if self.grid() > 0.0 { self.grid() } else { 1.0 };
            let mut copies = Vec::with_capacity(self.selected.len());
            for index in self.selected_objects() {
                let Some(mut copy) = scene.objects.get(index).copied() else {
                    continue;
                };
                copy.transform.translation += Vec3::new(step, 0.0, step);
                copies.push(Selected::Object(scene.objects.len()));
                scene.objects.push(copy);
            }
            for index in self.selected_waters() {
                let Some(mut copy) = scene.water.get(index).copied() else {
                    continue;
                };
                copy.center += Vec3::new(step, 0.0, step);
                copies.push(Selected::Water(scene.water.len()));
                scene.water.push(copy);
            }
            for index in self.selected_spawns() {
                let Some(mut copy) = scene.spawn_points.get(index).copied() else {
                    continue;
                };
                copy.position += Vec3::new(step, 0.0, step);
                copies.push(Selected::Spawn(scene.spawn_points.len()));
                scene.spawn_points.push(copy);
            }
            self.status = format!("duplicated {}", copies.len());
            // The copies become the selection, so dragging straight afterwards moves the new
            // ones rather than the originals.
            self.selected = copies;
            return true;
        }
        if input.was_key_pressed(KeyCode::R) {
            self.snapshot(scene);
            let step = self.yaw_step().to_radians() * if shift { -1.0 } else { 1.0 };
            let mut last = 0.0;
            for index in self.selected_waters() {
                let Some(body) = scene.water.get_mut(index) else {
                    continue;
                };
                last = snap_angle(body.yaw + step, self.yaw_step().to_radians());
                body.yaw = last;
            }
            // Turning a spawn point is turning the player who starts on it, which is the one
            // thing about a spawn point that is not obvious from where it stands.
            for index in self.selected_spawns() {
                let Some(spawn) = scene.spawn_points.get_mut(index) else {
                    continue;
                };
                last = snap_angle(spawn.yaw + step, self.yaw_step().to_radians());
                spawn.yaw = last;
            }
            for index in self.selected_objects() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                // Snapped to a multiple of the step rather than added to: an object turned by
                // hand and then stepped ends up aligned with everything else stepped by the
                // same amount, which is the whole point of a rotation snap.
                last = snap_angle(
                    yaw_of(object.transform) + step,
                    self.yaw_step().to_radians(),
                );
                object.transform.rotation =
                    engine_core::math::Quat::from_yaw_pitch_roll(last, 0.0, 0.0);
            }
            self.status = format!("yaw {:.0} deg", last.to_degrees());
            return true;
        }
        if input.was_key_pressed(KeyCode::T) {
            self.snapshot(scene);
            let factor = if shift { 1.0 / SCALE_STEP } else { SCALE_STEP };
            let mut last = 1.0;
            for index in self.selected_objects() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                object.transform.scale *= factor;
                last = object.transform.scale.y;
            }
            // A body of water has no scale: it has a size, in metres, and growing it is what
            // `T` means for one. Never past the point where it stops being a volume, because
            // a body with no extent is a level the scene loader refuses to read back.
            let mut resized = false;
            for index in self.selected_waters() {
                let Some(body) = scene.water.get_mut(index) else {
                    continue;
                };
                body.half_extents.x = (body.half_extents.x * factor).max(WATER_MIN_HALF_EXTENT);
                body.half_extents.y = (body.half_extents.y * factor).max(WATER_MIN_HALF_EXTENT);
                last = body.half_extents.x;
                resized = true;
            }
            self.status = if resized {
                format!("water {last:.1} m")
            } else {
                format!("scale {last:.2}")
            };
            return true;
        }
        if input.was_key_pressed(KeyCode::Digit0) {
            self.snapshot(scene);
            for index in self.selected_objects() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                // Where it stands is what an author placed; how it is turned and how big it is
                // are what they have been fiddling with. Reset means the second, not the first.
                object.transform.rotation = engine_core::math::Quat::IDENTITY;
                object.transform.scale = Vec3::ONE;
            }
            for index in self.selected_waters() {
                let Some(body) = scene.water.get_mut(index) else {
                    continue;
                };
                // The same rule, said about water: the yaw is what was fiddled with, and the
                // size is what somebody dragged out on purpose.
                body.yaw = 0.0;
            }
            for index in self.selected_spawns() {
                let Some(spawn) = scene.spawn_points.get_mut(index) else {
                    continue;
                };
                spawn.yaw = 0.0;
            }
            self.status = "transform reset".to_owned();
            return true;
        }
        if let Some(step) = self.nudge(input, shift) {
            self.snapshot(scene);
            let local = self.local_space;
            // In local space the step runs along the thing's own facing, so nudging a yawed
            // wall moves it along the wall rather than along the world's X. Written once, so
            // a yawed pond obeys the same rule a yawed wall does.
            let along = |yaw: f32| {
                if !local {
                    return step;
                }
                let (sin, cos) = yaw.sin_cos();
                Vec3::new(
                    step.x * cos + step.z * sin,
                    step.y,
                    -step.x * sin + step.z * cos,
                )
            };
            for index in self.selected_objects() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                object.transform.translation += along(yaw_of(object.transform));
            }
            for index in self.selected_waters() {
                let Some(body) = scene.water.get_mut(index) else {
                    continue;
                };
                body.center += along(body.yaw);
            }
            for index in self.selected_spawns() {
                let Some(spawn) = scene.spawn_points.get_mut(index) else {
                    continue;
                };
                spawn.position += along(spawn.yaw);
            }
            self.status = format!(
                "moved {:.2} {:.2} {:.2} ({})",
                step.x,
                step.y,
                step.z,
                if self.local_space { "local" } else { "world" }
            );
            return true;
        }
        false
    }

    /// How far this frame's arrow keys move the selection, if any were pressed.
    ///
    /// The step is the grid, so nudging and dragging agree about what "one over" means. With
    /// the grid off it is a tenth of a metre, which is the size of an adjustment somebody
    /// makes with an arrow key rather than with the mouse.
    ///
    /// `SHIFT` moves in Y instead of Z, because a blockout is laid out in plan and lifted
    /// occasionally — and because this engine's input state has no page keys to spend on it.
    fn nudge(&self, input: &InputState, shift: bool) -> Option<Vec3> {
        let step = if self.grid() > 0.0 { self.grid() } else { 0.1 };
        let mut delta = Vec3::ZERO;
        if input.was_key_pressed(KeyCode::ArrowLeft) {
            delta.x -= step;
        }
        if input.was_key_pressed(KeyCode::ArrowRight) {
            delta.x += step;
        }
        if input.was_key_pressed(KeyCode::ArrowUp) {
            if shift {
                delta.y += step;
            } else {
                delta.z -= step;
            }
        }
        if input.was_key_pressed(KeyCode::ArrowDown) {
            if shift {
                delta.y -= step;
            } else {
                delta.z += step;
            }
        }
        (delta != Vec3::ZERO).then_some(delta)
    }

    /// Forgets any manipulation in progress: a handle being dragged, an asset being carried.
    ///
    /// One function rather than two fields cleared in eight places, because "the editor is no
    /// longer in the middle of something" is one idea — and a gesture left half-finished by a
    /// mode change is exactly how a click three frames later moves something nobody selected.
    fn cancel_manipulation(&mut self) {
        self.gizmo_drag = None;
        self.placement = None;
        self.gizmo_hover = None;
        self.place_at_centre = false;
    }

    /// Where the gizmo stands: the primary selection's own position.
    ///
    /// The primary rather than the centre of the group, because the primary is the thing the
    /// inspector describes and the thing every keyboard verb reports on. A gizmo at the
    /// average of a hundred selected rocks stands in empty space.
    #[must_use]
    pub fn gizmo_origin(&self, scene: &EnvironmentScene) -> Option<Vec3> {
        position_of(scene, self.selection()?)
    }

    /// The gizmo as the pointer sees it this frame, if there is one to see.
    ///
    /// `None` when nothing is selected, when the brush has the pointer — a gizmo an author
    /// cannot grab is a lie about what a click will do — or when the selection is off screen,
    /// which is the case the striped silhouette answers instead.
    #[must_use]
    pub fn gizmo(&self, scene: &EnvironmentScene) -> Option<gizmo::GizmoScreen> {
        self.gizmo_with(scene, &self.view_projection, self.viewport)
    }

    /// [`Editor::gizmo`], against a matrix and a viewport the caller has rather than the
    /// remembered ones. What the pointer code uses, since it is answered before the frame's
    /// matrix has been remembered.
    fn gizmo_with(
        &self,
        scene: &EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
    ) -> Option<gizmo::GizmoScreen> {
        // No handles in the two modes where the left button means something else: a gizmo
        // an author cannot grab is a lie about what a click will do.
        if self.sculpting || self.placing {
            return None;
        }
        let origin = self.gizmo_origin(scene)?;
        let length = gizmo::length_for(origin, self.camera.position);
        gizmo::project(self.gizmo_mode, origin, length, view_projection, viewport)
    }

    /// Which handle the cursor is over, or the one being dragged while a drag is on.
    #[must_use]
    pub fn gizmo_hover(&self) -> Option<gizmo::Axis> {
        self.gizmo_drag
            .as_ref()
            .map(|drag| drag.axis)
            .or(self.gizmo_hover)
    }

    /// Which handle is being dragged, if one is.
    #[must_use]
    pub fn gizmo_axis(&self) -> Option<gizmo::Axis> {
        self.gizmo_drag.as_ref().map(|drag| drag.axis)
    }

    /// Whether an asset is being carried out of the browser and has not been dropped yet.
    #[must_use]
    pub fn is_placing(&self) -> bool {
        self.placement.is_some()
    }

    /// Where the editor's eye is. What the overlay measures distances from.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.camera.position
    }

    /// The view-projection the last frame was answered with. What the interface draws from.
    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        self.view_projection
    }

    /// Selecting and manipulating. Returns whether the scene changed.
    ///
    /// The order is the whole point, and it is the order of the brief:
    ///
    /// 1. a gizmo drag in progress owns the pointer until the button comes up;
    /// 2. a press on a handle starts one;
    /// 3. a press on something in the world *selects it*, and never places anything;
    /// 4. a press on empty ground drops the selection.
    ///
    /// Placing is not in this list. It is a drag out of the browser or a double click on a
    /// browser row — see [`Editor::update_placement`] — which is what makes "click a rock"
    /// mean one thing and never two.
    fn update_pointer(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
        add_to_selection: bool,
    ) -> bool {
        // A drag in progress owns the pointer until the button comes up, panels included: a
        // handle dragged over the browser is still that handle being dragged.
        if self.gizmo_drag.is_some() {
            return self.drag_gizmo(ui_input, scene);
        }

        // Placing mode: every press in the world puts one down, and nothing else happens.
        // Deliberately *not* a ray against the scene first — an author laying out a row of
        // pillars wants the next one where the cursor is, including on top of the last one.
        if self.placing {
            if !ui_input.pointer_pressed || over_panel {
                return false;
            }
            let Some(ground) = engine_presentation::ui::targeting::ground_under_cursor(
                ui_input.pointer,
                view_projection,
                viewport,
            ) else {
                self.status = "point at the ground to place".to_owned();
                return false;
            };
            return self.place(scene, ground);
        }

        let screen = self.gizmo_with(scene, view_projection, viewport);
        let grab = self.scale.px(gizmo::GRAB_PIXELS);
        self.gizmo_hover = if over_panel {
            None
        } else {
            screen.and_then(|screen| screen.hit(ui_input.pointer, grab))
        };

        if !ui_input.pointer_pressed || over_panel {
            return false;
        }

        // A handle beats everything under it: the handles are drawn over the level, so what
        // is behind one is not what the author was aiming at.
        if let (Some(axis), Some(screen)) = (self.gizmo_hover, screen) {
            let origins = self
                .selected
                .iter()
                .filter_map(|what| position_of(scene, *what).map(|origin| (*what, origin)))
                .collect();
            self.gizmo_drag = Some(GizmoDrag {
                axis,
                from: ui_input.pointer,
                screen,
                origins,
                moved: false,
            });
            self.status = format!("moving along {}", axis.label());
            return false;
        }

        let Some((origin, direction)) = engine_presentation::ui::targeting::cursor_ray(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            return false;
        };

        let Some(what) = pick::under_ray(scene, origin, direction) else {
            // Empty ground. Nothing is placed and nothing is moved: the selection is dropped,
            // which is the one thing a click on nothing can mean without guessing.
            if !self.selected.is_empty() {
                self.selected.clear();
                self.status = "selection cleared".to_owned();
            }
            return false;
        };

        // CTRL toggles: control-clicking a selected object takes it out of the group again,
        // which is the one gesture every editor agrees on. A plain click on something already
        // selected keeps the group, so a group can be nudged or dragged by its gizmo without
        // a stray click on one of its members dissolving it.
        if add_to_selection || !self.is_selected(what) {
            self.select(what, add_to_selection);
        }
        self.status = if self.selected.len() > 1 {
            format!("selected {} objects", self.selected.len())
        } else {
            match what {
                Selected::Object(index) => {
                    format!("selected {}", scene.objects[index].asset.name())
                }
                Selected::Water(index) => format!("selected water ({index})"),
                Selected::Spawn(index) => format!("selected spawn ({index})"),
            }
        };
        // Selecting never changes the level. That is what "a single click on an existing
        // object always means selection" costs: nothing.
        false
    }

    /// One frame of a gizmo drag. Returns whether the scene changed.
    fn drag_gizmo(&mut self, ui_input: &UiInput, scene: &mut EnvironmentScene) -> bool {
        let Some(drag) = self.gizmo_drag.clone() else {
            return false;
        };
        if !ui_input.pointer_down {
            self.gizmo_drag = None;
            return false;
        }
        let metres = drag.screen.metres(drag.axis, drag.from, ui_input.pointer);
        let step = drag.axis.direction() * metres;

        // Every target is worked out before anything moves, so the undo snapshot is the scene
        // as it was before this frame and not as it was halfway through it.
        let mut moves: Vec<(Selected, Vec3)> = Vec::new();
        for (what, origin) in &drag.origins {
            let Some(standing) = position_of(scene, *what) else {
                continue;
            };
            let target = self.snap_along(drag.axis, *origin + step);
            if target != standing {
                moves.push((*what, target - standing));
            }
        }
        if moves.is_empty() {
            return false;
        }
        if !drag.moved {
            // Taken on the first movement rather than on the press: a click on a handle that
            // does not drag it must not fill the undo stack.
            let before = Snapshot::of(scene);
            self.push_undo(before);
            self.gizmo_drag = Some(GizmoDrag {
                moved: true,
                ..drag.clone()
            });
        }
        for (what, delta) in &moves {
            move_by(scene, *what, *delta);
        }
        // A selection holds one kind, so this frame's edit reached the placements or the other
        // two lists and never both — which is the answer the application needs.
        self.water_touched = drag
            .origins
            .first()
            .is_some_and(|(what, _)| what.object().is_none());
        self.status = if drag.origins.len() > 1 {
            format!("moved {} along {}", drag.origins.len(), drag.axis.label())
        } else {
            format!("moved {:.2} m along {}", metres, drag.axis.label())
        };
        true
    }

    /// An asset carried out of the browser, or asked for by a double click. Returns whether
    /// the scene changed.
    ///
    /// The two explicit ways to place something, and the only two. Dropping is a press on a
    /// browser row that comes up over the world; double-clicking a row places at the middle
    /// of the view, because there is no cursor position in the world to use — the cursor is
    /// over the panel.
    fn update_placement(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
    ) -> bool {
        if std::mem::take(&mut self.place_at_centre) {
            let centre = (viewport.0 as f32 * 0.5, viewport.1 as f32 * 0.5);
            let position = engine_presentation::ui::targeting::ground_under_cursor(
                centre,
                view_projection,
                viewport,
            )
            .unwrap_or_else(|| {
                // Looking at the sky: the ground is not in front of the camera at all, so the
                // asset goes down where the camera is standing rather than nowhere.
                let ahead = self.camera.position + self.camera.forward() * 12.0;
                Vec3::new(ahead.x, 0.0, ahead.z)
            });
            self.placement = None;
            return self.place(scene, position);
        }

        let Some(drag) = self.placement else {
            return false;
        };
        if !ui_input.pointer_down && !ui_input.pointer_released {
            // The button came up in a frame this never saw. Nothing was carried anywhere.
            self.placement = None;
            return false;
        }
        if !ui_input.pointer_released {
            return false;
        }
        self.placement = None;
        if over_panel {
            // Dropped back on the panel: the row chose an asset and placed nothing.
            return false;
        }
        let travelled = {
            let (dx, dy) = (
                ui_input.pointer.0 - drag.from.0,
                ui_input.pointer.1 - drag.from.1,
            );
            (dx * dx + dy * dy).sqrt()
        };
        if travelled < self.scale.px(CLICK_SLOP_PIXELS) {
            // A click that started and ended in the same place is not a drag into the world.
            return false;
        }
        let Some(ground) = engine_presentation::ui::targeting::ground_under_cursor(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            self.status = "point at the ground to drop it".to_owned();
            return false;
        };
        self.place(scene, ground)
    }

    /// Puts the browser's asset into the scene at `position` and selects it.
    ///
    /// One function for both ways of placing, so a dropped asset and a double-clicked one land
    /// with the same collision, the same snapping and the same undo behind them.
    fn place(&mut self, scene: &mut EnvironmentScene, position: Vec3) -> bool {
        self.snapshot(scene);
        let asset = self.asset();
        // What it blocks is decided once, here, and written into the level. A built-in has an
        // authored primitive; a file has whatever its sidecar says, or a box around its
        // geometry. Several boxes are added to the scene's own shape table and named by
        // index, which is what lets a cliff overhang block a wall and a slab with walkable
        // space between them. The level carries it either way, so a placement keeps what it
        // was placed with even if the rule changes.
        let collision = match self.browser.selected_collision() {
            browser::PlacementCollision::Simple(collision) => collision,
            browser::PlacementCollision::Shape(shape) => scene.intern_collision(shape),
        };
        scene.push(EnvironmentObject {
            asset,
            transform: Transform::from_translation(self.snap(position)),
            collision,
        });
        self.selected.clear();
        self.selected
            .push(Selected::Object(scene.objects.len() - 1));
        self.status = format!("placed {}", asset.name());
        true
    }

    /// `position`, with the component `axis` runs along snapped to the grid.
    ///
    /// Only that component: a drag along Y that also rounded X and Z would move an object in
    /// two directions the author did not ask for, and a gizmo whose whole promise is "this
    /// drag changes one axis" cannot do that. Height *is* snapped here, unlike a placement —
    /// an author dragging the Y handle is deliberately choosing a height, and a stack of
    /// blockout pieces wants to land on the same steps as the floor plan.
    fn snap_along(&self, axis: gizmo::Axis, position: Vec3) -> Vec3 {
        let step = self.grid();
        if step <= 0.0 {
            return position;
        }
        let round = |value: f32| (value / step).round() * step;
        match axis {
            gizmo::Axis::X => Vec3::new(round(position.x), position.y, position.z),
            gizmo::Axis::Y => Vec3::new(position.x, round(position.y), position.z),
            gizmo::Axis::Z => Vec3::new(position.x, position.y, round(position.z)),
        }
    }

    /// A position on the grid, if there is one.
    ///
    /// Height is never snapped: everything in this game stands on the ground, and rounding
    /// `y` would lift a floor two centimetres above the plain onto it.
    fn snap(&self, position: Vec3) -> Vec3 {
        let step = self.grid();
        if step <= 0.0 {
            return position;
        }
        Vec3::new(
            (position.x / step).round() * step,
            position.y,
            (position.z / step).round() * step,
        )
    }

    /// Remembers the scene as it is now, for `Z`.
    fn snapshot(&mut self, scene: &EnvironmentScene) {
        let snapshot = Snapshot::of(scene);
        self.push_undo(snapshot);
    }

    /// Pushes a snapshot, dropping the oldest once the stack is full.
    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
    }

    /// Puts the last snapshot back. Returns whether anything changed.
    fn undo(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            self.status = "nothing to undo".to_owned();
            return false;
        };
        snapshot.restore(scene);
        self.selected.clear();
        self.cancel_manipulation();
        self.status = "undone".to_owned();
        true
    }

    /// Writes the scene, and says so in the status line.
    ///
    /// A failure is reported in the status line rather than returned: the author is standing
    /// in the level with the file dialog they do not have, and the useful thing is to see
    /// "could not save" on screen immediately. It is logged as an error too.
    fn save(&mut self, scene: &EnvironmentScene) {
        match scene_file::save(scene, &self.save_path) {
            Ok(()) => {
                log::info!(
                    "editor: wrote {} ({} objects) to {}",
                    scene.name,
                    scene.len(),
                    self.save_path.display()
                );
                self.status = format!("saved {}", self.save_path.display());
            }
            Err(error) => {
                log::error!("editor: could not save: {error}");
                self.status = format!("COULD NOT SAVE: {error}");
            }
        }
    }
}

/// Where something stands, whichever of the level's lists it lives in.
///
/// `None` for an index that is past the end of its list, which is what a drag whose target
/// was deleted under it looks like.
#[must_use]
fn position_of(scene: &EnvironmentScene, what: Selected) -> Option<Vec3> {
    match what {
        Selected::Object(index) => scene
            .objects
            .get(index)
            .map(|object| object.transform.translation),
        Selected::Water(index) => scene.water.get(index).map(|body| body.center),
        Selected::Spawn(index) => scene.spawn_points.get(index).map(|spawn| spawn.position),
    }
}

/// Moves something by `delta`, whichever list it lives in.
fn move_by(scene: &mut EnvironmentScene, what: Selected, delta: Vec3) {
    match what {
        Selected::Object(index) => {
            if let Some(object) = scene.objects.get_mut(index) {
                object.transform.translation += delta;
            }
        }
        Selected::Water(index) => {
            if let Some(body) = scene.water.get_mut(index) {
                body.center += delta;
            }
        }
        Selected::Spawn(index) => {
            if let Some(spawn) = scene.spawn_points.get_mut(index) {
                spawn.position += delta;
            }
        }
    }
}

/// How many rows the hierarchy has: every placement, and then every body of water.
///
/// Water after the objects rather than interleaved, because an object's row is its index into
/// the level's object list and renumbering that to make room for a pond would make the number
/// on screen disagree with the number in the file.
#[must_use]
pub fn hierarchy_len(scene: &EnvironmentScene) -> usize {
    scene.objects.len() + scene.water.len() + scene.spawn_points.len()
}

/// What the hierarchy's `row`th line names, if it names anything.
#[must_use]
pub fn hierarchy_row_at(row: usize, scene: &EnvironmentScene) -> Option<Selected> {
    if row < scene.objects.len() {
        return Some(Selected::Object(row));
    }
    let water = row - scene.objects.len();
    if water < scene.water.len() {
        return Some(Selected::Water(water));
    }
    let spawn = water - scene.water.len();
    (spawn < scene.spawn_points.len()).then_some(Selected::Spawn(spawn))
}

/// Which hierarchy row names `what`. The inverse of [`hierarchy_row_at`].
#[must_use]
pub fn hierarchy_row_of(what: Selected, scene: &EnvironmentScene) -> usize {
    match what {
        Selected::Object(index) => index,
        Selected::Water(index) => scene.objects.len() + index,
        Selected::Spawn(index) => scene.objects.len() + scene.water.len() + index,
    }
}

/// The nearest multiple of `step` to `angle`.
///
/// What makes rotation snapping mean "aligned with everything else" rather than "turned by
/// the same amount as everything else": an object nudged to 7 degrees by hand and then
/// stepped by 45 lands on 45, not on 52.
fn snap_angle(angle: f32, step: f32) -> f32 {
    if step <= 0.0 {
        return angle;
    }
    (angle / step).round() * step
}

/// How far a placement is turned about +Y.
pub(crate) fn yaw_of(transform: Transform) -> f32 {
    let rotation = transform.rotation;
    let siny = 2.0 * (rotation.w * rotation.y + rotation.z * rotation.x);
    let cosy = 1.0 - 2.0 * (rotation.x * rotation.x + rotation.y * rotation.y);
    siny.atan2(cosy)
}

/// Whether the editor should take over, given this frame's input.
///
/// `F6` both enters and leaves, so the application asks this before the editor exists and
/// [`Editor::update`] answers the same key while it does.
#[must_use]
pub fn wants_to_enter(input: &InputState) -> bool {
    input.was_key_pressed(KeyCode::F6)
}

/// Whether the mouse is doing something the game's own look control must not also do.
#[must_use]
pub fn owns_pointer(input: &InputState) -> bool {
    input.is_mouse_button_down(MouseButton::Right)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_environment::asset::{PILLAR, ROCK_SMALL};
    use engine_environment::object::default_collision;
    use engine_render_data::{Camera, Projection};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    /// A camera looking straight down from twenty metres, so a click in the middle of the
    /// screen lands on the origin and the arithmetic in a test is obvious.
    fn overhead() -> Mat4 {
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        camera.set_position(Vec3::new(0.0, 20.0, 0.0));
        camera
            .look_at(
                Vec3::new(0.0, 20.0, 0.0),
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -1.0),
            )
            .expect("a valid view");
        camera.view_projection()
    }

    fn scene() -> EnvironmentScene {
        EnvironmentScene::with_capacity("test_level", 4)
    }

    fn editor() -> Editor {
        let mut editor = Editor::new();
        editor.enter(Vec3::ZERO, "test_level", VIEWPORT);
        editor
    }

    /// A frame in which the pointer is at `pointer` and the left button went down.
    fn press(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        }
    }

    /// A frame in which the pointer is at `pointer` and the left button is held.
    fn hold(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_down: true,
            ..UiInput::new()
        }
    }

    fn keyed(key: KeyCode) -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown { key, repeat: false });
        input
    }

    /// Which placements are selected, as plain indices.
    ///
    /// The selection is tagged now — see [`Selected`] — and every assertion below is about
    /// objects, so this is where the tag is taken off rather than in twenty assertions.
    fn objects_of(editor: &Editor) -> Vec<usize> {
        editor.selected_objects()
    }

    fn centre() -> (f32, f32) {
        (VIEWPORT.0 as f32 / 2.0, VIEWPORT.1 as f32 / 2.0)
    }

    fn update(
        editor: &mut Editor,
        input: &InputState,
        ui: &UiInput,
        scene: &mut EnvironmentScene,
    ) -> EditorOutcome {
        editor.update(input, ui, scene, &overhead(), VIEWPORT, 1.0 / 60.0)
    }

    /// A frame answered against `view_projection` rather than the overhead camera.
    ///
    /// The overhead camera looks straight down, which points the Y axis at the eye: its
    /// handle is a dot, and a dot cannot be dragged. A test about the Y handle needs a camera
    /// that can see it.
    fn update_seen_from(
        editor: &mut Editor,
        ui: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
    ) -> EditorOutcome {
        editor.update(
            &InputState::new(),
            ui,
            scene,
            view_projection,
            VIEWPORT,
            1.0 / 60.0,
        )
    }

    /// A camera at the same height as the level, looking along -Z: it can see all three axes.
    fn side_on() -> Mat4 {
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        let eye = Vec3::new(6.0, 4.0, 22.0);
        camera.set_position(eye);
        camera
            .look_at(eye, Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0))
            .expect("a valid view");
        camera.view_projection()
    }

    /// A frame in which the button came up at `pointer`.
    fn release(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_released: true,
            ..UiInput::new()
        }
    }

    /// Where the browser row of the asset the editor would place sits, as a pointer position.
    ///
    /// The built-in library starts in a closed folder, so the row may not be listed yet: the
    /// folder is opened first, exactly as an author would open it.
    fn selected_row_point(editor: &mut Editor, scene: &mut EnvironmentScene) -> (f32, f32) {
        if let Some(point) = visible_selected_row(editor) {
            return point;
        }
        let folder = ui::browser_row_rect(editor.scale(), VIEWPORT, 0);
        update(
            editor,
            &InputState::new(),
            &press((
                folder.x + folder.width * 0.5,
                folder.y + folder.height * 0.5,
            )),
            scene,
        );
        visible_selected_row(editor).expect("the browser's own selection is not on screen")
    }

    /// The row rectangle of the browser's own selection, if it is listed.
    fn visible_selected_row(editor: &Editor) -> Option<(f32, f32)> {
        let visible = ui::browser_visible_rows(editor.scale(), VIEWPORT);
        let first = editor.browser().scroll();
        for slot in 0..visible {
            let Some(row) = editor.browser().row(first + slot) else {
                break;
            };
            if editor.browser().is_selected(row) {
                let rect = ui::browser_row_rect(editor.scale(), VIEWPORT, slot);
                return Some((rect.x + rect.width * 0.5, rect.y + rect.height * 0.5));
            }
        }
        None
    }

    /// Drags the browser's current asset out of the panel and drops it at `pointer`.
    ///
    /// The gesture the editor now requires: a press on a row and a release out in the world.
    /// Every test that used to place with one click in the middle of the screen places with
    /// this, which is the point of the change — a click in the world never places anything.
    fn drop_asset(
        editor: &mut Editor,
        scene: &mut EnvironmentScene,
        pointer: (f32, f32),
    ) -> EditorOutcome {
        let row = selected_row_point(editor, scene);
        update(editor, &InputState::new(), &press(row), scene);
        update(editor, &InputState::new(), &release(pointer), scene)
    }

    /// Presses on a gizmo handle and drags it by `by` pixels. Returns the last outcome.
    fn drag_handle(
        editor: &mut Editor,
        scene: &mut EnvironmentScene,
        axis: gizmo::Axis,
        by: (f32, f32),
    ) -> EditorOutcome {
        let handle = editor
            .gizmo(scene)
            .and_then(|screen| screen.handle(axis))
            .expect("a handle for this axis");
        let grab = (
            (handle.from.0 + handle.to.0) * 0.5,
            (handle.from.1 + handle.to.1) * 0.5,
        );
        update(editor, &InputState::new(), &press(grab), scene);
        update(
            editor,
            &InputState::new(),
            &hold((grab.0 + by.0, grab.1 + by.1)),
            scene,
        )
    }

    /// A frame whose wheel turned by `notches`.
    fn wheeled(notches: f32) -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::MouseWheel { delta: notches });
        input
    }

    /// The middle of hierarchy row `slot`.
    fn hierarchy_point(editor: &Editor, slot: usize) -> (f32, f32) {
        let rect = ui::hierarchy_row_rect(editor.scale(), VIEWPORT, slot);
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// A scene holding `count` rocks in a row, one metre apart and well away from the origin
    /// so that a click in the middle of the screen lands on empty ground.
    fn scene_of(count: usize) -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("test_level", count);
        for index in 0..count {
            scene.push(EnvironmentObject::decoration(
                ROCK_SMALL,
                Vec3::new(200.0 + index as f32, 0.0, 0.0),
            ));
        }
        scene
    }

    /// A frame in which the pointer is at `pointer` and no button is down.
    fn hover(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            ..UiInput::new()
        }
    }

    /// A frame in which `key` went down and the pointer is at `pointer`.
    fn keyed_at(key: KeyCode, pointer: (f32, f32)) -> (InputState, UiInput) {
        (keyed(key), hover(pointer))
    }

    /// An editor with one body of water in the level, poured at the origin.
    fn poured() -> (Editor, EnvironmentScene) {
        let mut editor = editor();
        let mut scene = scene();
        let (input, ui) = keyed_at(KeyCode::V, centre());
        let outcome = update(&mut editor, &input, &ui, &mut scene);
        assert_eq!(outcome, EditorOutcome::WaterChanged);
        (editor, scene)
    }

    #[test]
    fn choosing_a_cave_piece_snaps_to_the_module_grid_until_the_author_says_otherwise() {
        // Modular pieces meet on a face. Two tunnels four metres apart on a quarter-turn grid
        // join; two laid on a 22.5 degree grid leave a gap only visible from inside the cave.
        assert!(Editor::is_module_piece(
            "generated/environment/caves/cave_tunnel_straight"
        ));
        assert!(Editor::is_module_piece("cave_room_large"));
        assert!(!Editor::is_module_piece(
            "generated/environment/rocks/rock_small"
        ));

        let mut editor = editor();
        assert!(
            !editor.is_module_snap(),
            "a rock does not want the cave grid"
        );

        // While it is on, the grid and the rotation step are the module's, whatever `G` last
        // cycled to: that is the whole of what the mode does.
        editor.module_snap = true;
        assert!((editor.grid() - MODULE_GRID).abs() < 1e-6);
        assert!((editor.yaw_step() - MODULE_YAW).abs() < 1e-6);

        // `M` takes the decision off the editor for good. The browser has a rock selected, so
        // following it would turn the mode off; after `M` it must not, in either direction.
        let mut scene = scene();
        update(&mut editor, &keyed(KeyCode::M), &UiInput::new(), &mut scene);
        assert!(editor.module_snap_pinned, "M did not pin the choice");
        let pinned_to = editor.is_module_snap();
        editor.follow_module_snap();
        assert_eq!(
            editor.is_module_snap(),
            pinned_to,
            "the browser overrode an author who had already decided"
        );
    }

    #[test]
    fn v_pours_a_body_of_water_where_the_cursor_is_and_z_takes_it_back() {
        let (mut editor, mut scene) = poured();
        assert_eq!(scene.water.len(), 1);
        let body = scene.water[0];
        assert!(
            body.center.x.abs() < 0.01 && body.center.z.abs() < 0.01,
            "{body:?}"
        );
        assert!(body.is_valid(), "a poured body is a volume: {body:?}");
        assert_eq!(
            editor.selection(),
            Some(Selected::Water(0)),
            "what was just poured is what the next verb acts on"
        );

        // Pouring is an edit like any other, so it is on the undo stack. Phase 6 learned this
        // about brush strokes and the lesson is the same one.
        update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert!(scene.water.is_empty(), "the pour was not undoable");
    }

    #[test]
    fn a_body_of_water_moves_grows_turns_and_is_deleted_by_the_keys_that_do_that() {
        let (mut editor, mut scene) = poured();

        // Grow, then turn, then nudge, then delete: the four verbs, in the order an author
        // would reach for them.
        let before = scene.water[0].half_extents;
        let outcome = update(&mut editor, &keyed(KeyCode::T), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::WaterChanged);
        assert!(
            scene.water[0].half_extents.x > before.x && scene.water[0].half_extents.y > before.y,
            "T did not grow the body"
        );

        update(&mut editor, &keyed(KeyCode::R), &UiInput::new(), &mut scene);
        assert!(
            (scene.water[0].yaw - editor.yaw_step().to_radians()).abs() < 1e-4,
            "R turned it by {} rad",
            scene.water[0].yaw
        );

        let center = scene.water[0].center;
        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        assert!(
            scene.water[0].center.x > center.x,
            "an arrow key did not move the body"
        );

        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert!(scene.water.is_empty(), "X did not delete the body");
        assert!(editor.selection().is_none());
    }

    #[test]
    fn a_body_of_water_never_shrinks_to_something_the_loader_would_refuse() {
        let (mut editor, mut scene) = poured();
        let mut input = keyed(KeyCode::T);
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftShift,
            repeat: false,
        });
        // Far more presses than it takes to reach the floor, because the assertion is about
        // the floor and not about how many steps away it was.
        for _ in 0..200 {
            update(&mut editor, &input, &UiInput::new(), &mut scene);
        }
        assert!(
            scene.water[0].is_valid(),
            "shrinking made a body no level could hold: {:?}",
            scene.water[0]
        );
    }

    #[test]
    fn dragging_a_body_of_water_moves_it_and_leaves_the_placements_alone() {
        let mut editor = editor();
        let mut scene = scene();
        // A rock well away from the middle of the screen, so the drag below cannot reach it.
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(200.0, 0.0, 0.0),
        ));
        let (input, ui) = keyed_at(KeyCode::V, centre());
        update(&mut editor, &input, &ui, &mut scene);

        let rock = scene.objects[0].transform.translation;
        let before = scene.water[0].center;

        // Select the body in the middle of the screen, then drag its X handle.
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(editor.selected_water(), Some(0), "the click selected it");
        let outcome = drag_handle(&mut editor, &mut scene, gizmo::Axis::X, (140.0, 0.0));

        assert_eq!(outcome, EditorOutcome::WaterChanged);
        assert!(
            (scene.water[0].center - before).length() > 0.5,
            "the drag did not move the body"
        );
        assert_eq!(
            scene.objects[0].transform.translation, rock,
            "dragging water moved a placement"
        );
    }

    #[test]
    fn p_puts_a_spawn_point_down_and_every_verb_reaches_it() {
        let mut editor = editor();
        let mut scene = scene();
        let (input, ui) = keyed_at(KeyCode::P, centre());
        let outcome = update(&mut editor, &input, &ui, &mut scene);

        assert_eq!(outcome, EditorOutcome::WaterChanged);
        assert_eq!(scene.spawn_points.len(), 1);
        assert_eq!(editor.selection(), Some(Selected::Spawn(0)));
        assert!(scene.spawn_points[0].is_valid());

        // Turning it is the one thing about a spawn point that is not obvious from where it
        // stands, so it is the verb worth asserting first.
        update(&mut editor, &keyed(KeyCode::R), &UiInput::new(), &mut scene);
        assert!(
            (scene.spawn_points[0].yaw - editor.yaw_step().to_radians()).abs() < 1e-4,
            "R did not turn the spawn point: {}",
            scene.spawn_points[0].yaw
        );

        // Straightened, nudged, and taken away again.
        update(
            &mut editor,
            &keyed(KeyCode::Digit0),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.spawn_points[0].yaw, 0.0);

        let before = scene.spawn_points[0].position;
        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        assert!(scene.spawn_points[0].position.x > before.x);

        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert!(scene.spawn_points.is_empty(), "X did not delete it");

        // And putting one down is an edit like any other.
        update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert_eq!(scene.spawn_points.len(), 1, "the delete was not undoable");
    }

    #[test]
    fn a_second_spawn_point_faces_the_way_the_last_one_did() {
        // An author laying out a row of starts is nearly always laying out a row that faces
        // the same way, and turning each one by hand is the kind of work a tool should not ask
        // for.
        let mut editor = editor();
        let mut scene = scene();
        let (input, ui) = keyed_at(KeyCode::P, centre());
        update(&mut editor, &input, &ui, &mut scene);
        update(&mut editor, &keyed(KeyCode::R), &UiInput::new(), &mut scene);
        let turned = scene.spawn_points[0].yaw;

        update(&mut editor, &input, &ui, &mut scene);
        assert_eq!(scene.spawn_points.len(), 2);
        assert!(
            (scene.spawn_points[1].yaw - turned).abs() < 1e-6,
            "the second spawn point faced {} rather than {turned}",
            scene.spawn_points[1].yaw
        );
    }

    #[test]
    fn a_spawn_point_and_a_body_of_water_are_told_apart_by_every_verb() {
        // The third kind through the tagged selection, and the assertion that adding one did
        // not let a verb reach across the kinds.
        let mut editor = editor();
        let mut scene = scene();
        let (pour, ui) = keyed_at(KeyCode::V, centre());
        update(&mut editor, &pour, &ui, &mut scene);
        let (put, _) = keyed_at(KeyCode::P, centre());
        update(&mut editor, &put, &ui, &mut scene);

        assert_eq!(editor.selection(), Some(Selected::Spawn(0)));
        assert!(
            editor.selected_waters().is_empty(),
            "putting a spawn point kept the water selected"
        );

        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert!(scene.spawn_points.is_empty(), "X did not delete the spawn");
        assert_eq!(scene.water.len(), 1, "X reached the water as well");
    }

    #[test]
    fn a_verb_reaches_the_kind_that_is_selected_and_never_the_other() {
        // The assertion the tagged selection exists for: one object and one body of water in
        // the same level, each selectable, and `X` deleting exactly the one that is.
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(200.0, 0.0, 0.0),
        ));
        let (input, ui) = keyed_at(KeyCode::V, centre());
        update(&mut editor, &input, &ui, &mut scene);
        assert_eq!(editor.selection(), Some(Selected::Water(0)));

        // Selecting the rock from the hierarchy clears the water selection rather than
        // adding to it: a selection holds one kind at a time.
        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);
        assert_eq!(editor.selection(), Some(Selected::Object(0)));
        assert!(editor.selected_waters().is_empty());

        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert!(scene.objects.is_empty(), "X did not delete the rock");
        assert_eq!(scene.water.len(), 1, "X reached the water as well");

        // And the other way: the water row is the one after the objects, and deleting from
        // there leaves the level's placements alone.
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(200.0, 0.0, 0.0),
        ));
        let water_row = hierarchy_point(&editor, hierarchy_row_of(Selected::Water(0), &scene));
        update(
            &mut editor,
            &InputState::new(),
            &press(water_row),
            &mut scene,
        );
        assert_eq!(editor.selection(), Some(Selected::Water(0)));
        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert!(scene.water.is_empty(), "X did not delete the body");
        assert_eq!(scene.objects.len(), 1, "X reached a placement as well");
    }

    #[test]
    fn control_a_takes_every_object_and_no_water() {
        let mut editor = editor();
        let mut scene = scene_of(3);
        let (input, ui) = keyed_at(KeyCode::V, centre());
        update(&mut editor, &input, &ui, &mut scene);

        let mut input = keyed(KeyCode::A);
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftControl,
            repeat: false,
        });
        update(&mut editor, &input, &UiInput::new(), &mut scene);
        assert_eq!(objects_of(&editor), [0, 1, 2]);
        assert!(
            editor.selected_waters().is_empty(),
            "CTRL+A put two kinds in one selection"
        );
    }

    #[test]
    fn a_click_on_a_hierarchy_row_selects_that_object_and_places_nothing() {
        let mut editor = editor();
        let mut scene = scene_of(4);
        let point = hierarchy_point(&editor, 2);

        let outcome = update(&mut editor, &InputState::new(), &press(point), &mut scene);

        assert_eq!(editor.selected_object(), Some(2));
        assert_eq!(
            scene.len(),
            4,
            "the click through the panel placed an object"
        );
        assert_eq!(outcome, EditorOutcome::Idle);
    }

    #[test]
    fn the_hierarchy_scrolls_by_the_wheel_and_stops_at_both_ends() {
        let mut editor = editor();
        let visible = ui::hierarchy_visible_rows(editor.scale(), VIEWPORT);
        let mut scene = scene_of(visible + 10);
        let over = hierarchy_point(&editor, 0);
        let hover = UiInput {
            pointer: over,
            ..UiInput::new()
        };

        // Down four notches, then far more than the list is long.
        update(&mut editor, &wheeled(-4.0), &hover, &mut scene);
        assert_eq!(editor.hierarchy_scroll(), 4);
        update(&mut editor, &wheeled(-1_000.0), &hover, &mut scene);
        assert_eq!(
            editor.hierarchy_scroll(),
            scene.len() - visible,
            "the list scrolled past its own end"
        );
        update(&mut editor, &wheeled(1_000.0), &hover, &mut scene);
        assert_eq!(editor.hierarchy_scroll(), 0);
    }

    #[test]
    fn scrolling_the_hierarchy_selects_the_row_that_is_actually_under_the_cursor() {
        let mut editor = editor();
        let visible = ui::hierarchy_visible_rows(editor.scale(), VIEWPORT);
        let mut scene = scene_of(visible + 5);
        let over = hierarchy_point(&editor, 1);
        let hover = UiInput {
            pointer: over,
            ..UiInput::new()
        };
        update(&mut editor, &wheeled(-3.0), &hover, &mut scene);

        update(&mut editor, &InputState::new(), &press(over), &mut scene);
        assert_eq!(
            editor.selected_object(),
            Some(4),
            "row 1 of a list scrolled by 3"
        );
    }

    #[test]
    fn the_wheel_outside_the_panel_scrolls_nothing() {
        let mut editor = editor();
        let visible = ui::hierarchy_visible_rows(editor.scale(), VIEWPORT);
        let mut scene = scene_of(visible + 10);
        let hover = UiInput {
            pointer: centre(),
            ..UiInput::new()
        };
        update(&mut editor, &wheeled(-5.0), &hover, &mut scene);
        assert_eq!(editor.hierarchy_scroll(), 0);
    }

    #[test]
    fn a_new_selection_below_the_window_scrolls_it_into_view() {
        let mut editor = editor();
        let visible = ui::hierarchy_visible_rows(editor.scale(), VIEWPORT);
        let mut scene = scene_of(visible + 6);
        // Place one more: it becomes the last object and the selection.
        drop_asset(&mut editor, &mut scene, centre());
        let last = scene.len() - 1;
        assert_eq!(editor.selected_object(), Some(last));
        assert!(
            editor.hierarchy_scroll() + visible > last,
            "the selected object is off the bottom of the list"
        );
    }

    #[test]
    fn a_closed_editor_ignores_everything() {
        let mut editor = Editor::new();
        let mut scene = scene();
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::Idle);
        assert!(scene.is_empty(), "a closed editor placed something");
    }

    #[test]
    fn a_click_on_empty_ground_places_nothing_and_drops_the_selection() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(60.0, 0.0, 0.0),
        ));
        editor.select(Selected::Object(0), false);

        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::Idle);
        assert_eq!(scene.len(), 1, "a click on the ground placed something");
        assert_eq!(
            editor.selection(),
            None,
            "and it did not drop the selection"
        );
    }

    #[test]
    fn an_asset_dragged_out_of_the_browser_lands_where_it_was_dropped() {
        let mut editor = editor();
        let mut scene = scene();

        let outcome = drop_asset(&mut editor, &mut scene, centre());

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 1, "exactly one placement per drop");
        assert_eq!(scene.objects[0].asset, editor.asset());
        let position = scene.objects[0].position();
        assert!(
            position.x.abs() < 0.5 && position.z.abs() < 0.5,
            "it went where it was dropped: {position:?}"
        );
        assert_eq!(editor.selected_object(), Some(0), "and it is selected");
    }

    #[test]
    fn a_press_on_a_browser_row_that_comes_up_on_the_panel_places_nothing() {
        let mut editor = editor();
        let mut scene = scene();
        let row = selected_row_point(&mut editor, &mut scene);

        update(&mut editor, &InputState::new(), &press(row), &mut scene);
        let outcome = update(&mut editor, &InputState::new(), &release(row), &mut scene);

        assert_eq!(outcome, EditorOutcome::Idle);
        assert!(
            scene.is_empty(),
            "a click that never left the browser placed something"
        );
    }

    #[test]
    fn a_double_click_on_a_browser_row_places_one_in_front_of_the_camera() {
        let mut editor = editor();
        let mut scene = scene();
        let row = selected_row_point(&mut editor, &mut scene);

        update(&mut editor, &InputState::new(), &press(row), &mut scene);
        update(&mut editor, &InputState::new(), &release(row), &mut scene);
        let outcome = update(&mut editor, &InputState::new(), &press(row), &mut scene);

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 1, "the second click placed one");
        assert_eq!(editor.selected_object(), Some(0));
    }

    #[test]
    fn two_clicks_far_apart_in_time_are_not_a_double_click() {
        let mut editor = editor();
        let mut scene = scene();
        let row = selected_row_point(&mut editor, &mut scene);

        update(&mut editor, &InputState::new(), &press(row), &mut scene);
        update(&mut editor, &InputState::new(), &release(row), &mut scene);
        // A second of frames in between, which is longer than the double-click interval.
        for _ in 0..60 {
            update(&mut editor, &InputState::new(), &UiInput::new(), &mut scene);
        }
        update(&mut editor, &InputState::new(), &press(row), &mut scene);

        assert!(scene.is_empty(), "two slow clicks placed something");
    }

    #[test]
    fn a_placed_object_blocks_what_the_library_says_it_blocks() {
        let mut editor = editor();
        let mut scene = scene();
        // The browser starts on ground_plain, which blocks nothing; TAB along to a pillar.
        // Bounded, so a TAB that stops moving fails the test instead of hanging it.
        let mut presses = 0;
        while editor.asset() != PILLAR {
            update(
                &mut editor,
                &keyed(KeyCode::Tab),
                &UiInput::new(),
                &mut scene,
            );
            presses += 1;
            assert!(presses < 256, "TAB never reached the pillar");
        }
        drop_asset(&mut editor, &mut scene, centre());
        assert!(
            scene.objects[0].collision.blocks(),
            "a pillar placed in the editor is a pillar you cannot walk through"
        );
    }

    #[test]
    fn the_gizmo_is_there_only_while_something_is_selected() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );

        update(&mut editor, &InputState::new(), &UiInput::new(), &mut scene);
        assert!(
            editor.gizmo(&scene).is_none(),
            "a gizmo with nothing selected is a handle nobody can explain"
        );

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let screen = editor.gizmo(&scene).expect("a gizmo on the selection");
        assert!(
            screen.handle(gizmo::Axis::X).is_some() && screen.handle(gizmo::Axis::Z).is_some(),
            "the gizmo has no handles to grab"
        );

        update(
            &mut editor,
            &keyed(KeyCode::Escape),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.gizmo(&scene).is_none(), "it outlived the selection");
    }

    #[test]
    fn the_gizmo_follows_the_object_it_is_standing_on() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let before = editor.gizmo_origin(&scene).expect("an origin");

        drag_handle(&mut editor, &mut scene, gizmo::Axis::X, (150.0, 0.0));
        let after = editor.gizmo_origin(&scene).expect("an origin");

        assert_eq!(after, scene.objects[0].position());
        assert!(after.x > before.x, "the gizmo stayed behind: {after:?}");
    }

    #[test]
    fn a_press_on_a_handle_that_never_drags_it_changes_nothing() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let before = scene.objects.clone();

        let handle = editor
            .gizmo(&scene)
            .and_then(|screen| screen.handle(gizmo::Axis::X))
            .expect("an X handle");
        let grab = (
            (handle.from.0 + handle.to.0) * 0.5,
            (handle.from.1 + handle.to.1) * 0.5,
        );
        let outcome = update(&mut editor, &InputState::new(), &press(grab), &mut scene);
        assert_eq!(outcome, EditorOutcome::Idle);
        // The button comes up without the pointer ever moving.
        update(&mut editor, &InputState::new(), &release(grab), &mut scene);

        assert_eq!(scene.objects, before, "a click on a handle moved something");
        let outcome = update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::Idle, "and it filled the undo stack");
    }

    #[test]
    fn a_handle_beats_the_object_underneath_it() {
        let mut editor = editor();
        let mut scene = scene();
        // A second placement out along +X, under the X handle, so a press on that handle is
        // also a press on something selectable.
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        scene.push(
            EnvironmentObject::decoration(ROCK_SMALL, Vec3::new(2.5, 0.0, 0.0))
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(editor.selected_object(), Some(0));

        drag_handle(&mut editor, &mut scene, gizmo::Axis::X, (140.0, 0.0));

        assert_eq!(
            editor.selected_object(),
            Some(0),
            "the press selected what was under the handle instead of grabbing it"
        );
        assert!(
            scene.objects[0].position().x > 1.0,
            "the pillar did not move"
        );
    }

    #[test]
    fn the_y_handle_lifts_the_selection_and_leaves_the_floor_plan_alone() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        let seen_from = side_on();
        editor.select(Selected::Object(0), false);
        update_seen_from(&mut editor, &UiInput::new(), &mut scene, &seen_from);

        let handle = editor
            .gizmo(&scene)
            .and_then(|screen| screen.handle(gizmo::Axis::Y))
            .expect("a Y handle");
        let grab = (
            (handle.from.0 + handle.to.0) * 0.5,
            (handle.from.1 + handle.to.1) * 0.5,
        );
        update_seen_from(&mut editor, &press(grab), &mut scene, &seen_from);
        // Up the screen is up the Y axis.
        let outcome = update_seen_from(
            &mut editor,
            &hold((grab.0, grab.1 - 120.0)),
            &mut scene,
            &seen_from,
        );

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        let moved = scene.objects[0].position();
        assert!(moved.y > 0.5, "the pillar did not rise: {moved:?}");
        assert!(
            moved.x.abs() < 1e-3 && moved.z.abs() < 1e-3,
            "a Y drag moved it across the ground: {moved:?}"
        );
    }

    #[test]
    fn a_gizmo_drag_keeps_the_pointer_when_it_passes_over_a_panel() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let handle = editor
            .gizmo(&scene)
            .and_then(|screen| screen.handle(gizmo::Axis::X))
            .expect("an X handle");
        let grab = (
            (handle.from.0 + handle.to.0) * 0.5,
            (handle.from.1 + handle.to.1) * 0.5,
        );
        update(&mut editor, &InputState::new(), &press(grab), &mut scene);

        // Left, over the browser panel, and still dragging.
        let panel = ui::browser_rect(editor.scale(), VIEWPORT);
        let over_panel = (panel.x + panel.width * 0.5, panel.y + panel.height * 0.5);
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &hold(over_panel),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert!(
            scene.objects[0].position().x < -0.5,
            "the drag stopped at the edge of the panel: {:?}",
            scene.objects[0].position()
        );
        assert_eq!(scene.len(), 1, "and the panel placed something mid-drag");
    }

    #[test]
    fn turning_the_camera_never_moves_the_selection() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let before = scene.objects.clone();

        // The right button turns the camera; the left one is what a gizmo answers.
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::MouseButtonDown {
            button: MouseButton::Right,
        });
        input.apply(engine_core::input::InputEvent::MouseMoved { x: 400, y: 260 });
        let outcome = update(&mut editor, &input, &UiInput::new(), &mut scene);

        assert_eq!(outcome, EditorOutcome::Idle);
        assert_eq!(scene.objects, before, "a camera turn moved the level");
    }

    #[test]
    fn placing_mode_puts_one_down_on_every_click_and_never_selects() {
        let mut editor = editor();
        let mut scene = scene();

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        assert!(editor.is_place_mode(), "N did not turn placing mode on");

        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 1, "the click placed nothing");

        // A second click on the same spot, which is now on top of the first object: it
        // places another rather than selecting the one that is there.
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(
            scene.len(),
            2,
            "the second click selected instead of placing"
        );
        assert_eq!(editor.selected_object(), Some(1), "the new one is selected");
    }

    #[test]
    fn placing_mode_never_moves_what_is_already_there() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        let before = scene.objects[0];

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        assert!(
            editor.gizmo(&scene).is_none(),
            "placing mode still offers a gizmo to drag"
        );

        // A press and a drag across the screen: in select mode this is how an object moves.
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let (x, y) = centre();
        update(
            &mut editor,
            &InputState::new(),
            &hold((x + 200.0, y)),
            &mut scene,
        );

        assert_eq!(
            scene.objects[0], before,
            "placing mode moved the object that was already there"
        );
    }

    #[test]
    fn leaving_placing_mode_gives_the_left_button_back_to_selection() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        assert!(!editor.is_place_mode(), "N did not turn placing mode off");

        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::Idle, "the click placed something");
        assert_eq!(scene.len(), 1);
        assert_eq!(
            editor.selected_object(),
            Some(0),
            "the click did not select"
        );
        assert!(
            editor.gizmo(&scene).is_some(),
            "the gizmo did not come back"
        );
    }

    #[test]
    fn escape_leaves_placing_mode() {
        let mut editor = editor();
        let mut scene = scene();

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        update(
            &mut editor,
            &keyed(KeyCode::Escape),
            &UiInput::new(),
            &mut scene,
        );

        assert!(!editor.is_place_mode(), "escape left the editor placing");
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert!(scene.is_empty(), "a click after escape placed something");
    }

    #[test]
    fn placing_and_sculpting_are_never_both_on() {
        let mut editor = editor();
        let mut scene = scene();

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        assert!(editor.is_sculpting());
        assert!(!editor.is_place_mode(), "the brush left placing mode on");

        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);
        assert!(editor.is_place_mode());
        assert!(!editor.is_sculpting(), "placing mode left the brush on");
    }

    #[test]
    fn placing_mode_ignores_a_click_on_a_panel() {
        let mut editor = editor();
        let mut scene = scene();
        update(&mut editor, &keyed(KeyCode::N), &UiInput::new(), &mut scene);

        let panel = ui::hierarchy_rect(editor.scale(), VIEWPORT);
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press((panel.x + panel.width * 0.5, panel.y + panel.height * 0.5)),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::Idle);
        assert!(
            scene.is_empty(),
            "a click on the hierarchy placed an object"
        );
    }

    #[test]
    fn a_click_on_an_object_selects_it_instead_of_placing_another() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );

        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::Idle, "selecting is not a change");
        assert_eq!(scene.len(), 1, "nothing was placed on top of it");
        assert_eq!(editor.selected_object(), Some(0));
    }

    #[test]
    fn dragging_a_handle_moves_the_selected_object_and_nothing_else() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(30.0, 0.0, 30.0),
        ));
        let untouched = scene.objects[1];

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(scene.len(), 2, "the click on the pillar placed something");
        let outcome = drag_handle(&mut editor, &mut scene, gizmo::Axis::X, (160.0, 0.0));

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        let moved = scene.objects[0].position();
        assert!(
            moved.x > 1.0,
            "the pillar did not follow the handle: {moved:?}"
        );
        assert!(
            moved.z.abs() < 1e-3 && moved.y.abs() < 1e-3,
            "an X drag moved it off its own axis: {moved:?}"
        );
        assert_eq!(scene.objects[1], untouched, "the rock did not move");
    }

    #[test]
    fn a_drag_snaps_to_the_grid_when_one_is_on() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        assert_eq!(editor.grid(), 1.0, "the editor starts on a one metre grid");

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let (x, y) = centre();
        update(
            &mut editor,
            &InputState::new(),
            &hold((x + 173.0, y + 91.0)),
            &mut scene,
        );

        let position = scene.objects[0].position();
        assert!(
            (position.x - position.x.round()).abs() < 1e-4
                && (position.z - position.z.round()).abs() < 1e-4,
            "{position:?} is not on the grid"
        );
        assert_eq!(position.y, 0.0, "height is never snapped");
    }

    #[test]
    fn x_deletes_the_selection_and_does_nothing_without_one() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(ROCK_SMALL, Vec3::ZERO));

        let outcome = update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::Idle, "nothing was selected");
        assert_eq!(scene.len(), 1);

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let outcome = update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert!(scene.is_empty());
        assert_eq!(editor.selected_object(), None, "the selection went with it");
    }

    #[test]
    fn undo_puts_back_what_the_last_verb_changed() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(9.0, 0.0, 9.0),
        ));
        let before = scene.objects.clone();

        drop_asset(&mut editor, &mut scene, centre());
        assert_eq!(scene.len(), 2, "a placement happened");

        let outcome = update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.objects, before, "the placement was taken back");

        // And an empty stack is not an error.
        let outcome = update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::Idle);
        assert_eq!(scene.objects, before);
    }

    #[test]
    fn r_and_t_turn_and_resize_the_selection() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );

        update(&mut editor, &keyed(KeyCode::R), &UiInput::new(), &mut scene);
        let turned = scene.objects[0].transform.rotation;
        assert!(
            turned.rotate(Vec3::new(1.0, 0.0, 0.0)).z.abs() > 1e-3,
            "R turned nothing"
        );

        let before = scene.objects[0].transform.scale.y;
        update(&mut editor, &keyed(KeyCode::T), &UiInput::new(), &mut scene);
        assert!(scene.objects[0].transform.scale.y > before, "T grew it");
    }

    #[test]
    fn a_click_on_a_browser_row_chooses_an_asset_and_places_nothing() {
        let mut editor = editor();
        let mut scene = scene();
        editor.set_scale(UiScale::default(), VIEWPORT);

        // Row 0 is the closed `builtin` folder: click it to open it, and rows 1.. are then
        // the assets inside, so row 4 is the fourth entry of the compiled library.
        let folder = ui::browser_row_rect(editor.scale(), VIEWPORT, 0);
        update(
            &mut editor,
            &InputState::new(),
            &press((
                folder.x + folder.width * 0.5,
                folder.y + folder.height * 0.5,
            )),
            &mut scene,
        );
        let rect = ui::browser_row_rect(editor.scale(), VIEWPORT, 4);
        let pointer = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        update(&mut editor, &InputState::new(), &press(pointer), &mut scene);

        assert_eq!(
            editor.asset(),
            engine_environment::asset::LIBRARY[3],
            "the browser chose the asset"
        );
        assert!(scene.is_empty(), "and put nothing in the world");
    }

    #[test]
    fn a_click_on_a_browser_folder_opens_it_instead_of_choosing_anything() {
        let mut editor = editor();
        let mut scene = scene();
        editor.set_scale(UiScale::default(), VIEWPORT);
        let before = editor.browser().rows().len();

        let rect = ui::browser_row_rect(editor.scale(), VIEWPORT, 0);
        let pointer = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        update(&mut editor, &InputState::new(), &press(pointer), &mut scene);

        assert!(
            editor.browser().rows().len() > before,
            "clicking the builtin folder did not open it"
        );
        assert!(scene.is_empty());
    }

    #[test]
    fn the_browsers_filter_owns_the_keyboard_while_it_is_open() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(ROCK_SMALL, Vec3::ZERO));

        // Select the rock, then open the filter and type an X. X deletes a selection when the
        // filter is closed, and must not while it is open.
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(editor.selected_object(), Some(0));

        update(&mut editor, &keyed(KeyCode::F), &UiInput::new(), &mut scene);
        assert!(editor.browser().is_typing());
        let outcome = update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);

        assert_eq!(outcome, EditorOutcome::Idle);
        assert_eq!(scene.len(), 1, "X deleted an object while typing a filter");
        assert_eq!(editor.browser().filter(), "x");
    }

    /// A frame in which `key` went down with control held.
    fn control_keyed(key: KeyCode) -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftControl,
            repeat: false,
        });
        input.apply(engine_core::input::InputEvent::KeyDown { key, repeat: false });
        input
    }

    /// A frame in which `key` went down with shift held.
    fn shift_keyed(key: KeyCode) -> InputState {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftShift,
            repeat: false,
        });
        input.apply(engine_core::input::InputEvent::KeyDown { key, repeat: false });
        input
    }

    /// A scene with three rocks in a row along X, at z = 0.
    fn three_rocks() -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("test_level", 3);
        for index in 0..3 {
            scene.push(EnvironmentObject::decoration(
                ROCK_SMALL,
                Vec3::new(index as f32 * 4.0, 0.0, 0.0),
            ));
        }
        scene
    }

    #[test]
    fn control_clicking_a_hierarchy_row_adds_to_the_selection_and_takes_away_again() {
        let mut editor = editor();
        let mut scene = three_rocks();

        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);
        assert_eq!(objects_of(&editor), [0]);

        let second = hierarchy_point(&editor, 2);
        let control = {
            let mut input = InputState::new();
            input.apply(engine_core::input::InputEvent::KeyDown {
                key: KeyCode::LeftControl,
                repeat: false,
            });
            input
        };
        update(&mut editor, &control, &press(second), &mut scene);
        assert_eq!(objects_of(&editor), [0, 2], "control did not add");
        assert_eq!(
            editor.selected_object(),
            Some(2),
            "the last one clicked is primary"
        );

        update(&mut editor, &control, &press(second), &mut scene);
        assert_eq!(
            objects_of(&editor),
            [0],
            "control did not toggle it back off"
        );
    }

    #[test]
    fn control_a_selects_everything_and_escape_drops_it() {
        let mut editor = editor();
        let mut scene = three_rocks();

        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(objects_of(&editor), [0, 1, 2]);

        update(
            &mut editor,
            &keyed(KeyCode::Escape),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.selected().is_empty());
    }

    #[test]
    fn a_verb_acts_on_every_selected_object() {
        let mut editor = editor();
        let mut scene = three_rocks();
        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );

        // Scale: all three grow.
        update(&mut editor, &keyed(KeyCode::T), &UiInput::new(), &mut scene);
        for object in &scene.objects {
            assert!(object.transform.scale.y > 1.0, "one was left behind");
        }

        // Duplicate: three copies, and the copies are what is selected.
        let outcome = update(&mut editor, &keyed(KeyCode::C), &UiInput::new(), &mut scene);
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 6);
        assert_eq!(objects_of(&editor), [3, 4, 5]);

        // Delete: the copies go and the originals stay.
        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert_eq!(scene.len(), 3);
        assert!(editor.selected().is_empty());
    }

    #[test]
    fn deleting_several_objects_removes_the_ones_that_were_selected() {
        let mut editor = editor();
        let mut scene = three_rocks();
        // The outer two, leaving the middle one behind: the case that goes wrong when indices
        // are removed lowest-first.
        scene.objects[1].transform.scale = Vec3::splat(2.0);
        let first = hierarchy_point(&editor, 0);
        let third = hierarchy_point(&editor, 2);
        let control = control_keyed(KeyCode::LeftControl);
        update(&mut editor, &InputState::new(), &press(first), &mut scene);
        update(&mut editor, &control, &press(third), &mut scene);
        assert_eq!(objects_of(&editor), [0, 2]);

        update(&mut editor, &keyed(KeyCode::X), &UiInput::new(), &mut scene);
        assert_eq!(scene.len(), 1);
        assert_eq!(
            scene.objects[0].transform.scale.y, 2.0,
            "the wrong objects were removed"
        );
    }

    #[test]
    fn dragging_one_of_a_group_moves_all_of_it_by_the_same_amount() {
        let mut editor = editor();
        let mut scene = three_rocks();
        // Everything selected, then a drag that starts on the rock at the origin.
        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );
        let before: Vec<Vec3> = scene
            .objects
            .iter()
            .map(|object| object.transform.translation)
            .collect();

        drag_handle(&mut editor, &mut scene, gizmo::Axis::X, (120.0, 0.0));

        let delta = scene.objects[0].transform.translation - before[0];
        assert!(delta.length() > 0.5, "the drag moved nothing: {delta:?}");
        for (index, object) in scene.objects.iter().enumerate() {
            let moved = object.transform.translation - before[index];
            assert!(
                (moved - delta).length() < 1e-3,
                "object {index} moved by {moved:?}, not by {delta:?}"
            );
        }
    }

    #[test]
    fn the_arrow_keys_nudge_by_the_grid_step() {
        let mut editor = editor();
        let mut scene = three_rocks();
        assert_eq!(editor.grid(), 1.0, "the grid starts at a metre");
        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);

        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.objects[0].transform.translation.x, 1.0);

        update(
            &mut editor,
            &keyed(KeyCode::ArrowUp),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.objects[0].transform.translation.z, -1.0, "up is -Z");

        // SHIFT moves it in Y instead.
        update(
            &mut editor,
            &shift_keyed(KeyCode::ArrowUp),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.objects[0].transform.translation.y, 1.0);
    }

    #[test]
    fn a_local_nudge_follows_the_object_and_a_world_nudge_does_not() {
        let mut editor = editor();
        let mut scene = three_rocks();
        // A quarter turn, so local +X is world -Z.
        scene.objects[0].transform.rotation =
            engine_core::math::Quat::from_yaw_pitch_roll(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);

        // World: +X moves along world X whatever the object is doing.
        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        assert!(!editor.is_local_space());
        assert!((scene.objects[0].transform.translation.x - 1.0).abs() < 1e-5);

        // Local: the same key now runs along the object's own facing.
        update(&mut editor, &keyed(KeyCode::L), &UiInput::new(), &mut scene);
        assert!(editor.is_local_space());
        let before = scene.objects[0].transform.translation;
        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        let moved = scene.objects[0].transform.translation - before;
        assert!(
            moved.x.abs() < 1e-5,
            "local +X still moved world X: {moved:?}"
        );
        assert!((moved.z + 1.0).abs() < 1e-5, "{moved:?}");
    }

    #[test]
    fn rotation_snaps_to_the_step_rather_than_adding_to_whatever_was_there() {
        let mut editor = editor();
        let mut scene = three_rocks();
        // Seven degrees, as if turned by hand or imported from somewhere.
        scene.objects[0].transform.rotation =
            engine_core::math::Quat::from_yaw_pitch_roll(7.0_f32.to_radians(), 0.0, 0.0);
        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);

        // The default step is 22.5 degrees: 7 + 22.5 snaps to 22.5, not to 29.5.
        assert_eq!(editor.yaw_step(), 22.5);
        update(&mut editor, &keyed(KeyCode::R), &UiInput::new(), &mut scene);
        let yaw = yaw_of(scene.objects[0].transform).to_degrees();
        assert!((yaw - 22.5).abs() < 0.01, "{yaw} degrees");

        // SHIFT+G cycles the step, and it wraps.
        update(
            &mut editor,
            &shift_keyed(KeyCode::G),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(editor.yaw_step(), 15.0);
        assert_eq!(editor.grid(), 1.0, "SHIFT+G moved the grid as well");
    }

    #[test]
    fn zero_resets_the_rotation_and_the_size_and_leaves_the_position_alone() {
        let mut editor = editor();
        let mut scene = three_rocks();
        scene.objects[0].transform.rotation =
            engine_core::math::Quat::from_yaw_pitch_roll(1.0, 0.0, 0.0);
        scene.objects[0].transform.scale = Vec3::splat(3.0);
        let placed = scene.objects[0].transform.translation;
        let point = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);

        let outcome = update(
            &mut editor,
            &keyed(KeyCode::Digit0),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        let transform = scene.objects[0].transform;
        assert!(yaw_of(transform).abs() < 1e-6, "the rotation survived");
        assert_eq!(transform.scale, Vec3::ONE);
        assert_eq!(transform.translation, placed, "reset moved it");
    }

    #[test]
    fn undo_puts_back_a_multiple_selection_edit_in_one_step() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let before: Vec<Vec3> = scene
            .objects
            .iter()
            .map(|object| object.transform.translation)
            .collect();
        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );
        update(
            &mut editor,
            &keyed(KeyCode::ArrowRight),
            &UiInput::new(),
            &mut scene,
        );
        assert_ne!(scene.objects[0].transform.translation, before[0]);

        update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        for (index, object) in scene.objects.iter().enumerate() {
            assert_eq!(
                object.transform.translation, before[index],
                "object {index}"
            );
        }
    }

    #[test]
    fn b_puts_the_editor_in_and_out_of_sculpting_and_drops_the_selection() {
        let mut editor = editor();
        let mut scene = three_rocks();
        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(editor.selected().len(), 3);

        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        assert!(editor.is_sculpting());
        assert!(
            editor.selected().is_empty(),
            "a selection survived into sculpt mode, where its verbs no longer exist"
        );

        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        assert!(!editor.is_sculpting(), "B did not come back out");
    }

    #[test]
    fn a_stroke_on_a_level_with_no_terrain_makes_one_and_raises_it() {
        let mut editor = editor();
        let mut scene = scene();
        assert!(scene.terrain.is_none());
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);

        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::TerrainChanged);
        let terrain = scene.terrain.as_ref().expect("a stroke made a terrain");
        assert!(terrain.is_valid());
        assert!(
            terrain.height_at(Vec3::ZERO) > 0.0,
            "the ground did not move under the brush"
        );
        assert_eq!(
            terrain.height_at(Vec3::new(200.0, 0.0, 200.0)),
            0.0,
            "the stroke reached the far side of the level"
        );
    }

    #[test]
    fn a_stroke_reports_terrain_rather_than_scene_so_the_placements_are_left_alone() {
        // The distinction the outcome exists for: re-placing a few hundred objects on every
        // frame of a brush stroke is the cost TerrainChanged avoids.
        let mut editor = editor();
        let mut scene = three_rocks();
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::TerrainChanged);
        assert_eq!(scene.len(), 3, "a brush stroke changed the placements");
    }

    #[test]
    fn the_brush_keys_change_the_brush_and_nothing_else() {
        let mut editor = editor();
        let mut scene = scene();
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);

        let radius = editor.brush_radius();
        let strength = editor.brush_strength();
        assert_eq!(
            update(
                &mut editor,
                &keyed(KeyCode::Digit2),
                &UiInput::new(),
                &mut scene
            ),
            EditorOutcome::Idle,
            "changing the brush cost a rebuild"
        );
        assert!(editor.brush_radius() > radius);
        update(
            &mut editor,
            &keyed(KeyCode::Digit1),
            &UiInput::new(),
            &mut scene,
        );
        assert!(
            (editor.brush_radius() - radius).abs() < 1e-4,
            "1 and 2 disagree"
        );

        update(
            &mut editor,
            &keyed(KeyCode::Digit4),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.brush_strength() > strength);
        assert!(scene.terrain.is_none(), "a brush key made a terrain");
    }

    #[test]
    fn the_brush_mode_cycles_while_sculpting() {
        let mut editor = editor();
        let mut scene = scene();
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        assert_eq!(editor.brush(), BrushMode::Raise);
        update(
            &mut editor,
            &keyed(KeyCode::Tab),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(editor.brush(), BrushMode::Lower);
        assert!(editor.is_sculpting(), "cycling the brush left sculpt mode");
        // And B still leaves, from any brush.
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);
        assert!(!editor.is_sculpting());
    }

    #[test]
    fn one_stroke_is_one_undo_however_many_frames_it_lasted() {
        let mut editor = editor();
        let mut scene = scene();
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);

        // A drag: one press and several held frames.
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        for _ in 0..5 {
            update(&mut editor, &InputState::new(), &hold(centre()), &mut scene);
        }
        assert!(
            scene
                .terrain
                .as_ref()
                .is_some_and(|terrain| { terrain.height_at(Vec3::ZERO) > 0.0 })
        );

        update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        assert!(
            scene.terrain.is_none()
                || scene
                    .terrain
                    .as_ref()
                    .is_some_and(|terrain| terrain.height_at(Vec3::ZERO).abs() < 1e-6),
            "one undo did not take back the whole stroke"
        );
    }

    #[test]
    fn placement_verbs_do_nothing_while_the_brush_has_the_pointer() {
        let mut editor = editor();
        let mut scene = three_rocks();
        update(&mut editor, &keyed(KeyCode::B), &UiInput::new(), &mut scene);

        // A click in the world sculpts instead of placing.
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert_eq!(scene.len(), 3, "the brush placed an object");
        assert!(editor.selection().is_none());
    }

    #[test]
    fn saving_writes_the_scene_where_the_status_line_says_it_did() {
        let directory = std::env::temp_dir().join("mygame_editor_tests");
        std::fs::create_dir_all(&directory).expect("a temp directory");
        let path = directory.join("saved_level.json");

        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(2.0, 0.0, -3.0))
                .with_collision(default_collision(PILLAR)),
        );
        editor.save_path = path.clone();

        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::LeftControl,
            repeat: false,
        });
        input.apply(engine_core::input::InputEvent::KeyDown {
            key: KeyCode::S,
            repeat: false,
        });
        update(&mut editor, &input, &UiInput::new(), &mut scene);

        assert!(editor.status().contains("saved"), "{}", editor.status());
        let loaded = scene_file::load(&path).expect("what was saved loads");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.objects[0].asset, PILLAR);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn f6_leaves_and_the_scene_keeps_what_was_done_to_it() {
        let mut editor = editor();
        let mut scene = scene();
        drop_asset(&mut editor, &mut scene, centre());
        assert_eq!(scene.len(), 1);

        let outcome = update(
            &mut editor,
            &keyed(KeyCode::F6),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::Left);
        assert!(!editor.is_active());
        assert_eq!(scene.len(), 1, "leaving is not undoing");
    }

    #[test]
    fn the_grid_cycles_and_can_be_turned_off() {
        let mut editor = editor();
        let mut scene = scene();
        let mut seen = Vec::new();
        for _ in 0..GRID_STEPS.len() {
            seen.push(editor.grid());
            update(&mut editor, &keyed(KeyCode::G), &UiInput::new(), &mut scene);
        }
        assert!(seen.contains(&0.0), "one step of the cycle is 'off'");
        assert_eq!(editor.grid(), seen[0], "and it comes back round");
    }
}
