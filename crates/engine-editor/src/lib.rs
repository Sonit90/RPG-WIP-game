//! The level editor: placing, moving and deleting environment objects inside the running game.
//!
//! ```text
//!  F6 ──► Editing ───────────────► EditorOutcome
//!   ▲        │  left click object   select it, and never place anything
//!   │        │  left drag on ground box-select; a click on ground drops the selection
//!   │        │  N                   placing mode: left click puts one down, every time
//!   │        │  drag from browser   place that asset where it is dropped, on what is there
//!   │        │  double click a row  place one in front of the camera / open a level
//!   │        │  1 / 2 / 3           gizmo: move / turn / scale
//!   │        │  drag a handle       move, turn or scale the selection (CTRL: no snapping)
//!   │        │  R / SHIFT+R         yaw, snapped to the rotation step
//!   │        │  G / SHIFT+G         grid step / rotation step
//!   │        │  T / SHIFT+T         scale
//!   │        │  ARROWS / SHIFT      nudge by the grid, in X and Z / in Y
//!   │        │  L                   nudge along world axes or the object's own
//!   │        │  0                   reset rotation and scale
//!   │        │  X / DELETE          delete
//!   │        │  CTRL+D              duplicate
//!   │        │  CTRL+G              drop the selection onto what is under it
//!   │        │  H / SHIFT+H         hide the selection / show everything
//!   │        │  K                   lock the selection: the world's clicks pass through it
//!   │        │  V / P / I           pour water / put a spawn point / put a light
//!   │        │  M                   module snap: the cave grid, on and off
//!   │        │  CTRL+click          add to the selection; CTRL+A takes everything
//!   │        │  CTRL+Z / CTRL+Y     undo / redo
//!   │        │  TAB                 next asset in the browser
//!   │        │  CTRL+F / F2         filter the panel under the pointer / read assets/ again
//!   │        │  F / HOME            frame the selection
//!   │        │  ALT+drag / wheel    orbit / fly forward and back
//!   │        │  B                   sculpt the ground; TAB cycles the brush
//!   │        │  1 2 / 3 4           brush size / brush strength, while sculpting
//!   │        │  CTRL+S / +SHIFT     save / save as
//!   └────────┘  F6                  leave
//! ```
//!
//! # One gesture, one meaning
//!
//! Placing and selecting are different gestures, and neither can be mistaken for the other:
//!
//! 1. a drag on a gizmo handle transforms what is selected;
//! 2. a click on something in the world selects it — it never places anything;
//! 3. a drag from a browser row into the world places one there;
//! 4. a double click on a browser row places one in front of the camera;
//! 5. a drag that starts on empty ground draws a selection box;
//! 6. anything else is the camera's.
//!
//! `N` puts the editor in **placing mode**, where rule 2 is suspended: every left click in
//! the world puts an object down, on top of whatever is already there, and nothing selects or
//! moves. Laying out a row of pillars is a dozen clicks rather than a dozen drags out of the
//! browser. `N` again, or `ESC`, and the left button means select once more.
//!
//! # It edits data, and nothing else
//!
//! The editor's whole world is an [`EnvironmentScene`]. It never touches a mesh, a device, a
//! render entity or a collision world, and it cannot: this crate depends on neither
//! `engine-renderer` nor `engine-game`. What it returns is an [`EditorOutcome`], and the
//! application answers that by re-placing the scene it already knows how to place.
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
//! `CTRL+Z` restores the previous scene wholesale rather than inverting the last verb. A
//! scene is a few hundred 44-byte records and a terrain, so a snapshot is a memcpy; inverting
//! verbs means a second implementation of every verb, written backwards, which is where undo
//! bugs live. Redo is the same thing the other way.
//!
//! # Work is not lost
//!
//! The editor counts edits against the last save, says `UNSAVED` while they differ, and
//! writes `<level>.autosave.json` beside the level every couple of minutes, on the way out
//! with F6 and when the game closes. Undo survives F6: leaving to walk through a level and
//! coming back to fix what was wrong with it is the loop an editor exists for.

#![deny(missing_docs)]

pub mod browser;
pub mod camera;
pub mod commands;
pub mod field;
pub mod gizmo;
pub mod icons;
pub mod inspector;
mod interaction;
pub mod layout;
pub mod panels;
pub mod pick;
pub mod theme;
pub mod ui;
pub mod widgets;

use std::path::{Path, PathBuf};

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Mat4, Quat, Vec3};
use engine_core::transform::Transform;
use engine_environment::lighting::SceneLight;
use engine_environment::spawn::SpawnPoint;
use engine_environment::terrain::{BrushMode, Terrain};
use engine_environment::water::WaterBody;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene, scene_file};
use engine_presentation::ui::ground_decal::GroundSurface;
use engine_presentation::ui::input::UiInput;
use engine_presentation::ui::targeting::{cursor_ray, ground_under_cursor};
use engine_render_data::{Camera, RenderDataError, UiScale};

pub use camera::FlyCamera;

/// How many snapshots `CTRL+Z` can walk back through.
const UNDO_DEPTH: usize = 64;

/// How long unsaved work may sit before it is written to the autosave file, in seconds.
const AUTOSAVE_SECONDS: f32 = 120.0;

/// A placement's hidden flag. See [`Editor::hide`].
const HIDDEN: u8 = 1;
/// A placement's locked flag. See [`Editor::update_keys`].
const LOCKED: u8 = 2;

/// What `CTRL+Z` puts back: everything an edit can change.
///
/// The placements, the ground, the water, the spawn points and the lights — and the editor's
/// own hide and lock flags, which are kept beside the placements they describe so that
/// deleting one and taking the delete back brings its flags back with it.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    objects: Vec<EnvironmentObject>,
    terrain: Option<Terrain>,
    water: Vec<WaterBody>,
    spawn_points: Vec<SpawnPoint>,
    lights: Vec<SceneLight>,
    flags: Vec<u8>,
}

impl Snapshot {
    /// The scene as it is now.
    fn of(scene: &EnvironmentScene, flags: &[u8]) -> Self {
        Self {
            objects: scene.objects.clone(),
            terrain: scene.terrain.clone(),
            water: scene.water.clone(),
            spawn_points: scene.spawn_points.clone(),
            lights: scene.lights.clone(),
            flags: flags.to_vec(),
        }
    }

    /// Puts this snapshot back into `scene`, and returns the flags it carried.
    fn restore(self, scene: &mut EnvironmentScene) -> Vec<u8> {
        scene.objects = self.objects;
        scene.terrain = self.terrain;
        scene.water = self.water;
        scene.spawn_points = self.spawn_points;
        scene.lights = self.lights;
        self.flags
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

/// How high above the ground `I` puts a light, in metres: about a brazier's flame.
const LIGHT_HEIGHT: f32 = 2.0;

/// How far one notch of the wheel flies the camera, in metres. `SHIFT` makes it four.
const DOLLY_METRES: f32 = 3.0;

/// How far the pointer must move for a drag on empty ground to be a selection box rather
/// than a click, in logical pixels.
const BOX_SLOP_PIXELS: f32 = 6.0;

/// How many pixels of vertical movement double a uniform scale, in logical pixels.
const UNIFORM_SCALE_PIXELS: f32 = 150.0;

/// What a scale drag rounds to when snapping is on.
const SCALE_SNAP: f32 = 0.05;

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
    /// The water or a spawn point changed: rebuild their meshes and markers.
    ///
    /// Separate from [`EditorOutcome::SceneChanged`] for the reason
    /// [`EditorOutcome::TerrainChanged`] is: re-placing a scene respawns a few hundred render
    /// entities, and resizing one quad is one upload.
    WaterChanged,
    /// The author asked to go back to playing.
    Left,
}

/// What is selected: a placement, a body of water, a spawn point or a light.
///
/// # Why a tag rather than several lists
///
/// Every kind wants the same verbs: click to select, drag to move, `X` to delete, `CTRL+Z` to
/// take it back. Parallel selections would be copies of each verb that can disagree about
/// which one a key press meant.
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
    /// A local light, by its index into `scene.lights`.
    Light(usize),
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

    /// The light index, if this is a light.
    #[must_use]
    pub fn light(self) -> Option<usize> {
        match self {
            Self::Light(index) => Some(index),
            _ => None,
        }
    }

    /// Whether two selections name the same kind of thing.
    #[must_use]
    fn same_kind(self, other: Self) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    /// Whether an edit to this kind is answered by re-placing the level rather than by
    /// rebuilding the water and the spawn markers.
    fn is_scene_kind(self) -> bool {
        matches!(self, Self::Object(_) | Self::Light(_))
    }
}

/// Where something stands, how it is turned and how big it is — whichever kind it is.
///
/// What a gizmo drag remembers at the press and writes back each frame. Water keeps its size
/// in `scale` (half-width, depth, half-length), a light its range in all three, and a spawn
/// point has no size at all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    position: Vec3,
    yaw: f32,
    scale: Vec3,
}

/// A drag of one gizmo handle, and everything it needs to be absolute.
///
/// # Why the gizmo is remembered rather than re-projected
///
/// The handles are projected from the selection's own origin, so a gizmo re-projected each
/// frame moves with the thing being dragged — and then the metres one pixel of movement asks
/// for changes under the author's hand mid-drag. The projection taken at the moment of the
/// press is the ruler for the whole drag.
///
/// # Why the starting poses are remembered
///
/// The drag is absolute: every frame asks "where should this be, given where the pointer
/// started and where it is now", rather than accumulating a delta per frame. An accumulated
/// drag drifts, because a snapped value feeds the next frame's delta back into itself.
#[derive(Debug, Clone, PartialEq)]
struct GizmoDrag {
    /// Which part of the gizmo was grabbed.
    handle: gizmo::Handle,
    /// Where the pointer went down, in physical pixels.
    from: (f32, f32),
    /// The gizmo as it stood at the press. See above.
    screen: gizmo::GizmoScreen,
    /// How everything selected stood when the drag began.
    starts: Vec<(Selected, Pose)>,
    /// What a turn turns about: the primary's position at the press.
    pivot: Vec3,
    /// The pointer's angle round the gizmo last frame, for the turn ring.
    last_angle: Option<f32>,
    /// How far the pointer has swept round the gizmo since the press, radians.
    swept: f32,
    /// Whether anything has actually changed yet, so a click on a handle that does not drag
    /// it does not push an undo snapshot.
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

/// A selection box being dragged out over the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marquee {
    /// Where the press was, in physical pixels.
    pub from: (f32, f32),
    /// Where the pointer is now.
    pub to: (f32, f32),
    /// Whether it adds to the selection (`CTRL`) rather than replacing it.
    add: bool,
}

impl Marquee {
    /// The rectangle, whichever way it was dragged.
    #[must_use]
    pub fn rect(&self) -> engine_render_data::ui::UiRect {
        let (left, right) = (self.from.0.min(self.to.0), self.from.0.max(self.to.0));
        let (top, bottom) = (self.from.1.min(self.to.1), self.from.1.max(self.to.1));
        engine_render_data::ui::UiRect::new(left, top, right - left, bottom - top)
    }
}

/// A number being typed into the inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectorEdit {
    /// Which number.
    pub property: inspector::Property,
    /// What has been typed.
    pub field: field::TextField,
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

/// A dialog that has the editor's full attention until it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// Every key the editor answers, grouped by what it is for.
    Shortcuts,
    /// Opening another level would throw away unsaved work: save, discard or stay.
    Unsaved {
        /// The level that was asked for.
        path: PathBuf,
    },
}

/// A drag on an inspector number's tag, changing it by how far the pointer moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scrub {
    /// Which number.
    property: inspector::Property,
    /// Where the pointer went down.
    from: f32,
    /// What the number was then.
    start: f32,
    /// Whether it has changed yet, so a click without a drag types instead.
    moved: bool,
}

/// A splitter being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Resize {
    /// Which.
    splitter: layout::Splitter,
    /// Where the pointer went down.
    from: (f32, f32),
    /// The sizes then.
    start: layout::PanelSizes,
}

/// The tooltip the pointer is resting on, and since when.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tip {
    /// Which control, so moving to another one restarts the delay.
    key: u32,
    /// What it says.
    pub(crate) title: String,
    /// The key that does the same thing.
    pub(crate) shortcut: &'static str,
    /// When the pointer arrived.
    since: f32,
}

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
    /// What the left button does to the selection: select, move, rotate or scale.
    tool: commands::Tool,
    /// What a turn and the gizmo stand on when several things are selected.
    pivot: commands::Pivot,
    /// The handle being dragged, if one is.
    gizmo_drag: Option<GizmoDrag>,
    /// Which handle the cursor is over, so it can be drawn as the one that would be grabbed.
    gizmo_hover: Option<gizmo::Handle>,
    /// An asset dragged out of the browser and not yet dropped.
    placement: Option<PlacementDrag>,
    /// A selection box being dragged out, if one is.
    marquee: Option<Marquee>,
    /// What an `ALT` drag is orbiting round, while one is.
    orbit_pivot: Option<Vec3>,
    /// Whether a double click on a browser row asked for a placement in front of the camera.
    place_at_centre: bool,
    /// The last browser row or tile that was clicked and when, for the double click.
    last_row_click: Option<(browser::BrowserRow, f32)>,
    /// The last hierarchy row that was clicked and when, for the double click that frames it.
    last_hierarchy_click: Option<(Selected, f32)>,
    /// The last thing clicked in the viewport and when, for the double click that frames it.
    last_viewport_click: Option<(Selected, f32)>,
    /// Where a shift-click in the hierarchy selects from.
    hierarchy_anchor: Option<Selected>,
    /// Seconds since the editor was opened. The clock the double click is measured on.
    clock: f32,
    /// This frame's view-projection, kept so the interface can draw the gizmo and the
    /// silhouette from the same matrix the pointer was answered with.
    view_projection: Mat4,
    /// This frame's window size, kept for the same reason.
    viewport: (u32, u32),
    /// Where the pointer is.
    pointer: (f32, f32),
    /// Index into [`GRID_STEPS`].
    grid: usize,
    /// Whether moves snap to the grid.
    snap: bool,
    /// Index into [`YAW_STEPS`].
    yaw_step: usize,
    /// Whether the terrain brush has the pointer instead of placement and selection.
    sculpting: bool,
    /// Whether a click in the world puts an object down rather than selecting one.
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
    module_snap: bool,
    /// Whether the author has taken the module decision off the editor.
    module_snap_pinned: bool,
    /// Whether what changed this frame was water or a spawn point rather than a placement.
    water_touched: bool,
    /// Snapshots for `CTRL+Z`, oldest first.
    undo: Vec<Snapshot>,
    /// Snapshots for `CTRL+Y`, the most recently undone last.
    redo: Vec<Snapshot>,
    /// Which level the undo history belongs to.
    history_of: Option<String>,
    /// How many edits have been made since the editor first opened this level.
    revision: u64,
    /// The revision the level file on disk holds.
    saved_revision: u64,
    /// The revision the autosave file holds.
    autosaved_revision: u64,
    /// Seconds unsaved work has been waiting for the autosave.
    since_autosave: f32,
    /// Hidden and locked, one byte per placement, parallel to `scene.objects`.
    flags: Vec<u8>,
    /// Where `CTRL+S` writes.
    save_path: PathBuf,
    /// The Save As prompt, while it is open.
    save_as: Option<field::TextField>,
    /// A level the author asked to open, for the application to load.
    open_request: Option<PathBuf>,
    /// A number being typed into the inspector.
    inspector_edit: Option<InspectorEdit>,
    /// The last property the wheel changed and when, so a run of notches is one undo.
    wheel_run: Option<(inspector::Property, f32)>,
    /// A drag on an inspector tag, while one is on.
    scrub: Option<Scrub>,
    /// How far the inspector is scrolled, in pixels.
    inspector_scroll: f32,
    /// Which inspector sections are folded away.
    collapsed_sections: u8,
    /// What the hierarchy is filtered by.
    hierarchy_filter: String,
    /// The hierarchy filter, while it is being typed.
    hierarchy_typing: Option<field::TextField>,
    /// Which hierarchy groups are folded away.
    collapsed_groups: u16,
    /// The last primary selection the hierarchy scrolled to, so it scrolls once per change
    /// rather than every frame.
    revealed: Option<Selected>,
    /// The last thing that happened, for the status bar.
    status: String,
    /// The status as it was last frame, to notice when it changes.
    status_seen: String,
    /// When the status last changed, so it gives way to a hint after a few seconds.
    status_at: f32,
    /// The first row shown in the hierarchy list.
    hierarchy_scroll: usize,
    /// How the browser shows a folder.
    browser_view: commands::BrowserView,
    /// Which folder the browser's tile view is in.
    tile_folder: panels::TileFolder,
    /// The first row of tiles shown.
    tile_scroll: usize,
    /// How wide the panels are.
    panels: layout::PanelSizes,
    /// A splitter being dragged.
    resizing: Option<Resize>,
    /// The popup menu that is open, if one is.
    menu: Option<panels::OpenMenu>,
    /// Where the menu was opened, for the commands that act *there*: paste, add a light.
    menu_origin: Option<(f32, f32)>,
    /// A dialog, if one is up.
    modal: Option<Modal>,
    /// Whether a button or a menu asked to go back to the game.
    leave_requested: bool,
    /// Whether the debug overlay is on.
    show_debug: bool,
    /// What is on the clipboard.
    clipboard: Vec<commands::Clip>,
    /// The tooltip the pointer is resting on.
    tip: Option<Tip>,
    /// Last frame's tooltip, so resting on the same control keeps counting towards the delay.
    tip_last: Option<Tip>,
    /// What the pointer is over in the viewport, for the hover outline.
    hovered: Option<Selected>,
    /// Where the right button went down and when, so a click can open a menu and a drag can
    /// turn the camera.
    right_press: Option<((f32, f32), f32)>,
    /// The ground under the pointer, for the status bar.
    cursor_world: Option<Vec3>,
    /// Frames per second, smoothed, for the debug overlay.
    fps: f32,
    /// How long the last frame took, so a brush stroke moves the ground at the same rate
    /// whatever the frame rate is.
    last_seconds: f32,
    /// The interface scale the player chose; the editor lays itself out at this or more.
    settings_scale: UiScale,
    /// The scale the editor is laid out at this frame.
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
            tool: commands::Tool::Move,
            pivot: commands::Pivot::Active,
            gizmo_drag: None,
            gizmo_hover: None,
            placement: None,
            marquee: None,
            orbit_pivot: None,
            place_at_centre: false,
            last_row_click: None,
            last_hierarchy_click: None,
            last_viewport_click: None,
            hierarchy_anchor: None,
            clock: 0.0,
            view_projection: Mat4::IDENTITY,
            viewport: (0, 0),
            pointer: (0.0, 0.0),
            grid: 0,
            snap: true,
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
            redo: Vec::new(),
            history_of: None,
            revision: 0,
            saved_revision: 0,
            autosaved_revision: 0,
            since_autosave: 0.0,
            flags: Vec::new(),
            save_path: PathBuf::from("assets/scenes/untitled.json"),
            save_as: None,
            open_request: None,
            inspector_edit: None,
            wheel_run: None,
            scrub: None,
            inspector_scroll: 0.0,
            collapsed_sections: 0,
            hierarchy_filter: String::new(),
            hierarchy_typing: None,
            collapsed_groups: 0,
            revealed: None,
            status: String::new(),
            status_seen: String::new(),
            status_at: 0.0,
            hierarchy_scroll: 0,
            browser_view: commands::BrowserView::List,
            tile_folder: panels::TileFolder::Root,
            tile_scroll: 0,
            panels: layout::PanelSizes::default(),
            resizing: None,
            menu: None,
            menu_origin: None,
            modal: None,
            leave_requested: false,
            show_debug: false,
            clipboard: Vec::new(),
            tip: None,
            tip_last: None,
            hovered: None,
            right_press: None,
            cursor_world: None,
            fps: 60.0,
            last_seconds: 1.0 / 60.0,
            settings_scale: UiScale::default(),
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

    /// The primary selection, if it is a light.
    #[must_use]
    pub fn selected_light(&self) -> Option<usize> {
        self.selection().and_then(Selected::light)
    }

    /// Every selected placement, in the order it was added.
    #[must_use]
    fn selected_objects(&self) -> Vec<usize> {
        self.selected
            .iter()
            .filter_map(|what| what.object())
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

    /// What the left button does to the selection.
    #[must_use]
    pub fn tool(&self) -> commands::Tool {
        self.tool
    }

    /// What the gizmo does with a drag, if the tool shows one.
    #[must_use]
    pub fn gizmo_mode(&self) -> Option<gizmo::GizmoMode> {
        self.tool.gizmo()
    }

    /// What a turn and the gizmo stand on.
    #[must_use]
    pub fn pivot(&self) -> commands::Pivot {
        self.pivot
    }

    /// Whether moves snap to the grid.
    #[must_use]
    pub fn is_snapping(&self) -> bool {
        self.module_snap || (self.snap && GRID_STEPS[self.grid % GRID_STEPS.len()] > 0.0)
    }

    /// The grid size chosen, whether or not snapping is on.
    #[must_use]
    pub fn grid_size(&self) -> f32 {
        if self.snap {
            GRID_STEPS[self.grid % GRID_STEPS.len()]
        } else {
            0.0
        }
    }

    /// Which of the grid sizes is chosen.
    #[must_use]
    pub fn grid_index(&self) -> usize {
        self.grid % GRID_STEPS.len()
    }

    /// Which of the rotation steps is chosen.
    #[must_use]
    pub fn yaw_step_index(&self) -> usize {
        self.yaw_step % YAW_STEPS.len()
    }

    /// Whether there is anything to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Whether the clipboard holds anything.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        !self.clipboard.is_empty()
    }

    /// Whether the debug overlay is on.
    #[must_use]
    pub fn shows_debug(&self) -> bool {
        self.show_debug
    }

    /// Whether the keyboard shortcuts window is up.
    #[must_use]
    pub fn shows_shortcuts(&self) -> bool {
        self.modal == Some(Modal::Shortcuts)
    }

    /// The dialog that is up, if one is.
    #[must_use]
    pub fn modal(&self) -> Option<&Modal> {
        self.modal.as_ref()
    }

    /// The popup menu that is open, if one is.
    #[must_use]
    pub fn open_menu(&self) -> Option<&panels::OpenMenu> {
        self.menu.as_ref()
    }

    /// How the browser shows a folder.
    #[must_use]
    pub fn browser_view(&self) -> commands::BrowserView {
        self.browser_view
    }

    /// Which folder the browser's tile view is in.
    #[must_use]
    pub fn tile_folder(&self) -> panels::TileFolder {
        self.tile_folder
    }

    /// The first row of tiles shown.
    #[must_use]
    pub fn tile_scroll(&self) -> usize {
        self.tile_scroll
    }

    /// How far the inspector is scrolled, in pixels.
    #[must_use]
    pub fn inspector_scroll(&self) -> f32 {
        self.inspector_scroll
    }

    /// Which inspector sections are folded away, one bit each.
    #[must_use]
    pub fn collapsed_sections(&self) -> u8 {
        self.collapsed_sections
    }

    /// Whether a hierarchy group is folded away.
    #[must_use]
    pub fn is_group_collapsed(&self, group: panels::Group) -> bool {
        self.collapsed_groups & group.bit() != 0
    }

    /// What the pointer is over in the viewport.
    #[must_use]
    pub fn hovered(&self) -> Option<Selected> {
        self.hovered
    }

    /// Where the pointer is.
    #[must_use]
    pub fn pointer(&self) -> (f32, f32) {
        self.pointer
    }

    /// The ground under the pointer, when it is over the viewport.
    #[must_use]
    pub fn cursor_world(&self) -> Option<Vec3> {
        self.cursor_world
    }

    /// Seconds since the editor opened.
    #[must_use]
    pub fn clock(&self) -> f32 {
        self.clock
    }

    /// Frames per second, smoothed.
    #[must_use]
    pub fn fps(&self) -> f32 {
        self.fps
    }

    /// How many steps `CTRL+Z` can take back, and `CTRL+Y` forward.
    #[must_use]
    pub fn history_depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// The tooltip to show, once the pointer has rested long enough.
    #[must_use]
    pub fn tooltip(&self) -> Option<(&str, &str)> {
        self.tip
            .as_ref()
            .filter(|tip| self.clock - tip.since >= theme::metrics::TOOLTIP_DELAY)
            .map(|tip| (tip.title.as_str(), tip.shortcut))
    }

    /// The camera's yaw and pitch, for the axis indicator in the corner of the viewport.
    #[must_use]
    pub fn camera_angles(&self) -> (f32, f32) {
        (self.camera.yaw, self.camera.pitch)
    }

    /// The panel layout for a window of `viewport` pixels at this frame's scale.
    #[must_use]
    pub fn layout_for(&self, viewport: (u32, u32)) -> layout::Layout {
        layout::Layout::new(self.scale, viewport, self.panels)
    }

    /// The panel layout this frame was answered with.
    #[must_use]
    pub fn layout(&self) -> layout::Layout {
        self.layout_for(self.viewport)
    }

    /// Whether a gizmo drag, a box or an asset carried out of the browser is in progress.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.gizmo_drag.is_some() || self.marquee.is_some() || self.placement.is_some()
    }

    /// A short readout of the gizmo drag in progress: how far, how much.
    #[must_use]
    pub fn drag_readout(&self) -> Option<&str> {
        self.gizmo_drag
            .as_ref()
            .filter(|drag| drag.moved)
            .map(|_| self.status.as_str())
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
        if !self.snap {
            return 0.0;
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

    /// Where unsaved work is written while it waits: `<level>.autosave.json` beside the level.
    #[must_use]
    pub fn autosave_path(&self) -> PathBuf {
        let stem = self
            .save_path
            .file_stem()
            .map_or_else(|| "untitled".into(), |stem| stem.to_string_lossy());
        self.save_path
            .with_file_name(format!("{stem}.autosave.json"))
    }

    /// Whether there are edits the level file on disk does not have.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// The last thing that happened, for the status line.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// The last thing that happened, if it happened in the last few seconds — after that the
    /// status bar goes back to saying what can be done next.
    #[must_use]
    pub fn recent_status(&self) -> Option<&str> {
        (!self.status.is_empty() && self.clock - self.status_at < 5.0)
            .then_some(self.status.as_str())
    }

    /// Which row is at the top of the hierarchy list.
    #[must_use]
    pub fn hierarchy_scroll(&self) -> usize {
        self.hierarchy_scroll
    }

    /// What the hierarchy is filtered by; empty for everything.
    #[must_use]
    pub fn hierarchy_filter(&self) -> &str {
        &self.hierarchy_filter
    }

    /// Whether the hierarchy's filter is being typed.
    #[must_use]
    pub fn is_filtering_hierarchy(&self) -> bool {
        self.hierarchy_typing.is_some()
    }

    /// The number being typed into the inspector, if one is.
    #[must_use]
    pub fn inspector_edit(&self) -> Option<&InspectorEdit> {
        self.inspector_edit.as_ref()
    }

    /// The Save As name being typed, if the prompt is open.
    #[must_use]
    pub fn save_as_text(&self) -> Option<&str> {
        self.save_as.as_ref().map(field::TextField::text)
    }

    /// The selection box being dragged, if one is.
    #[must_use]
    pub fn marquee(&self) -> Option<Marquee> {
        self.marquee
    }

    /// Whether a placement is hidden: not drawn and not pickable in the world.
    #[must_use]
    pub fn is_hidden(&self, index: usize) -> bool {
        self.flags
            .get(index)
            .is_some_and(|flags| flags & HIDDEN != 0)
    }

    /// Whether a placement is locked: drawn, but the world's clicks pass through it.
    #[must_use]
    pub fn is_locked(&self, index: usize) -> bool {
        self.flags
            .get(index)
            .is_some_and(|flags| flags & LOCKED != 0)
    }

    /// Whether anything is hidden.
    #[must_use]
    pub fn has_hidden(&self) -> bool {
        self.flags.iter().any(|flags| flags & HIDDEN != 0)
    }

    /// The scene as it should be *drawn*: without what the author has hidden.
    ///
    /// `None` when nothing is hidden, so the application draws the scene it already has and
    /// pays for no copy. Hiding is an editor view: the level, its collision and its file keep
    /// every placement.
    #[must_use]
    pub fn visible_scene(&self, scene: &EnvironmentScene) -> Option<EnvironmentScene> {
        if !self.active || !self.has_hidden() {
            return None;
        }
        let mut visible = scene.clone();
        let mut index = 0;
        visible.objects.retain(|_| {
            let keep = !self.is_hidden(index);
            index += 1;
            keep
        });
        Some(visible)
    }

    /// Where `CTRL+S` writes from now on.
    ///
    /// The application sets this when a level was opened from a named file, so saving writes
    /// back to the file it was opened from rather than to one named after the scene.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.status = format!("editing {}", path.display());
        self.save_path = path;
    }

    /// The level the author asked to open, if they did — for the application to load and
    /// hand back with [`Editor::opened`].
    pub fn take_open_request(&mut self) -> Option<PathBuf> {
        self.open_request.take()
    }

    /// Starts editing a level the application has just loaded from `path`.
    ///
    /// Everything that described the previous level goes: its history, its selection, its
    /// flags and its unsaved count. The file it came from is where `CTRL+S` writes.
    pub fn opened(&mut self, scene: &EnvironmentScene, path: PathBuf) {
        self.reset_history(&scene.name);
        self.flags = vec![0; scene.objects.len()];
        self.selected.clear();
        self.cancel_manipulation();
        self.hierarchy_scroll = 0;
        self.status = format!("opened {}", path.display());
        self.save_path = path;
        if let Some(pivot) = scene.objects.first().map(|object| object.position()) {
            self.camera = FlyCamera::overlooking(pivot);
        }
    }

    /// Says why a level the author asked to open could not be.
    pub fn open_failed(&mut self, error: &str) {
        self.status = format!("COULD NOT OPEN: {error}");
    }

    /// Forgets the undo history and the unsaved count, for a level the editor has not seen.
    fn reset_history(&mut self, name: &str) {
        self.undo.clear();
        self.redo.clear();
        self.history_of = Some(name.to_owned());
        self.revision = 0;
        self.saved_revision = 0;
        self.autosaved_revision = 0;
        self.since_autosave = 0.0;
    }

    /// The interface scale the panels are laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.scale
    }

    /// Sets the interface scale the player chose. The editor lays itself out at this, or at
    /// more on a tall window.
    pub fn set_scale(&mut self, scale: UiScale, viewport: (u32, u32)) {
        self.settings_scale = scale;
        self.scale = theme::scale_for(scale, viewport);
    }

    /// Enters the editor, looking at `focus`.
    ///
    /// Re-entering the level the editor was last in keeps its undo history, its unsaved count
    /// and the file it saves to: leaving to walk through a level and coming back to fix it is
    /// the loop an editor exists for, and it must not cost the author their undo. A different
    /// level starts clean and saves to a file named after it.
    pub fn enter(&mut self, focus: Vec3, scene_name: &str, viewport: (u32, u32)) {
        self.active = true;
        self.camera = FlyCamera::overlooking(focus);
        self.placing = false;
        self.sculpting = false;
        self.selected.clear();
        self.cancel_manipulation();
        self.hierarchy_scroll = 0;
        if self.history_of.as_deref() != Some(scene_name) {
            self.reset_history(scene_name);
            self.flags.clear();
            self.save_path = PathBuf::from(format!("assets/scenes/{scene_name}.json"));
        }
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
        if self.autosave_path().is_file() && !self.is_dirty() {
            self.status = format!(
                "an autosave is waiting: {} (open it from the browser)",
                self.autosave_path().display()
            );
        }
        let _ = viewport;
    }

    /// Leaves the editor. The scene keeps whatever was done to it.
    pub fn leave(&mut self) {
        self.active = false;
        self.cancel_manipulation();
        self.inspector_edit = None;
        self.save_as = None;
        self.hierarchy_typing = None;
        self.menu = None;
        self.modal = None;
        self.scrub = None;
        self.resizing = None;
    }

    /// Writes this camera into the renderer's.
    ///
    /// # Errors
    ///
    /// [`RenderDataError::InvalidConfig`] for a degenerate aspect ratio.
    pub fn apply_camera(&self, camera: &mut Camera, aspect: f32) -> Result<(), RenderDataError> {
        self.camera.apply(camera, aspect)
    }

    /// Keeps the hide and lock flags as long as the object list, whatever changed it.
    fn sync_flags(&mut self, scene: &EnvironmentScene) {
        self.flags.resize(scene.objects.len(), 0);
    }

    /// Whether the world's pointer can reach `what`: not hidden, not locked.
    fn pickable(&self, what: Selected) -> bool {
        match what {
            Selected::Object(index) => self.flags.get(index).is_none_or(|flags| *flags == 0),
            _ => true,
        }
    }

    /// Applies one frame of input to `scene`.
    ///
    /// `view_projection` must be this frame's, from the camera [`Editor::apply_camera`]
    /// wrote — a ray built from last frame's matrix places objects where the cursor was, not
    /// where it is.
    ///
    /// # The order input is answered in
    ///
    /// A dialog, then an open menu, then a text field that owns the keyboard, then a splitter
    /// being dragged, then the toolbar and the three panels, and only then the viewport. Each
    /// layer that answers the pointer stops it reaching the layers under it, which is what
    /// keeps a click on a menu item from also selecting the rock behind the menu.
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
        self.sync_flags(scene);
        if input.was_key_pressed(KeyCode::F6) || std::mem::take(&mut self.leave_requested) {
            // The last chance before the author walks into the level: whatever they did is
            // written somewhere, even if they never pressed CTRL+S.
            self.autosave(scene);
            self.leave();
            return EditorOutcome::Left;
        }
        self.water_touched = false;
        // Remembered so the interface can draw the gizmo and the silhouette from the same
        // matrix the pointer was answered with, and so the double click has a clock to
        // measure itself on.
        self.view_projection = *view_projection;
        self.viewport = viewport;
        self.pointer = ui_input.pointer;
        self.clock += seconds;
        if seconds > 0.0 {
            self.fps += (1.0 / seconds - self.fps) * 0.1;
        }
        self.scale = theme::scale_for(self.settings_scale, viewport);
        let layout = self.layout_for(viewport);
        self.tick_autosave(scene, seconds);
        self.tip_last = self.tip.take();
        if self.status != self.status_seen {
            self.status_seen.clone_from(&self.status);
            self.status_at = self.clock;
        }

        // Dialogs own everything while they are up.
        if self.save_as.is_some() {
            self.update_save_as(input, ui_input, scene, &layout);
            return EditorOutcome::Idle;
        }
        if self.modal.is_some() {
            let changed = self.update_modal(input, ui_input, scene, &layout);
            return self.outcome(changed);
        }
        if self.menu.is_some() {
            let (answered, changed) = self.update_menu(input, ui_input, scene, &layout);
            if answered {
                return self.outcome(changed);
            }
        }
        if self.hierarchy_typing.is_some() {
            if ui_input.pointer_pressed
                && !layout
                    .hierarchy_search
                    .contains(ui_input.pointer.0, ui_input.pointer.1)
            {
                // A click anywhere else is done with the filter, and still lands.
                self.hierarchy_typing = None;
            } else {
                self.update_hierarchy_filter(input);
                return EditorOutcome::Idle;
            }
        }

        let modifiers = input.modifiers();
        let control = modifiers.ctrl();
        let pointer = ui_input.pointer;
        let over_viewport = layout.viewport.contains(pointer.0, pointer.1);

        // CTRL+F finds in the panel under the pointer: the hierarchy's filter over the
        // hierarchy, the browser's everywhere else.
        if control
            && input.was_key_pressed(KeyCode::F)
            && layout.hierarchy.contains(pointer.0, pointer.1)
        {
            self.start_hierarchy_filter();
            return EditorOutcome::Idle;
        }

        let mut changed = false;
        // A number being typed owns the keyboard; the pointer carries on, and a click that
        // lands anywhere else commits it first.
        let field_took_the_keys = self.inspector_edit.is_some();
        if field_took_the_keys {
            changed |= self.update_inspector_edit(input, scene);
        }

        // The browser reads the keyboard next, because while its filter is open every key
        // belongs to the filter — including the ones that would otherwise fly the camera.
        let browser_took_the_keys = !field_took_the_keys && self.browser.update_keys(input);
        let typing = field_took_the_keys || self.browser.is_typing();
        if !typing {
            self.last_seconds = seconds;
            self.camera.update(input, seconds);
        }

        // A splitter being dragged owns the pointer until the button comes up.
        if self.update_splitters(ui_input, &layout) {
            return self.outcome(changed);
        }

        // ALT+drag orbits, and while it does the left button is the camera's.
        if self.update_orbit(
            input,
            ui_input,
            scene,
            view_projection,
            viewport,
            !over_viewport,
        ) {
            return EditorOutcome::Idle;
        }
        let wheel = input.wheel_delta();
        if wheel != 0.0 && over_viewport && !typing {
            let fast = if modifiers.shift() { 4.0 } else { 1.0 };
            self.camera.dolly(wheel * DOLLY_METRES * fast);
        }

        // The toolbar and the panels own the pointer over themselves: a click on a browser row
        // chooses an asset and must never also place one in the world behind the panel.
        let drag_in_progress = self.gizmo_drag.is_some() || self.marquee.is_some();
        if !drag_in_progress {
            changed |= self.update_toolbar(ui_input, scene, &layout);
            self.update_browser_panel(input, ui_input, &layout);
            // What is about to be placed decides the grid, until somebody says otherwise.
            self.follow_module_snap();
            changed |= self.update_hierarchy_panel(input, ui_input, scene, &layout);
            changed |= self.update_inspector_panel(input, ui_input, scene, &layout);
        }
        if self.update_context_menu(input, scene, &layout) {
            return self.outcome(changed);
        }
        let over_panel = !over_viewport;

        // Sculpting takes the world pointer and most of the keyboard; the panels keep theirs.
        // A key the brush answered is *not* passed on: `B` cycles the brush while sculpting
        // and switches modes while not, and one press must not do both.
        if self.sculpting && !typing {
            let (outcome, handled) = self.update_terrain(
                input,
                ui_input,
                scene,
                view_projection,
                viewport,
                over_panel,
            );
            if outcome != EditorOutcome::Idle || handled {
                self.reveal_selection(scene, &layout);
                return outcome;
            }
        }

        // `V`, `P` and `I` put something where the cursor is, so they need the ray the
        // pointer verbs use rather than only the keyboard — answered here, before the keyboard
        // verbs, and their key is not passed on to them.
        if !browser_took_the_keys && !typing && !self.sculpting && !control {
            let put = if input.was_key_pressed(KeyCode::V) {
                Some(self.pour_water(ui_input, scene, view_projection, viewport, over_panel))
            } else if input.was_key_pressed(KeyCode::P) {
                Some(self.put_spawn(ui_input, scene, view_projection, viewport, over_panel))
            } else if input.was_key_pressed(KeyCode::I) {
                Some(self.put_light(ui_input, scene, view_projection, viewport, over_panel))
            } else {
                None
            };
            if let Some(put) = put {
                self.reveal_selection(scene, &layout);
                return match (put, self.water_touched) {
                    (false, _) => EditorOutcome::Idle,
                    (true, true) => EditorOutcome::WaterChanged,
                    (true, false) => EditorOutcome::SceneChanged,
                };
            }
        }

        if !(browser_took_the_keys || typing) && self.update_keys(input, scene) {
            changed = true;
        }
        // An asset carried out of the browser, or asked for by a double click on a row. The
        // only two ways anything is placed — see [`Editor::update_placement`].
        if self.update_placement(ui_input, scene, view_projection, viewport, over_panel) {
            changed = true;
        }

        let precise = control;
        if self.update_pointer(
            ui_input,
            scene,
            view_projection,
            viewport,
            over_panel,
            control,
            precise,
        ) {
            changed = true;
        }
        self.update_hover(ui_input, scene, view_projection, viewport, over_viewport);

        // Whatever just happened, what is selected is worth seeing in the list.
        self.reveal_selection(scene, &layout);
        self.outcome(changed)
    }

    /// What a frame that did or did not change the level tells the application.
    fn outcome(&self, changed: bool) -> EditorOutcome {
        match (changed, self.water_touched) {
            // Water changed and no placement did: one or two quads to rebuild, and none of
            // the level's render entities to respawn.
            (true, true) => EditorOutcome::WaterChanged,
            (true, false) => EditorOutcome::SceneChanged,
            (false, _) => EditorOutcome::Idle,
        }
    }

    /// One frame of the Save As prompt.
    fn update_save_as(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        layout: &layout::Layout,
    ) {
        let dialog = panels::dialog(layout, panels::DialogKind::SaveAs);
        let (x, y) = ui_input.pointer;
        let clicked = if ui_input.pointer_pressed {
            if dialog.close.contains(x, y) || !dialog.panel.contains(x, y) {
                Some(field::FieldAnswer::Cancel)
            } else {
                dialog
                    .buttons
                    .iter()
                    .find(|(rect, ..)| rect.contains(x, y))
                    .map(|(_, button, ..)| match button {
                        panels::DialogButton::Save => field::FieldAnswer::Commit,
                        _ => field::FieldAnswer::Cancel,
                    })
            }
        } else {
            None
        };
        let Some(field) = self.save_as.as_mut() else {
            return;
        };
        let answer = match clicked {
            Some(answer) => answer,
            None => field.update(input),
        };
        match answer {
            field::FieldAnswer::Editing => {}
            field::FieldAnswer::Cancel => {
                self.save_as = None;
                self.status = "save as: cancelled".to_owned();
            }
            field::FieldAnswer::Commit | field::FieldAnswer::CommitNext => {
                let name = field.text().trim_matches(['-', '_']).to_owned();
                self.save_as = None;
                if name.is_empty() {
                    self.status = "save as: a level needs a name".to_owned();
                    return;
                }
                let directory = self
                    .save_path
                    .parent()
                    .map_or_else(|| PathBuf::from("assets/scenes"), Path::to_path_buf);
                self.save_path = directory.join(format!("{name}.json"));
                // The level is called what its file is called from now on, and its history
                // goes with it: the undo stack is still this level's.
                scene.name = name.clone().into();
                self.history_of = Some(name);
                self.save(scene);
            }
        }
    }

    /// One frame of the hierarchy filter being typed.
    fn update_hierarchy_filter(&mut self, input: &InputState) {
        let Some(typing) = self.hierarchy_typing.as_mut() else {
            return;
        };
        let answer = typing.update(input);
        self.hierarchy_filter = typing.text().to_owned();
        self.hierarchy_scroll = 0;
        match answer {
            field::FieldAnswer::Editing => {}
            field::FieldAnswer::Commit | field::FieldAnswer::CommitNext => {
                self.hierarchy_typing = None;
            }
            field::FieldAnswer::Cancel => {
                self.hierarchy_typing = None;
                self.hierarchy_filter.clear();
            }
        }
    }

    /// `ALT` + left drag: orbit round the selection, or round the ground in the middle of the
    /// view. Returns whether the camera had the pointer this frame.
    fn update_orbit(
        &mut self,
        input: &InputState,
        ui_input: &UiInput,
        scene: &EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
    ) -> bool {
        if !ui_input.pointer_down {
            self.orbit_pivot = None;
            return false;
        }
        if self.orbit_pivot.is_none() {
            if !(ui_input.pointer_pressed && input.modifiers().alt() && !over_panel) {
                return false;
            }
            let centre = (viewport.0 as f32 * 0.5, viewport.1 as f32 * 0.5);
            self.orbit_pivot = self
                .gizmo_origin(scene)
                .or_else(|| ground_under(centre, view_projection, viewport, scene))
                .or(Some(self.camera.position + self.camera.forward() * 20.0));
            self.cancel_manipulation();
        }
        if let Some(pivot) = self.orbit_pivot {
            let (dx, dy) = input.mouse_delta();
            self.camera.orbit(pivot, dx as f32, dy as f32);
        }
        true
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
        let Some(ground) = ground_under(ui_input.pointer, view_projection, viewport, scene) else {
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

    /// Puts a warm point light above the ground under the cursor. Returns whether one arrived.
    fn put_light(
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
        let Some(ground) = self.drop_point(ui_input.pointer, scene, view_projection, viewport)
        else {
            self.status = "point at the ground to put a light".to_owned();
            return false;
        };
        self.snapshot(scene);
        let standing = self.snap(ground);
        let position = Vec3::new(standing.x, ground.y + LIGHT_HEIGHT, standing.z);
        // Like the last light, so a row of torches is a row of the same torch.
        let light = scene.lights.last().map_or_else(
            || SceneLight::brazier(position, 10.0),
            |last| SceneLight { position, ..*last },
        );
        scene.push_light(light);
        self.selected.clear();
        self.selected.push(Selected::Light(scene.lights.len() - 1));
        self.status = format!("light at {:.0} {:.0}", position.x, position.z);
        true
    }

    /// Pours a body of water on the ground under the cursor. Returns whether one arrived.
    ///
    /// Eight metres square and two deep, at the height of the ground it was poured onto — the
    /// size of a thing an author grows rather than one they immediately have to shrink. It is
    /// a snapshot like any other edit, so `CTRL+Z` takes the pour back.
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
        let Some(ground) = ground_under(ui_input.pointer, view_projection, viewport, scene) else {
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
        let modifiers = input.modifiers();
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
        let redo = modifiers.ctrl()
            && (input.was_key_pressed(KeyCode::Y)
                || (modifiers.shift() && input.was_key_pressed(KeyCode::Z)));
        if redo || (modifiers.ctrl() && input.was_key_pressed(KeyCode::Z)) {
            // Undo can put back a scene whose objects *and* terrain are different, so the
            // whole level is rebuilt rather than guessing which half moved.
            let done = if redo {
                self.redo(scene)
            } else {
                self.undo(scene)
            };
            let outcome = if done {
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
        let Some(ground) = ground_under(ui_input.pointer, view_projection, viewport, scene) else {
            return (EditorOutcome::Idle, false);
        };

        // A level with no terrain gets one the first time somebody paints on it. Sized to the
        // level's own playable square, so the ground reaches as far as the walls do.
        if scene.terrain.is_none() {
            let cell = 2.0;
            let resolution = ((scene.half_extent * 2.0 / cell).round() as usize + 1).max(9);
            self.snapshot(scene);
            scene.terrain = Some(Terrain::flat(resolution, cell, scene.ground_height));
            self.status = format!("terrain created, {resolution} points a side");
        } else if ui_input.pointer_pressed {
            // The snapshot is taken when a stroke *starts*, not on every frame of it: a drag
            // is one edit, and an undo stack with sixty entries for one stroke is an undo
            // stack nobody can walk back through.
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

    /// Asks the application to open `path` — or, over unsaved work, asks the author first.
    pub(crate) fn request_open(&mut self, path: PathBuf) {
        if self.is_dirty() {
            self.modal = Some(Modal::Unsaved { path });
            return;
        }
        self.status = format!("Opening {}", path.display());
        self.open_request = Some(path);
    }

    /// One frame of typing into the inspector. Returns whether the scene changed.
    fn update_inspector_edit(&mut self, input: &InputState, scene: &mut EnvironmentScene) -> bool {
        let Some(edit) = self.inspector_edit.as_mut() else {
            return false;
        };
        match edit.field.update(input) {
            field::FieldAnswer::Editing => false,
            field::FieldAnswer::Cancel => {
                self.inspector_edit = None;
                self.status = "not changed".to_owned();
                false
            }
            field::FieldAnswer::Commit => self.commit_inspector_edit(scene),
            field::FieldAnswer::CommitNext => {
                let property = edit.property;
                let changed = self.commit_inspector_edit(scene);
                // On to the next number in reading order, wrapping, the way TAB moves through
                // any form.
                if let Some(primary) = self.selection() {
                    let all: Vec<_> = inspector::properties(primary).collect();
                    let next = all
                        .iter()
                        .position(|known| *known == property)
                        .map_or(0, |at| (at + 1) % all.len().max(1));
                    if let Some(property) = all.get(next).copied() {
                        let value = inspector::get(scene, primary, property).unwrap_or(0.0);
                        self.inspector_edit = Some(InspectorEdit {
                            property,
                            field: field::TextField::new(
                                &format_value(value),
                                field::Charset::Number,
                            ),
                        });
                    }
                }
                changed
            }
        }
    }

    /// Sets the number being typed, if it is one. Returns whether the scene changed.
    fn commit_inspector_edit(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(edit) = self.inspector_edit.take() else {
            return false;
        };
        let Some(primary) = self.selection() else {
            return false;
        };
        let Some(value) = edit.field.number() else {
            self.status = format!("'{}' is not a number", edit.field.text());
            return false;
        };
        let before = Snapshot::of(scene, &self.flags);
        let selection = self.selected.clone();
        if !inspector::set(scene, &selection, primary, edit.property, value) {
            return false;
        }
        self.push_undo(before);
        self.water_touched = !primary.is_scene_kind();
        self.status = format!("set to {}", format_value(value));
        true
    }

    /// Adds `what` to the selection, or replaces the selection with it.
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
            Selected::Light(index) => *index < scene.lights.len(),
        });
    }

    /// Moves the camera to see the selection whole — or the whole level, with nothing
    /// selected.
    fn frame_selection(&mut self, scene: &EnvironmentScene) {
        let targets: Vec<Selected> = if self.selected.is_empty() {
            pick::inside_screen_rect(scene, |_| true, |_| true)
        } else {
            self.selected.clone()
        };
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for what in &targets {
            if let Some((center, half_extents, _)) = pick::selection_box(scene, *what) {
                let reach = Vec3::splat(half_extents.length());
                low = low.min(center - reach);
                high = high.max(center + reach);
            }
        }
        if !(low.x.is_finite() && high.x.is_finite()) {
            self.status = "nothing to frame".to_owned();
            return;
        }
        let center = (low + high) * 0.5;
        let radius = ((high - low) * 0.5).length();
        self.camera.frame(center, radius);
        self.status = if self.selected.is_empty() {
            "framed the level".to_owned()
        } else {
            format!("framed {}", self.selected.len())
        };
    }

    /// The keyboard verbs. Returns whether the scene changed.
    ///
    /// Every verb is a [`commands::Command`]: a key here, a toolbar button and a menu item
    /// all end up in [`Editor::execute`].
    fn update_keys(&mut self, input: &InputState, scene: &mut EnvironmentScene) -> bool {
        use commands::{Command, Tool};
        let modifiers = input.modifiers();
        let (control, shift) = (modifiers.ctrl(), modifiers.shift());
        let pressed = |key: KeyCode| input.was_key_pressed(key);

        if pressed(KeyCode::Escape) {
            // The way out of everything in progress, not only the selection: an asset picked
            // up from the browser and not yet dropped is a gesture an author has to be able
            // to abandon, and a mode where every click puts something down is another.
            self.selected.clear();
            self.cancel_manipulation();
            if self.placing {
                self.placing = false;
                self.status = "Select and move".to_owned();
            }
            return false;
        }
        if pressed(KeyCode::F1) {
            return self.execute(Command::ToggleShortcuts, scene);
        }
        if pressed(KeyCode::F3) {
            return self.execute(Command::ToggleDebug, scene);
        }
        if control {
            let command = if pressed(KeyCode::S) {
                Some(if shift {
                    Command::SaveAs
                } else {
                    Command::Save
                })
            } else if pressed(KeyCode::Y) || (shift && pressed(KeyCode::Z)) {
                Some(Command::Redo)
            } else if pressed(KeyCode::Z) {
                Some(Command::Undo)
            } else if pressed(KeyCode::A) {
                Some(Command::SelectAll)
            } else if pressed(KeyCode::D) {
                Some(Command::Duplicate)
            } else if pressed(KeyCode::C) {
                Some(Command::Copy)
            } else if pressed(KeyCode::V) {
                Some(Command::Paste)
            } else if pressed(KeyCode::G) {
                Some(Command::DropToGround)
            } else {
                None
            };
            return command.is_some_and(|command| self.execute(command, scene));
        }

        let command = if pressed(KeyCode::F) || pressed(KeyCode::Home) {
            Some(Command::Frame)
        } else if pressed(KeyCode::M) {
            Some(Command::ToggleModule)
        } else if pressed(KeyCode::B) {
            Some(Command::ToggleSculpt)
        } else if pressed(KeyCode::N) {
            Some(Command::TogglePlacing)
        } else if pressed(KeyCode::L) {
            Some(Command::ToggleSpace)
        } else if pressed(KeyCode::Digit1) {
            Some(Command::SetTool(Tool::Move))
        } else if pressed(KeyCode::Digit2) {
            Some(Command::SetTool(Tool::Turn))
        } else if pressed(KeyCode::Digit3) {
            Some(Command::SetTool(Tool::Scale))
        } else if pressed(KeyCode::H) {
            Some(if shift {
                Command::ShowAll
            } else {
                Command::Hide
            })
        } else if pressed(KeyCode::K) {
            Some(Command::ToggleLock)
        } else if pressed(KeyCode::X) || pressed(KeyCode::Delete) {
            Some(Command::Delete)
        } else if pressed(KeyCode::Digit0) {
            Some(Command::ResetTransform)
        } else {
            None
        };
        if let Some(command) = command {
            return self.execute(command, scene);
        }

        if pressed(KeyCode::G) {
            if shift {
                self.yaw_step = (self.yaw_step + 1) % YAW_STEPS.len();
                self.status = format!("Rotate by {} degrees", self.yaw_step());
            } else {
                self.grid = (self.grid + 1) % GRID_STEPS.len();
                self.snap = true;
                let step = self.grid();
                self.status = if step > 0.0 {
                    format!("Grid {step} m")
                } else {
                    "Grid off".to_owned()
                };
            }
            return false;
        }
        if pressed(KeyCode::Tab) {
            self.browser.select_next();
            self.status = format!(
                "Placing {}",
                theme::display_name(self.browser.selected_name())
            );
            return false;
        }
        if pressed(KeyCode::R) {
            return self.turn_step(scene, shift);
        }
        if pressed(KeyCode::T) {
            return self.scale_step(scene, shift);
        }
        if let Some(step) = self.nudge(input, shift) {
            return self.nudge_by(scene, step);
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

    /// Forgets any manipulation in progress: a handle being dragged, an asset being carried,
    /// a box being drawn.
    ///
    /// One function rather than fields cleared in eight places, because "the editor is no
    /// longer in the middle of something" is one idea — and a gesture left half-finished by a
    /// mode change is exactly how a click three frames later moves something nobody selected.
    fn cancel_manipulation(&mut self) {
        self.gizmo_drag = None;
        self.placement = None;
        self.gizmo_hover = None;
        self.marquee = None;
        self.place_at_centre = false;
    }

    /// Where the gizmo stands: the primary selection, or the middle of the selection when the
    /// pivot says so.
    #[must_use]
    pub fn gizmo_origin(&self, scene: &EnvironmentScene) -> Option<Vec3> {
        self.pivot_point(scene)
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
        // No handles in the two modes where the left button means something else, nor for
        // the select tool: a gizmo an author cannot grab is a lie about what a click will do.
        if self.sculpting || self.placing {
            return None;
        }
        let mode = self.tool.gizmo()?;
        let origin = self.pivot_point(scene)?;
        let length = gizmo::length_for(origin, self.camera.position);
        gizmo::project(
            mode,
            origin,
            length,
            self.primary_rotation(scene),
            self.camera.position,
            view_projection,
            viewport,
        )
    }

    /// Which handle the cursor is over, or the one being dragged while a drag is on.
    #[must_use]
    pub fn gizmo_hover(&self) -> Option<gizmo::Handle> {
        self.gizmo_drag
            .as_ref()
            .map(|drag| drag.handle)
            .or(self.gizmo_hover)
    }

    /// Which handle is being dragged, if one is.
    #[must_use]
    pub fn gizmo_axis(&self) -> Option<gizmo::Handle> {
        self.gizmo_drag.as_ref().map(|drag| drag.handle)
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
    /// The order is the whole point:
    ///
    /// 1. a gizmo drag or a selection box in progress owns the pointer until the button
    ///    comes up;
    /// 2. a press on a handle starts a drag;
    /// 3. a press on something in the world *selects it*, and never places anything;
    /// 4. a press on empty ground drops the selection and starts a selection box.
    ///
    /// Placing is not in this list. It is a drag out of the browser or a double click on a
    /// browser row — see [`Editor::update_placement`] — which is what makes "click a rock"
    /// mean one thing and never two.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site; the arguments are one frame's input"
    )]
    fn update_pointer(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
        add_to_selection: bool,
        precise: bool,
    ) -> bool {
        // A drag in progress owns the pointer until the button comes up, panels included: a
        // handle dragged over the browser is still that handle being dragged.
        if self.gizmo_drag.is_some() {
            return self.drag_gizmo(ui_input, scene, precise);
        }
        if self.marquee.is_some() {
            self.drag_marquee(ui_input, scene, view_projection, viewport);
            return false;
        }

        // Placing mode: every press in the world puts one down, and nothing else happens.
        // Deliberately *not* a pick against the scene first — an author laying out a row of
        // pillars wants the next one where the cursor is, including on top of the last one.
        if self.placing {
            if !ui_input.pointer_pressed || over_panel {
                return false;
            }
            let Some(ground) = self.drop_point(ui_input.pointer, scene, view_projection, viewport)
            else {
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
        if let (Some(handle), Some(screen)) = (self.gizmo_hover, screen) {
            let starts: Vec<(Selected, Pose)> = self
                .selected
                .iter()
                .filter_map(|what| pose_of(scene, *what).map(|pose| (*what, pose)))
                .collect();
            let pivot = self.pivot_point(scene).unwrap_or(Vec3::ZERO);
            self.gizmo_drag = Some(GizmoDrag {
                handle,
                from: ui_input.pointer,
                screen,
                starts,
                pivot,
                last_angle: screen.pointer_angle(ui_input.pointer),
                swept: 0.0,
                moved: false,
            });
            self.status = format!("{} {}", self.tool.label(), handle.label());
            return false;
        }

        let Some((origin, direction)) = cursor_ray(ui_input.pointer, view_projection, viewport)
        else {
            return false;
        };

        let hit = pick::under_ray_where(scene, origin, direction, |what| self.pickable(what));
        let Some(what) = hit else {
            // Empty ground. Nothing is placed and nothing is moved: the selection is dropped,
            // which is the one thing a click on nothing can mean without guessing — and the
            // press is the start of a selection box, if the pointer goes on to draw one.
            if !add_to_selection && !self.selected.is_empty() {
                self.selected.clear();
                self.status = "selection cleared".to_owned();
            }
            self.marquee = Some(Marquee {
                from: ui_input.pointer,
                to: ui_input.pointer,
                add: add_to_selection,
            });
            return false;
        };

        // CTRL toggles: control-clicking a selected object takes it out of the group again,
        // which is the one gesture every editor agrees on. A plain click on something already
        // selected keeps the group, so a group can be nudged or dragged by its gizmo without
        // a stray click on one of its members dissolving it.
        if add_to_selection || !self.is_selected(what) {
            self.select(what, add_to_selection);
        }
        // A double click frames what was clicked: the quickest way from "that rock over
        // there" to looking at it.
        let doubled = self
            .last_viewport_click
            .is_some_and(|(last, when)| last == what && self.clock - when <= DOUBLE_CLICK_SECONDS);
        self.last_viewport_click = Some((what, self.clock));
        if doubled && !add_to_selection {
            self.last_viewport_click = None;
            self.frame_selection(scene);
            return false;
        }
        self.status = if self.selected.len() > 1 {
            format!("{} selected", self.selected.len())
        } else {
            format!("Selected {}", panels::item_name(scene, what))
        };
        // Selecting never changes the level. That is what "a single click on an existing
        // object always means selection" costs: nothing.
        false
    }

    /// One frame of a selection box being dragged; on release, selects what is inside it.
    fn drag_marquee(
        &mut self,
        ui_input: &UiInput,
        scene: &EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
    ) {
        let Some(mut marquee) = self.marquee else {
            return;
        };
        marquee.to = ui_input.pointer;
        if ui_input.pointer_down {
            self.marquee = Some(marquee);
            return;
        }
        self.marquee = None;
        let rect = marquee.rect();
        let slop = self.scale.px(BOX_SLOP_PIXELS);
        if rect.width < slop && rect.height < slop {
            // A click, not a box: the press already dropped the selection.
            return;
        }
        let inside = pick::inside_screen_rect(
            scene,
            |point| {
                engine_presentation::nameplate::project(point, view_projection, viewport)
                    .is_some_and(|(x, y)| rect.contains(x, y))
            },
            |what| self.pickable(what),
        );
        // One kind, as always: the kind the selection already is when adding to it, and
        // otherwise the first kind the box caught, in hierarchy order — placements first.
        let kind = if marquee.add {
            self.selected.first().copied()
        } else {
            None
        }
        .or_else(|| inside.first().copied());
        let Some(kind) = kind else {
            self.status = "nothing inside the box".to_owned();
            return;
        };
        if !marquee.add {
            self.selected.clear();
        }
        for what in inside {
            if what.same_kind(kind) && !self.selected.contains(&what) {
                self.selected.push(what);
            }
        }
        self.status = format!("selected {}", self.selected.len());
    }

    /// One frame of a gizmo drag. Returns whether the scene changed.
    ///
    /// `precise` (`CTRL` held) turns the snapping off for this frame, for the placement that
    /// has to sit between two grid lines.
    fn drag_gizmo(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        precise: bool,
    ) -> bool {
        let Some(mut drag) = self.gizmo_drag.clone() else {
            return false;
        };
        if !ui_input.pointer_down {
            self.gizmo_drag = None;
            return false;
        }
        let primary = self.selection();
        let primary_start = drag
            .starts
            .iter()
            .find(|(what, _)| Some(*what) == primary)
            .map(|(_, pose)| *pose);

        // Every target is worked out before anything moves, so the undo snapshot is the scene
        // as it was before this frame and not as it was halfway through it.
        let mut targets: Vec<(Selected, Pose)> = Vec::with_capacity(drag.starts.len());
        let describe;
        match (drag.screen.mode, drag.handle) {
            (gizmo::GizmoMode::Rotate, _) => {
                if let (Some(last), Some(now)) =
                    (drag.last_angle, drag.screen.pointer_angle(ui_input.pointer))
                {
                    drag.swept += gizmo::wrap_angle(now - last);
                    drag.last_angle = Some(now);
                } else if drag.last_angle.is_none() {
                    drag.last_angle = drag.screen.pointer_angle(ui_input.pointer);
                }
                let mut turn = drag.screen.yaw_for(drag.swept);
                if !precise && let Some(start) = primary_start {
                    let step = self.yaw_step().to_radians();
                    turn = snap_angle(start.yaw + turn, step) - start.yaw;
                }
                let rotation = Quat::from_yaw_pitch_roll(turn, 0.0, 0.0);
                for (what, start) in &drag.starts {
                    targets.push((
                        *what,
                        Pose {
                            position: drag.pivot + rotation.rotate(start.position - drag.pivot),
                            yaw: start.yaw + turn,
                            ..*start
                        },
                    ));
                }
                describe = format!("Rotate {:+.1} degrees", turn.to_degrees());
            }
            (gizmo::GizmoMode::Scale, handle) => {
                let factor = match handle {
                    gizmo::Handle::Axis(axis) => {
                        let metres = drag.screen.metres(axis, drag.from, ui_input.pointer);
                        1.0 + metres / drag.screen.length.max(1e-3)
                    }
                    _ => {
                        let rise = drag.from.1 - ui_input.pointer.1;
                        2.0_f32.powf(rise / self.scale.px(UNIFORM_SCALE_PIXELS).max(1.0))
                    }
                }
                .max(0.01);
                for (what, start) in &drag.starts {
                    let component = |value: f32| {
                        let scaled = (value * factor).max(0.01);
                        if precise {
                            scaled
                        } else {
                            ((scaled / SCALE_SNAP).round() * SCALE_SNAP).max(SCALE_SNAP)
                        }
                    };
                    let mut scale = start.scale;
                    match (handle, what) {
                        // A light's size is one number, whichever handle stretches it.
                        (gizmo::Handle::Axis(axis), Selected::Object(_) | Selected::Water(_)) => {
                            match axis {
                                gizmo::Axis::X => scale.x = component(scale.x),
                                gizmo::Axis::Y => scale.y = component(scale.y),
                                gizmo::Axis::Z => scale.z = component(scale.z),
                            }
                        }
                        _ => {
                            scale = Vec3::new(
                                component(scale.x),
                                component(scale.y),
                                component(scale.z),
                            );
                        }
                    }
                    targets.push((*what, Pose { scale, ..*start }));
                }
                // The scale the primary ended up at, snapped, rather than the raw factor.
                let shown = targets
                    .iter()
                    .find(|(what, _)| Some(*what) == primary)
                    .or(targets.first())
                    .map_or(Vec3::ONE, |(_, pose)| pose.scale);
                describe = match handle {
                    gizmo::Handle::Axis(gizmo::Axis::X) => format!("Scale X {:.2}", shown.x),
                    gizmo::Handle::Axis(gizmo::Axis::Y) => format!("Scale Y {:.2}", shown.y),
                    gizmo::Handle::Axis(gizmo::Axis::Z) => format!("Scale Z {:.2}", shown.z),
                    _ => format!("Scale {:.2} {:.2} {:.2}", shown.x, shown.y, shown.z),
                };
            }
            (gizmo::GizmoMode::Translate, handle) => {
                let gizmo::Handle::Axis(axis) = handle else {
                    return false;
                };
                let metres = drag.screen.metres(axis, drag.from, ui_input.pointer);
                let step = axis.direction() * metres;
                for (what, start) in &drag.starts {
                    let moved = start.position + step;
                    let position = if precise {
                        moved
                    } else {
                        self.snap_along(axis, moved)
                    };
                    targets.push((*what, Pose { position, ..*start }));
                }
                // What was actually applied, snapped, rather than how far the hand moved.
                let applied = targets.iter().zip(&drag.starts).next().map_or(
                    0.0,
                    |((_, target), (_, start))| {
                        (target.position - start.position).dot(axis.direction())
                    },
                );
                describe = if drag.starts.len() > 1 {
                    format!(
                        "{} {applied:+.2} m ({} items)",
                        axis.label(),
                        drag.starts.len()
                    )
                } else {
                    format!("{} {applied:+.2} m", axis.label())
                };
            }
        }

        targets.retain(|(what, target)| pose_of(scene, *what) != Some(*target));
        if targets.is_empty() {
            self.gizmo_drag = Some(drag);
            return false;
        }
        if !drag.moved {
            // Taken on the first movement rather than on the press: a click on a handle that
            // does not drag it must not fill the undo stack.
            let before = Snapshot::of(scene, &self.flags);
            self.push_undo(before);
            drag.moved = true;
        }
        for (what, pose) in &targets {
            set_pose(scene, *what, *pose);
        }
        // A selection holds one kind, so this frame's edit reached the placements and lights
        // or the other two lists and never both — which is the answer the application needs.
        self.water_touched = drag.starts.iter().all(|(what, _)| !what.is_scene_kind());
        self.status = describe;
        self.gizmo_drag = Some(drag);
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
            let position = self
                .drop_point(centre, scene, view_projection, viewport)
                .unwrap_or_else(|| {
                    // Looking at the sky: the ground is not in front of the camera at all, so
                    // the asset goes down in front of where the camera is standing rather
                    // than nowhere.
                    let ahead = self.camera.position + self.camera.forward() * 12.0;
                    Vec3::new(ahead.x, scene.ground_at(ahead), ahead.z)
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
        let Some(ground) = self.drop_point(ui_input.pointer, scene, view_projection, viewport)
        else {
            self.status = "point at the ground to drop it".to_owned();
            return false;
        };
        self.place(scene, ground)
    }

    /// What something put down under `pointer` stands on: the top of a solid thing if the
    /// pointer is over one, and the ground — the terrain where the level has one — otherwise.
    fn drop_point(
        &self,
        pointer: (f32, f32),
        scene: &EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
    ) -> Option<Vec3> {
        let ground = ground_under(pointer, view_projection, viewport, scene);
        let Some((origin, direction)) = cursor_ray(pointer, view_projection, viewport) else {
            return ground;
        };
        pick::surface_under(scene, origin, direction, ground, |index| {
            self.is_hidden(index)
        })
    }

    /// Puts the browser's asset into the scene at `position` and selects it.
    ///
    /// One function for every way of placing, so a dropped asset and a double-clicked one land
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
        self.flags.push(0);
        self.selected.clear();
        self.selected
            .push(Selected::Object(scene.objects.len() - 1));
        self.status = format!("Placed {}", theme::display_name(asset.name()));
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
    /// Height is never snapped: everything in this game stands on something, and rounding
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

    /// Remembers the scene as it is now, for `CTRL+Z`.
    fn snapshot(&mut self, scene: &EnvironmentScene) {
        let snapshot = Snapshot::of(scene, &self.flags);
        self.push_undo(snapshot);
    }

    /// Pushes a snapshot, dropping the oldest once the stack is full.
    ///
    /// Every edit comes through here, which makes it the one place an edit is counted, and
    /// the one place a new edit throws away what could have been redone.
    fn push_undo(&mut self, snapshot: Snapshot) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.redo.clear();
        self.revision += 1;
    }

    /// Puts the last snapshot back. Returns whether anything changed.
    fn undo(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(snapshot) = self.undo.pop() else {
            self.status = "nothing to undo".to_owned();
            return false;
        };
        self.redo.push(Snapshot::of(scene, &self.flags));
        self.flags = snapshot.restore(scene);
        self.after_history_step(scene);
        self.status = format!("undone ({} more)", self.undo.len());
        true
    }

    /// Puts back what the last undo took away. Returns whether anything changed.
    fn redo(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(snapshot) = self.redo.pop() else {
            self.status = "nothing to redo".to_owned();
            return false;
        };
        self.undo.push(Snapshot::of(scene, &self.flags));
        self.flags = snapshot.restore(scene);
        self.after_history_step(scene);
        self.status = format!("redone ({} more)", self.redo.len());
        true
    }

    /// What every step through the history leaves behind: a new revision, a selection that
    /// may no longer point at anything, and no gesture in progress.
    fn after_history_step(&mut self, scene: &EnvironmentScene) {
        self.revision += 1;
        self.sync_flags(scene);
        self.prune_selection(scene);
        self.cancel_manipulation();
        self.inspector_edit = None;
    }

    /// Writes the scene, and says so in the status line.
    ///
    /// A failure is reported in the status line rather than returned: the author is standing
    /// in the level with the file dialog they do not have, and the useful thing is to see
    /// "could not save" on screen immediately. It is logged as an error too.
    fn save(&mut self, scene: &EnvironmentScene) {
        match write_scene(scene, &self.save_path) {
            Ok(()) => {
                log::info!(
                    "editor: wrote {} ({} objects) to {}",
                    scene.name,
                    scene.len(),
                    self.save_path.display()
                );
                self.saved_revision = self.revision;
                self.autosaved_revision = self.revision;
                self.status = format!("saved {}", self.save_path.display());
                // The autosave is older than what was just written, and an autosave that is
                // older than the level is a trap for whoever finds it next.
                let autosave = self.autosave_path();
                if autosave.is_file() {
                    std::fs::remove_file(&autosave).ok();
                }
            }
            Err(error) => {
                log::error!("editor: could not save: {error}");
                self.status = format!("COULD NOT SAVE: {error}");
            }
        }
    }

    /// Writes unsaved work to [`Editor::autosave_path`], if there is any the autosave does not
    /// already hold. Returns whether it wrote.
    ///
    /// Called on the way out with F6, every couple of minutes while there is unsaved work,
    /// and by the application when the game closes.
    pub fn autosave(&mut self, scene: &EnvironmentScene) -> bool {
        if !self.is_dirty() || self.autosaved_revision == self.revision {
            return false;
        }
        let path = self.autosave_path();
        match write_scene(scene, &path) {
            Ok(()) => {
                self.autosaved_revision = self.revision;
                self.since_autosave = 0.0;
                log::info!("editor: unsaved work autosaved to {}", path.display());
                true
            }
            Err(error) => {
                log::error!("editor: could not autosave: {error}");
                false
            }
        }
    }

    /// Counts how long unsaved work has waited, and autosaves it when it has waited too long.
    fn tick_autosave(&mut self, scene: &EnvironmentScene, seconds: f32) {
        if !self.is_dirty() || self.autosaved_revision == self.revision {
            self.since_autosave = 0.0;
            return;
        }
        self.since_autosave += seconds;
        if self.since_autosave >= AUTOSAVE_SECONDS && self.autosave(scene) {
            self.status = format!("autosaved to {}", self.autosave_path().display());
        }
    }
}

/// Writes `scene` to `path` as the editor writes every level: without the collision shapes
/// no placement names any more.
fn write_scene(scene: &EnvironmentScene, path: &Path) -> Result<(), scene_file::SceneFileError> {
    let mut written = scene.clone();
    let dropped = written.prune_collision_shapes();
    if dropped > 0 {
        log::debug!("editor: {dropped} unused collision shape(s) left out of the file");
    }
    scene_file::save(&written, path)
}

/// The ground under a screen point: the terrain where the level has one and the pointer is
/// over it, and the level's floor everywhere else.
///
/// The terrain is marched rather than intersected, so a ray that would clear a ridge stops on
/// it — the author is pointing at the ground they can see, which on a hill is not where a flat
/// plane at the floor's height would put it.
fn ground_under(
    pointer: (f32, f32),
    view_projection: &Mat4,
    viewport: (u32, u32),
    scene: &EnvironmentScene,
) -> Option<Vec3> {
    if let Some(terrain) = scene.terrain.as_ref()
        && let Some(hit) = ground_under_cursor(
            pointer,
            view_projection,
            viewport,
            &GroundSurface::Terrain(terrain),
        )
        && terrain.contains(hit)
    {
        return Some(hit);
    }
    ground_under_cursor(
        pointer,
        view_projection,
        viewport,
        &GroundSurface::Plane(scene.ground_height),
    )
}

/// A number as the inspector writes it: as few decimals as it needs, at most three.
fn format_value(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// The index a selection names in its own list.
fn index_of(what: Selected) -> usize {
    match what {
        Selected::Object(index)
        | Selected::Water(index)
        | Selected::Spawn(index)
        | Selected::Light(index) => index,
    }
}

/// Where something stands, whichever of the level's lists it lives in.
///
/// `None` for an index that is past the end of its list, which is what a drag whose target
/// was deleted under it looks like.
#[must_use]
pub(crate) fn position_of(scene: &EnvironmentScene, what: Selected) -> Option<Vec3> {
    match what {
        Selected::Object(index) => scene
            .objects
            .get(index)
            .map(|object| object.transform.translation),
        Selected::Water(index) => scene.water.get(index).map(|body| body.center),
        Selected::Spawn(index) => scene.spawn_points.get(index).map(|spawn| spawn.position),
        Selected::Light(index) => scene.lights.get(index).map(|light| light.position),
    }
}

/// Moves something by `delta`, whichever list it lives in.
pub(crate) fn move_by(scene: &mut EnvironmentScene, what: Selected, delta: Vec3) {
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
        Selected::Light(index) => {
            if let Some(light) = scene.lights.get_mut(index) {
                light.position += delta;
            }
        }
    }
}

/// How something stands. See [`Pose`].
fn pose_of(scene: &EnvironmentScene, what: Selected) -> Option<Pose> {
    Some(match what {
        Selected::Object(index) => {
            let object = scene.objects.get(index)?;
            Pose {
                position: object.transform.translation,
                yaw: yaw_of(object.transform),
                scale: object.transform.scale,
            }
        }
        Selected::Water(index) => {
            let body = scene.water.get(index)?;
            Pose {
                position: body.center,
                yaw: body.yaw,
                scale: Vec3::new(body.half_extents.x, body.depth, body.half_extents.y),
            }
        }
        Selected::Spawn(index) => {
            let spawn = scene.spawn_points.get(index)?;
            Pose {
                position: spawn.position,
                yaw: spawn.yaw,
                scale: Vec3::ONE,
            }
        }
        Selected::Light(index) => {
            let light = scene.lights.get(index)?;
            Pose {
                position: light.position,
                yaw: 0.0,
                scale: Vec3::splat(light.range),
            }
        }
    })
}

/// Makes something stand as `pose` says, within what its kind can be.
///
/// Nothing is ever shrunk to where the loader would refuse it: a placement keeps a scale, a
/// body of water keeps a footprint and a depth, a light keeps a reach.
fn set_pose(scene: &mut EnvironmentScene, what: Selected, pose: Pose) {
    match what {
        Selected::Object(index) => {
            if let Some(object) = scene.objects.get_mut(index) {
                object.transform.translation = pose.position;
                object.transform.rotation = Quat::from_yaw_pitch_roll(pose.yaw, 0.0, 0.0);
                object.transform.scale = pose.scale.max(Vec3::splat(0.01));
            }
        }
        Selected::Water(index) => {
            if let Some(body) = scene.water.get_mut(index) {
                body.center = pose.position;
                body.yaw = pose.yaw;
                body.half_extents.x = pose.scale.x.max(WATER_MIN_HALF_EXTENT);
                body.half_extents.y = pose.scale.z.max(WATER_MIN_HALF_EXTENT);
                body.depth = pose.scale.y.max(0.1);
            }
        }
        Selected::Spawn(index) => {
            if let Some(spawn) = scene.spawn_points.get_mut(index) {
                spawn.position = pose.position;
                spawn.yaw = pose.yaw;
            }
        }
        Selected::Light(index) => {
            if let Some(light) = scene.lights.get_mut(index) {
                light.position = pose.position;
                light.range = pose.scale.x.max(0.1);
            }
        }
    }
}

/// How many rows the unfiltered hierarchy has: every placement, the water, the spawn points
/// and the lights.
#[must_use]
pub fn hierarchy_len(scene: &EnvironmentScene) -> usize {
    scene.objects.len() + scene.water.len() + scene.spawn_points.len() + scene.lights.len()
}

/// What the unfiltered hierarchy's `row`th line names, if it names anything.
///
/// Placements first, so an object's row is its index in the file.
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
    if spawn < scene.spawn_points.len() {
        return Some(Selected::Spawn(spawn));
    }
    let light = spawn - scene.spawn_points.len();
    (light < scene.lights.len()).then_some(Selected::Light(light))
}

/// Which unfiltered hierarchy row names `what`. The inverse of [`hierarchy_row_at`].
#[must_use]
pub fn hierarchy_row_of(what: Selected, scene: &EnvironmentScene) -> usize {
    match what {
        Selected::Object(index) => index,
        Selected::Water(index) => scene.objects.len() + index,
        Selected::Spawn(index) => scene.objects.len() + scene.water.len() + index,
        Selected::Light(index) => {
            scene.objects.len() + scene.water.len() + scene.spawn_points.len() + index
        }
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

    impl Editor {
        /// Every selected body of water, in the order it was added.
        fn selected_waters(&self) -> Vec<usize> {
            self.selected
                .iter()
                .filter_map(|what| what.water())
                .collect()
        }
    }

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
        // Never the repository: F6 autosaves, and a test that leaves the editor would
        // otherwise write a level into the crate's own directory.
        editor.save_path = scratch_path("test_level.json");
        editor
    }

    /// A file in a directory of this test run's own, so parallel tests never share one.
    fn scratch_path(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir()
            .join("mygame_editor_tests")
            .join(format!(
                "{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&directory).expect("a temp directory");
        directory.join(name)
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
        let folder = editor.layout_for(VIEWPORT).browser_row(0);
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
        let visible = editor.layout_for(VIEWPORT).browser_rows();
        let first = editor.browser().scroll();
        for slot in 0..visible {
            let Some(row) = editor.browser().row(first + slot) else {
                break;
            };
            if editor.browser().is_selected(row) {
                let rect = editor.layout_for(VIEWPORT).browser_row(slot);
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
        let rect = editor.layout_for(VIEWPORT).hierarchy_row(slot);
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
        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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
        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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
        let point = item_point(&editor, &scene, Selected::Object(0));
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
        let water_row = item_point(&editor, &scene, Selected::Water(0));
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
        let point = item_point(&editor, &scene, Selected::Object(2));

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
        let visible = editor.layout_for(VIEWPORT).hierarchy_rows();
        let mut scene = scene_of(visible + 30);
        let over = hierarchy_point(&editor, 0);
        let hover = UiInput {
            pointer: over,
            ..UiInput::new()
        };

        // Down four notches — three rows each — then far more than the list is long.
        update(&mut editor, &wheeled(-4.0), &hover, &mut scene);
        assert_eq!(editor.hierarchy_scroll(), 12);
        update(&mut editor, &wheeled(-1_000.0), &hover, &mut scene);
        // Every rock is in one group, so the list is the objects and one heading.
        assert_eq!(
            editor.hierarchy_scroll(),
            scene.len() + 1 - visible,
            "the list scrolled past its own end"
        );
        update(&mut editor, &wheeled(1_000.0), &hover, &mut scene);
        assert_eq!(editor.hierarchy_scroll(), 0);
    }

    #[test]
    fn scrolling_the_hierarchy_selects_the_row_that_is_actually_under_the_cursor() {
        let mut editor = editor();
        let visible = editor.layout_for(VIEWPORT).hierarchy_rows();
        let mut scene = scene_of(visible + 20);
        let over = hierarchy_point(&editor, 1);
        let hover = UiInput {
            pointer: over,
            ..UiInput::new()
        };
        update(&mut editor, &wheeled(-3.0), &hover, &mut scene);

        update(&mut editor, &InputState::new(), &press(over), &mut scene);
        // Three notches is nine rows; row 1 under the pointer is the tenth line, and the
        // first line is the group's heading.
        assert_eq!(editor.hierarchy_scroll(), 9);
        assert_eq!(
            editor.selected_object(),
            Some(9),
            "row 1 of a list scrolled by 9"
        );
    }

    #[test]
    fn the_wheel_outside_the_panel_scrolls_nothing() {
        let mut editor = editor();
        let visible = editor.layout_for(VIEWPORT).hierarchy_rows();
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
        let visible = editor.layout_for(VIEWPORT).hierarchy_rows();
        let mut scene = scene_of(visible + 6);
        // Place one more: it becomes the last object and the selection.
        drop_asset(&mut editor, &mut scene, centre());
        let last = scene.len() - 1;
        assert_eq!(editor.selected_object(), Some(last));
        let row = editor
            .hierarchy_view(&scene)
            .iter()
            .position(|row| *row == panels::HierarchyRow::Item(Selected::Object(last)))
            .expect("listed");
        assert!(
            row >= editor.hierarchy_scroll() && row < editor.hierarchy_scroll() + visible,
            "the selected object is outside the visible part of the list"
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
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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
        let panel = editor.layout_for(VIEWPORT).left;
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

        let panel = editor.layout_for(VIEWPORT).hierarchy;
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

        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.objects, before, "the placement was taken back");

        // And an empty stack is not an error.
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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
        let folder = editor.layout_for(VIEWPORT).browser_row(0);
        update(
            &mut editor,
            &InputState::new(),
            &press((
                folder.x + folder.width * 0.5,
                folder.y + folder.height * 0.5,
            )),
            &mut scene,
        );
        let rect = editor.layout_for(VIEWPORT).browser_row(4);
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

        let rect = editor.layout_for(VIEWPORT).browser_row(0);
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

        update(
            &mut editor,
            &control_keyed(KeyCode::F),
            &UiInput::new(),
            &mut scene,
        );
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

        let point = item_point(&editor, &scene, Selected::Object(0));
        update(&mut editor, &InputState::new(), &press(point), &mut scene);
        assert_eq!(objects_of(&editor), [0]);

        let second = item_point(&editor, &scene, Selected::Object(2));
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
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::D),
            &UiInput::new(),
            &mut scene,
        );
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
        let first = item_point(&editor, &scene, Selected::Object(0));
        let third = item_point(&editor, &scene, Selected::Object(2));
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
        let point = item_point(&editor, &scene, Selected::Object(0));
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
        let point = item_point(&editor, &scene, Selected::Object(0));
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
        let point = item_point(&editor, &scene, Selected::Object(0));
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
        let point = item_point(&editor, &scene, Selected::Object(0));
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

        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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

        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
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

    // -----------------------------------------------------------------------------------
    // The usability pass: terrain-aware pointing, history, saving, and the new tools.
    // -----------------------------------------------------------------------------------

    /// The middle of the inspector's field for `property`.
    fn value_point(
        editor: &Editor,
        scene: &EnvironmentScene,
        property: inspector::Property,
    ) -> (f32, f32) {
        let layout = editor.layout_for(VIEWPORT);
        panels::inspector(editor, scene, &layout)
            .into_iter()
            .find_map(|element| match element {
                panels::Element::Value {
                    rect,
                    property: shown,
                } if shown == property => {
                    Some((rect.x + rect.width * 0.5, rect.y + rect.height * 0.5))
                }
                _ => None,
            })
            .expect("the inspector shows that number")
    }

    /// The middle of the hierarchy row that lists `what`.
    fn item_point(editor: &Editor, scene: &EnvironmentScene, what: Selected) -> (f32, f32) {
        let slot = editor
            .hierarchy_view(scene)
            .iter()
            .position(|row| *row == panels::HierarchyRow::Item(what))
            .expect("the hierarchy lists it")
            - editor.hierarchy_scroll();
        let rect = editor.layout_for(VIEWPORT).hierarchy_row(slot);
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// A frame in which every one of `keys` went down together.
    fn chord(keys: &[KeyCode]) -> InputState {
        let mut input = InputState::new();
        for key in keys {
            input.apply(engine_core::input::InputEvent::KeyDown {
                key: *key,
                repeat: false,
            });
        }
        input
    }

    /// Where a world point is on screen from the overhead camera.
    fn on_screen(point: Vec3) -> (f32, f32) {
        engine_presentation::nameplate::project(point, &overhead(), VIEWPORT)
            .expect("the point is in view")
    }

    /// Selects the object under the middle of the overhead camera.
    fn click_centre(editor: &mut Editor, scene: &mut EnvironmentScene) {
        update(editor, &InputState::new(), &press(centre()), scene);
        update(editor, &InputState::new(), &release(centre()), scene);
    }

    #[test]
    fn an_asset_dropped_on_a_hill_lands_on_the_hill_where_the_cursor_points() {
        let mut editor = editor();
        let mut scene = scene();
        // A plateau ten metres up, eighty wide: the flat plane at the floor's height is
        // twenty metres from the eye and the plateau ten, so a slanted ray meets the plateau
        // half as far out.
        scene.terrain = Some(Terrain::flat(41, 2.0, 10.0));
        let pointer = (centre().0 + 300.0, centre().1);
        let floor = ground_under_cursor(pointer, &overhead(), VIEWPORT, &GroundSurface::Plane(0.0))
            .expect("the floor is under the pointer");

        drop_asset(&mut editor, &mut scene, pointer);
        let placed = scene.objects[0].position();
        assert!((placed.y - 10.0).abs() < 0.01, "buried at {placed:?}");
        assert!(
            (placed.x - floor.x * 0.5).abs() < 0.6,
            "landed at {placed:?}, not under the cursor (the floor says {floor:?})"
        );
    }

    #[test]
    fn an_asset_dropped_on_a_pillar_stands_on_top_of_it() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
                .with_collision(default_collision(PILLAR)),
        );
        drop_asset(&mut editor, &mut scene, centre());
        let placed = scene.objects[1].position();
        assert!(
            placed.y > 1.0,
            "the drop fell through the pillar to {placed:?}"
        );
    }

    #[test]
    fn control_g_drops_the_selection_onto_what_is_under_it() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(
            ROCK_SMALL,
            Vec3::new(0.0, 5.0, 0.0),
        ));
        editor.select(Selected::Object(0), false);
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::G),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert!(scene.objects[0].position().y.abs() < 1e-4);
    }

    #[test]
    fn undo_and_redo_walk_both_ways_and_a_new_edit_forgets_what_could_be_redone() {
        let mut editor = editor();
        let mut scene = scene();
        drop_asset(&mut editor, &mut scene, centre());
        assert_eq!(scene.len(), 1);

        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.len(), 0);
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::Y),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 1, "CTRL+Y put nothing back");

        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        drop_asset(&mut editor, &mut scene, centre());
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::Y),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(
            outcome,
            EditorOutcome::Idle,
            "a new edit left something to redo"
        );
    }

    #[test]
    fn a_plain_z_or_c_is_not_an_edit() {
        // One slip of the hand used to undo or duplicate. They are chords now.
        let mut editor = editor();
        let mut scene = scene();
        drop_asset(&mut editor, &mut scene, centre());
        update(&mut editor, &keyed(KeyCode::Z), &UiInput::new(), &mut scene);
        update(&mut editor, &keyed(KeyCode::C), &UiInput::new(), &mut scene);
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn leaving_to_play_and_coming_back_keeps_the_undo_history() {
        let mut editor = editor();
        let mut scene = scene();
        drop_asset(&mut editor, &mut scene, centre());
        update(
            &mut editor,
            &keyed(KeyCode::F6),
            &UiInput::new(),
            &mut scene,
        );
        assert!(!editor.is_active());

        editor.enter(Vec3::ZERO, "test_level", VIEWPORT);
        assert!(editor.is_dirty(), "the unsaved edit was forgotten");
        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(
            scene.len(),
            0,
            "the placement from before F6 could not be undone"
        );

        // A different level starts clean.
        editor.enter(Vec3::ZERO, "another_level", VIEWPORT);
        assert!(!editor.is_dirty());
        assert!(editor.undo.is_empty());
    }

    #[test]
    fn unsaved_work_is_counted_saved_and_autosaved_on_the_way_out() {
        let mut editor = editor();
        let mut scene = scene();
        assert!(!editor.is_dirty());
        drop_asset(&mut editor, &mut scene, centre());
        assert!(editor.is_dirty());

        update(
            &mut editor,
            &control_keyed(KeyCode::S),
            &UiInput::new(),
            &mut scene,
        );
        assert!(!editor.is_dirty(), "{}", editor.status());

        drop_asset(&mut editor, &mut scene, (centre().0 + 200.0, centre().1));
        update(
            &mut editor,
            &keyed(KeyCode::F6),
            &UiInput::new(),
            &mut scene,
        );
        let autosave = editor.autosave_path();
        let saved = scene_file::load(&autosave).expect("F6 wrote the autosave");
        assert_eq!(saved.len(), 2);

        // Saving makes the autosave stale, and a stale autosave is removed.
        editor.enter(Vec3::ZERO, "test_level", VIEWPORT);
        update(
            &mut editor,
            &control_keyed(KeyCode::S),
            &UiInput::new(),
            &mut scene,
        );
        assert!(
            !autosave.is_file(),
            "an autosave older than the level was left behind"
        );
    }

    #[test]
    fn a_saved_level_carries_only_the_collision_shapes_its_placements_use() {
        let mut editor = editor();
        let mut scene = scene();
        let unused = engine_environment::object::CollisionShape::new([
            engine_environment::CollisionBox::new(Vec3::ZERO, Vec3::ONE),
        ]);
        scene.intern_collision(unused);
        scene.push(EnvironmentObject::decoration(PILLAR, Vec3::ZERO));
        update(
            &mut editor,
            &control_keyed(KeyCode::S),
            &UiInput::new(),
            &mut scene,
        );
        let saved = scene_file::load(editor.save_path()).expect("the level saved");
        assert!(saved.collision_shapes.is_empty());
        assert_eq!(
            scene.collision_shapes.len(),
            1,
            "the level being edited is untouched"
        );
    }

    #[test]
    fn save_as_writes_a_new_file_and_renames_the_level() {
        let mut editor = editor();
        let mut scene = scene();
        scene.push(EnvironmentObject::decoration(PILLAR, Vec3::ZERO));
        update(
            &mut editor,
            &chord(&[KeyCode::LeftControl, KeyCode::LeftShift, KeyCode::S]),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.save_as_text().is_some(), "the prompt did not open");
        // Typing replaces the name it opened with, and W does not fly the camera.
        let eye = editor.eye();
        for key in [
            KeyCode::W,
            KeyCode::E,
            KeyCode::S,
            KeyCode::T,
            KeyCode::Enter,
        ] {
            update(&mut editor, &keyed(key), &UiInput::new(), &mut scene);
        }
        assert_eq!(editor.eye(), eye);
        assert_eq!(scene.name, "west");
        assert_eq!(
            editor
                .save_path()
                .file_name()
                .and_then(|name| name.to_str()),
            Some("west.json")
        );
        assert!(editor.save_path().is_file());
    }

    #[test]
    fn control_s_saves_without_flying_the_camera_backwards() {
        let mut editor = editor();
        let mut scene = scene();
        let eye = editor.eye();
        update(
            &mut editor,
            &control_keyed(KeyCode::S),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(editor.eye(), eye);
    }

    #[test]
    fn opening_another_level_over_unsaved_work_asks_first() {
        let mut editor = editor();
        let mut scene = scene();
        drop_asset(&mut editor, &mut scene, centre());
        let path = PathBuf::from("assets/scenes/elsewhere.json");
        editor.request_open(path.clone());
        assert_eq!(editor.take_open_request(), None);
        assert_eq!(editor.modal(), Some(&Modal::Unsaved { path: path.clone() }));

        // Discard is the button that opens without saving.
        let layout = editor.layout_for(VIEWPORT);
        let dialog = panels::dialog(&layout, panels::DialogKind::Unsaved);
        let (discard, ..) = dialog
            .buttons
            .iter()
            .find(|(_, button, ..)| *button == panels::DialogButton::Discard)
            .copied()
            .expect("a discard button");
        let point = (
            discard.x + discard.width * 0.5,
            discard.y + discard.height * 0.5,
        );
        update(&mut editor, &InputState::new(), &press(point), &mut scene);
        assert!(editor.modal().is_none());
        assert_eq!(editor.take_open_request(), Some(path.clone()));

        // And the editor starts the opened level clean.
        editor.opened(&EnvironmentScene::empty("elsewhere"), path.clone());
        assert!(!editor.is_dirty());
        assert_eq!(editor.save_path(), path.as_path());
    }

    #[test]
    fn a_hidden_object_is_not_drawn_or_picked_and_shift_h_brings_it_back() {
        let mut editor = editor();
        let mut scene = three_rocks();
        editor.select(Selected::Object(0), false);
        let outcome = update(&mut editor, &keyed(KeyCode::H), &UiInput::new(), &mut scene);
        assert_eq!(
            outcome,
            EditorOutcome::SceneChanged,
            "what is drawn changed"
        );
        assert!(editor.is_hidden(0));
        assert_eq!(scene.len(), 3, "hiding is a view, not a delete");
        assert_eq!(
            editor.visible_scene(&scene).map(|visible| visible.len()),
            Some(2)
        );

        click_centre(&mut editor, &mut scene);
        assert_eq!(
            editor.selection(),
            None,
            "a click landed on something hidden"
        );

        update(
            &mut editor,
            &shift_keyed(KeyCode::H),
            &UiInput::new(),
            &mut scene,
        );
        assert!(!editor.has_hidden());
        assert!(editor.visible_scene(&scene).is_none());
    }

    #[test]
    fn a_locked_object_lets_clicks_through_but_stays_in_the_hierarchy() {
        let mut editor = editor();
        let mut scene = three_rocks();
        editor.select(Selected::Object(0), false);
        update(&mut editor, &keyed(KeyCode::K), &UiInput::new(), &mut scene);
        assert!(editor.is_locked(0));
        editor.selected.clear();

        click_centre(&mut editor, &mut scene);
        assert_eq!(editor.selection(), None);
        assert!(
            editor
                .hierarchy_items(&scene)
                .contains(&Selected::Object(0))
        );
    }

    #[test]
    fn a_box_dragged_over_empty_ground_selects_what_is_inside_it() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let from = on_screen(Vec3::new(-2.0, 0.0, -2.0));
        let to = on_screen(Vec3::new(6.0, 0.0, 2.0));
        update(&mut editor, &InputState::new(), &press(from), &mut scene);
        assert!(editor.marquee().is_some(), "the press did not start a box");
        update(&mut editor, &InputState::new(), &hold(to), &mut scene);
        update(&mut editor, &InputState::new(), &release(to), &mut scene);
        assert_eq!(
            objects_of(&editor),
            vec![0, 1],
            "the rock at x = 8 is outside the box"
        );
        assert!(editor.marquee().is_none());
    }

    #[test]
    fn the_turn_ring_turns_the_selection_by_snapped_steps_and_undoes_in_one() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        scene.objects[0].transform.translation = Vec3::ZERO;
        editor.select(Selected::Object(0), false);
        update(
            &mut editor,
            &keyed(KeyCode::Digit2),
            &UiInput::new(),
            &mut scene,
        );
        // One frame so the gizmo is projected from this frame's camera.
        update(
            &mut editor,
            &InputState::new(),
            &hover(centre()),
            &mut scene,
        );

        let gizmo = editor.gizmo(&scene).expect("a gizmo");
        let (grab, _) = gizmo.ring_segments().next().expect("a ring");
        let (dx, dy) = (grab.0 - gizmo.origin.0, grab.1 - gizmo.origin.1);
        // Forty-five degrees counter-clockwise on screen, in small steps as a hand would.
        let mut turned = grab;
        update(&mut editor, &InputState::new(), &press(grab), &mut scene);
        for step in 1..=9 {
            let angle = (step as f32 * 5.0).to_radians();
            let (sin, cos) = angle.sin_cos();
            turned = (
                gizmo.origin.0 + dx * cos + dy * sin,
                gizmo.origin.1 - dx * sin + dy * cos,
            );
            update(&mut editor, &InputState::new(), &hold(turned), &mut scene);
        }
        let _ = turned;
        let yaw = yaw_of(scene.objects[0].transform).to_degrees();
        assert!((yaw.abs() - 45.0).abs() < 0.01, "turned to {yaw}");
        assert!(
            (yaw / editor.yaw_step()).fract().abs() < 1e-3,
            "{yaw} is not a multiple of the step"
        );

        update(
            &mut editor,
            &InputState::new(),
            &release(turned),
            &mut scene,
        );
        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert!(yaw_of(scene.objects[0].transform).abs() < 1e-4);
    }

    #[test]
    fn a_scale_handle_stretches_one_axis_and_the_centre_all_three() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        scene.objects[0].transform.translation = Vec3::ZERO;
        editor.select(Selected::Object(0), false);
        update(
            &mut editor,
            &keyed(KeyCode::Digit3),
            &UiInput::new(),
            &mut scene,
        );
        update_seen_from(&mut editor, &hover(centre()), &mut scene, &side_on());

        let gizmo = editor.gizmo(&scene).expect("a gizmo");
        let handle = gizmo.handle(gizmo::Axis::X).expect("an X handle");
        let grab = (
            (handle.from.0 + handle.to.0) * 0.5,
            (handle.from.1 + handle.to.1) * 0.5,
        );
        let by = (handle.to.0 - handle.from.0, handle.to.1 - handle.from.1);
        update_seen_from(&mut editor, &press(grab), &mut scene, &side_on());
        update_seen_from(
            &mut editor,
            &hold((grab.0 + by.0, grab.1 + by.1)),
            &mut scene,
            &side_on(),
        );
        let scale = scene.objects[0].transform.scale;
        assert!((scale.x - 2.0).abs() < 0.06, "stretched to {scale:?}");
        assert_eq!((scale.y, scale.z), (1.0, 1.0), "the other two moved");
        update_seen_from(&mut editor, &release(grab), &mut scene, &side_on());

        // The centre: up the screen grows all three.
        let origin = gizmo.origin;
        update_seen_from(&mut editor, &press(origin), &mut scene, &side_on());
        update_seen_from(
            &mut editor,
            &hold((origin.0, origin.1 - 150.0)),
            &mut scene,
            &side_on(),
        );
        let grown = scene.objects[0].transform.scale;
        assert!(
            grown.y > 1.5 && grown.z > 1.5 && grown.x > scale.x,
            "{grown:?}"
        );
    }

    #[test]
    fn a_number_typed_into_the_inspector_sets_it_and_one_undo_takes_it_back() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        editor.select(Selected::Object(0), false);
        let point = value_point(&editor, &scene, inspector::Property::PositionX);
        update(&mut editor, &InputState::new(), &press(point), &mut scene);
        assert!(
            editor.inspector_edit().is_some(),
            "the click did not open the field"
        );

        for key in [KeyCode::Minus, KeyCode::Digit1, KeyCode::Digit2] {
            update(&mut editor, &keyed(key), &hover(point), &mut scene);
        }
        let outcome = update(
            &mut editor,
            &keyed(KeyCode::Enter),
            &hover(point),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.objects[0].position().x, -12.0);
        assert!(editor.inspector_edit().is_none());

        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.objects[0].position().x, 200.0);
    }

    #[test]
    fn the_wheel_over_an_inspector_number_steps_it() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        editor.select(Selected::Object(0), false);
        let point = value_point(&editor, &scene, inspector::Property::Yaw);
        let outcome = update(&mut editor, &wheeled(2.0), &hover(point), &mut scene);
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        let yaw = yaw_of(scene.objects[0].transform).to_degrees();
        assert!((yaw - 2.0 * editor.yaw_step()).abs() < 1e-3, "yaw {yaw}");
    }

    #[test]
    fn f_frames_the_selection_from_where_the_camera_is_looking() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        editor.select(Selected::Object(0), false);
        update(&mut editor, &keyed(KeyCode::F), &UiInput::new(), &mut scene);
        let distance = (editor.eye() - scene.objects[0].position()).length();
        assert!(distance < 30.0, "still {distance} m away");
    }

    #[test]
    fn i_puts_a_light_where_the_cursor_is_and_every_verb_reaches_it() {
        let mut editor = editor();
        let mut scene = scene();
        let (input, ui) = keyed_at(KeyCode::I, centre());
        let outcome = update(&mut editor, &input, &ui, &mut scene);
        assert_eq!(
            outcome,
            EditorOutcome::SceneChanged,
            "a light is re-placed with the level"
        );
        assert_eq!(scene.lights.len(), 1);
        assert_eq!(editor.selection(), Some(Selected::Light(0)));
        assert!(
            scene.lights[0].position.y > 1.0,
            "a light on the floor lights nothing"
        );

        let range = scene.lights[0].range;
        update(&mut editor, &keyed(KeyCode::T), &UiInput::new(), &mut scene);
        assert!(scene.lights[0].range > range, "T reaches a light's range");
        update(
            &mut editor,
            &control_keyed(KeyCode::D),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.lights.len(), 2);
        update(
            &mut editor,
            &keyed(KeyCode::Delete),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.lights.len(), 1);
    }

    #[test]
    fn control_f_over_the_hierarchy_filters_it_by_name() {
        let mut editor = editor();
        let mut scene = three_rocks();
        scene.push(EnvironmentObject::decoration(PILLAR, Vec3::ZERO));
        let over = item_point(&editor, &scene, Selected::Object(0));
        update(
            &mut editor,
            &control_keyed(KeyCode::F),
            &hover(over),
            &mut scene,
        );
        assert!(editor.is_filtering_hierarchy());
        for key in [KeyCode::P, KeyCode::I, KeyCode::L, KeyCode::Enter] {
            update(&mut editor, &keyed(key), &hover(over), &mut scene);
        }
        assert!(!editor.is_filtering_hierarchy());
        assert_eq!(editor.hierarchy_items(&scene), vec![Selected::Object(3)]);
        // And the one row left is the one a click on it selects: under its group's heading.
        let row = item_point(&editor, &scene, Selected::Object(3));
        update(&mut editor, &InputState::new(), &press(row), &mut scene);
        assert_eq!(editor.selection(), Some(Selected::Object(3)));
    }

    // -----------------------------------------------------------------------------------
    // The editor's interface: toolbar, menus, panels, dialogs.
    // -----------------------------------------------------------------------------------

    fn middle(rect: engine_render_data::ui::UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// The toolbar control whose tooltip is `tip`.
    fn tool_rect(editor: &Editor, tip: &str) -> engine_render_data::ui::UiRect {
        panels::toolbar(editor, &editor.layout_for(VIEWPORT))
            .into_iter()
            .find(|item| item.tip == tip)
            .map(|item| item.rect)
            .expect("that control is on the toolbar")
    }

    /// A right click at `pointer`: the press in one frame, the release in the next.
    fn right_click(editor: &mut Editor, scene: &mut EnvironmentScene, pointer: (f32, f32)) {
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::MouseButtonDown {
            button: MouseButton::Right,
        });
        update(editor, &input, &hover(pointer), scene);
        input.begin_frame();
        input.apply(engine_core::input::InputEvent::MouseButtonUp {
            button: MouseButton::Right,
        });
        update(editor, &input, &hover(pointer), scene);
    }

    #[test]
    fn a_toolbar_button_picks_its_tool_and_the_select_tool_has_no_handles() {
        let mut editor = editor();
        let mut scene = three_rocks();
        editor.select(Selected::Object(0), false);
        let rotate = tool_rect(&editor, "Rotate about the vertical axis");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(rotate)),
            &mut scene,
        );
        assert_eq!(editor.tool(), commands::Tool::Turn);
        assert_eq!(scene.len(), 3, "a toolbar click reached the level");

        let select = tool_rect(&editor, "Select: click objects, no handles");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(select)),
            &mut scene,
        );
        assert_eq!(editor.tool(), commands::Tool::Select);
        assert!(editor.gizmo(&scene).is_none());
    }

    #[test]
    fn a_menu_opens_from_the_toolbar_and_its_item_runs_its_command() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let edit = tool_rect(&editor, "Undo, duplicate, delete, select");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(edit)),
            &mut scene,
        );
        let menu = editor.open_menu().cloned().expect("the Edit menu is open");
        assert_eq!(menu.kind, panels::MenuKind::Edit);

        let layout = editor.layout_for(VIEWPORT);
        let entries = panels::menu_entries(menu.kind, &editor, &scene);
        let (_, rects) = panels::menu_rects(&layout, menu.origin, &entries);
        let at = entries
            .iter()
            .position(|entry| entry.label == "Select All")
            .expect("Select All is in Edit");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(rects[at])),
            &mut scene,
        );
        assert!(
            editor.open_menu().is_none(),
            "choosing an item closes the menu"
        );
        assert_eq!(objects_of(&editor), vec![0, 1, 2]);
    }

    #[test]
    fn a_disabled_menu_item_does_nothing_and_escape_closes_the_menu() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let edit = tool_rect(&editor, "Undo, duplicate, delete, select");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(edit)),
            &mut scene,
        );
        let menu = editor.open_menu().cloned().expect("open");
        let layout = editor.layout_for(VIEWPORT);
        let entries = panels::menu_entries(menu.kind, &editor, &scene);
        let (_, rects) = panels::menu_rects(&layout, menu.origin, &entries);
        let delete = entries
            .iter()
            .position(|entry| entry.label == "Delete")
            .expect("Delete");
        assert!(!entries[delete].enabled, "nothing is selected to delete");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(rects[delete])),
            &mut scene,
        );
        assert!(
            editor.open_menu().is_some(),
            "a disabled item is not a click"
        );
        assert_eq!(scene.len(), 3);
        update(
            &mut editor,
            &keyed(KeyCode::Escape),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.open_menu().is_none());
    }

    #[test]
    fn a_right_click_on_an_object_selects_it_and_offers_what_can_be_done_to_it() {
        let mut editor = editor();
        let mut scene = three_rocks();
        right_click(&mut editor, &mut scene, centre());
        assert_eq!(editor.selection(), Some(Selected::Object(0)));
        assert_eq!(
            editor.open_menu().map(|menu| menu.kind),
            Some(panels::MenuKind::Object)
        );

        // And on empty ground, what can be done there.
        editor.menu = None;
        right_click(&mut editor, &mut scene, (centre().0, centre().1 + 300.0));
        assert_eq!(
            editor.open_menu().map(|menu| menu.kind),
            Some(panels::MenuKind::Viewport)
        );
    }

    #[test]
    fn a_right_drag_turns_the_camera_and_opens_nothing() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let mut input = InputState::new();
        input.apply(engine_core::input::InputEvent::MouseButtonDown {
            button: MouseButton::Right,
        });
        update(&mut editor, &input, &hover(centre()), &mut scene);
        input.begin_frame();
        input.apply(engine_core::input::InputEvent::MouseButtonUp {
            button: MouseButton::Right,
        });
        update(
            &mut editor,
            &input,
            &hover((centre().0 + 80.0, centre().1)),
            &mut scene,
        );
        assert!(editor.open_menu().is_none());
    }

    #[test]
    fn dragging_a_splitter_widens_the_browser_and_the_viewport_gives_way() {
        let mut editor = editor();
        let mut scene = scene();
        let before = editor.layout_for(VIEWPORT);
        let grab = middle(before.splitter(layout::Splitter::Left));
        update(&mut editor, &InputState::new(), &press(grab), &mut scene);
        update(
            &mut editor,
            &InputState::new(),
            &hold((grab.0 + 100.0, grab.1)),
            &mut scene,
        );
        update(
            &mut editor,
            &InputState::new(),
            &release((grab.0 + 100.0, grab.1)),
            &mut scene,
        );
        let after = editor.layout_for(VIEWPORT);
        assert!((after.left.width - before.left.width - 100.0).abs() < 1.0);
        assert_eq!(after.viewport.x, after.left.right());
        assert!(scene.is_empty(), "the drag reached the level");
    }

    #[test]
    fn copy_and_paste_put_a_copy_under_the_pointer_and_select_it() {
        let mut editor = editor();
        let mut scene = three_rocks();
        editor.select(Selected::Object(1), false);
        update(
            &mut editor,
            &control_keyed(KeyCode::C),
            &hover(centre()),
            &mut scene,
        );
        assert!(editor.can_paste());
        let outcome = update(
            &mut editor,
            &control_keyed(KeyCode::V),
            &hover(centre()),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 4);
        assert_eq!(editor.selection(), Some(Selected::Object(3)));
        let pasted = scene.objects[3].position();
        assert!(
            pasted.x.abs() < 0.6 && pasted.z.abs() < 0.6,
            "pasted at {pasted:?}, not under the pointer"
        );
    }

    #[test]
    fn shift_click_in_the_hierarchy_selects_everything_between() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let first = item_point(&editor, &scene, Selected::Object(0));
        update(&mut editor, &InputState::new(), &press(first), &mut scene);
        let last = item_point(&editor, &scene, Selected::Object(2));
        update(
            &mut editor,
            &shift_keyed(KeyCode::Unknown),
            &press(last),
            &mut scene,
        );
        let mut chosen = objects_of(&editor);
        chosen.sort_unstable();
        assert_eq!(chosen, vec![0, 1, 2]);
        assert_eq!(
            editor.selection(),
            Some(Selected::Object(2)),
            "the one clicked is the primary"
        );
    }

    #[test]
    fn a_group_folds_away_and_its_eye_hides_everything_in_it() {
        let mut editor = editor();
        let mut scene = three_rocks();
        let heading = hierarchy_point(&editor, 0);
        update(&mut editor, &InputState::new(), &press(heading), &mut scene);
        assert_eq!(
            editor.hierarchy_view(&scene).len(),
            1,
            "the group did not fold"
        );

        let layout = editor.layout_for(VIEWPORT);
        let (eye, _) = panels::row_toggles(&layout, layout.hierarchy_row(0));
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(middle(eye)),
            &mut scene,
        );
        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert!((0..3).all(|index| editor.is_hidden(index)));
    }

    #[test]
    fn an_inspector_section_folds_and_its_numbers_go_with_it() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        editor.select(Selected::Object(0), false);
        let layout = editor.layout_for(VIEWPORT);
        let section = panels::inspector(&editor, &scene, &layout)
            .into_iter()
            .find_map(|element| match element {
                panels::Element::Section {
                    rect,
                    section: panels::Section::Transform,
                    ..
                } => Some(rect),
                _ => None,
            })
            .expect("a Transform section");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(section)),
            &mut scene,
        );
        let shown = panels::inspector(&editor, &scene, &layout);
        assert!(!shown.iter().any(|element| matches!(
            element,
            panels::Element::Value {
                property: inspector::Property::PositionX,
                ..
            }
        )));
    }

    #[test]
    fn dragging_a_numbers_tag_scrubs_it_and_one_undo_takes_the_whole_drag_back() {
        let mut editor = editor();
        let mut scene = scene_of(1);
        editor.select(Selected::Object(0), false);
        let layout = editor.layout_for(VIEWPORT);
        let tag = panels::inspector(&editor, &scene, &layout)
            .into_iter()
            .find_map(|element| match element {
                panels::Element::Tag {
                    rect,
                    property: inspector::Property::PositionX,
                    ..
                } => Some(rect),
                _ => None,
            })
            .expect("an X tag");
        let from = middle(tag);
        update(&mut editor, &InputState::new(), &press(from), &mut scene);
        for step in 1..=4 {
            update(
                &mut editor,
                &InputState::new(),
                &hold((from.0 + step as f32 * 20.0, from.1)),
                &mut scene,
            );
        }
        update(
            &mut editor,
            &InputState::new(),
            &release((from.0 + 80.0, from.1)),
            &mut scene,
        );
        let x = scene.objects[0].position().x;
        assert!(x > 200.5, "the scrub moved it to {x}");
        update(
            &mut editor,
            &control_keyed(KeyCode::Z),
            &UiInput::new(),
            &mut scene,
        );
        assert_eq!(scene.objects[0].position().x, 200.0);
    }

    #[test]
    fn a_centre_pivot_stands_the_gizmo_in_the_middle_of_the_selection() {
        let mut editor = editor();
        let mut scene = three_rocks();
        update(
            &mut editor,
            &control_keyed(KeyCode::A),
            &UiInput::new(),
            &mut scene,
        );
        editor.execute(commands::Command::TogglePivot, &mut scene);
        let origin = editor.gizmo_origin(&scene).expect("a gizmo");
        assert!((origin.x - 4.0).abs() < 1e-4, "{origin:?}");
    }

    #[test]
    fn the_tile_view_opens_a_folder_and_the_breadcrumb_goes_back() {
        let mut editor = editor();
        let mut scene = scene();
        editor.execute(
            commands::Command::SetBrowserView(commands::BrowserView::Tiles),
            &mut scene,
        );
        let layout = editor.layout_for(VIEWPORT);
        // The first tile at the root is the built-in library.
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(layout.tile(0))),
            &mut scene,
        );
        assert_eq!(editor.tile_folder(), panels::TileFolder::Builtin);
        let crumbs = layout.crumbs();
        update(
            &mut editor,
            &InputState::new(),
            &press((crumbs.x + layout.px(10.0), crumbs.y + crumbs.height * 0.5)),
            &mut scene,
        );
        assert_eq!(editor.tile_folder(), panels::TileFolder::Root);
    }

    #[test]
    fn save_as_can_be_cancelled_with_its_button() {
        let mut editor = editor();
        let mut scene = scene();
        editor.execute(commands::Command::SaveAs, &mut scene);
        assert!(editor.save_as_text().is_some());
        let layout = editor.layout_for(VIEWPORT);
        let dialog = panels::dialog(&layout, panels::DialogKind::SaveAs);
        let (cancel, ..) = dialog
            .buttons
            .iter()
            .find(|(_, button, ..)| *button == panels::DialogButton::Cancel)
            .copied()
            .expect("a cancel button");
        update(
            &mut editor,
            &InputState::new(),
            &press(middle(cancel)),
            &mut scene,
        );
        assert!(editor.save_as_text().is_none());
    }

    #[test]
    fn f1_opens_the_shortcuts_and_escape_closes_them() {
        let mut editor = editor();
        let mut scene = scene();
        update(
            &mut editor,
            &keyed(KeyCode::F1),
            &UiInput::new(),
            &mut scene,
        );
        assert!(editor.shows_shortcuts());
        update(
            &mut editor,
            &keyed(KeyCode::Escape),
            &UiInput::new(),
            &mut scene,
        );
        assert!(!editor.shows_shortcuts());
    }

    #[test]
    fn resting_on_a_toolbar_button_shows_its_tooltip_after_a_moment() {
        let mut editor = editor();
        let mut scene = scene();
        let save = middle(tool_rect(&editor, "Save the level"));
        editor.update(
            &InputState::new(),
            &hover(save),
            &mut scene,
            &overhead(),
            VIEWPORT,
            0.1,
        );
        assert!(
            editor.tooltip().is_none(),
            "a tooltip that appears at once is in the way"
        );
        for _ in 0..6 {
            editor.update(
                &InputState::new(),
                &hover(save),
                &mut scene,
                &overhead(),
                VIEWPORT,
                0.1,
            );
        }
        assert_eq!(editor.tooltip(), Some(("Save the level", "Ctrl+S")));
        editor.update(
            &InputState::new(),
            &hover(centre()),
            &mut scene,
            &overhead(),
            VIEWPORT,
            0.1,
        );
        assert!(editor.tooltip().is_none());
    }
}
