//! The level editor: placing, moving and deleting environment objects inside the running game.
//!
//! ```text
//!  F6 ──► Editing ───────────────► EditorOutcome
//!   ▲        │  left click ground   place the browser's asset
//!   │        │  left click object   select it
//!   │        │  drag                move it, snapped to the grid
//!   │        │  R / SHIFT+R         yaw, snapped to the rotation step
//!   │        │  G / SHIFT+G         grid step / rotation step
//!   │        │  T / SHIFT+T         scale
//!   │        │  ARROWS / SHIFT      nudge by the grid, in X and Z / in Y
//!   │        │  L                   nudge along world axes or the object's own
//!   │        │  0                   reset rotation and scale
//!   │        │  X                   delete
//!   │        │  C                   duplicate
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
pub mod pick;
pub mod ui;

use std::path::{Path, PathBuf};

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Mat4, Vec3};
use engine_core::transform::Transform;
use engine_environment::terrain::{BrushMode, Terrain};
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene, scene_file};
use engine_presentation::ui::input::UiInput;
use engine_render_data::{Camera, RenderDataError, UiScale};

pub use camera::FlyCamera;

/// How many snapshots `Z` can walk back through.
const UNDO_DEPTH: usize = 64;

/// What `Z` puts back: everything an edit can change.
///
/// The placements *and* the ground. It used to be the placements alone, which was right while
/// they were the only thing an author could change; a brush stroke is an edit like any other
/// and has to be undoable the same way. A terrain is 66 KB at the shipping size and a
/// snapshot is only taken when something changes, so sixty-four of them is a few megabytes in
/// the editor and nothing at all in a shipping build, which has none of this.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    objects: Vec<EnvironmentObject>,
    terrain: Option<Terrain>,
}

impl Snapshot {
    /// The scene as it is now.
    fn of(scene: &EnvironmentScene) -> Self {
        Self {
            objects: scene.objects.clone(),
            terrain: scene.terrain.clone(),
        }
    }

    /// Puts this snapshot back into `scene`.
    fn restore(self, scene: &mut EnvironmentScene) {
        scene.objects = self.objects;
        scene.terrain = self.terrain;
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
    /// The author asked to go back to playing.
    Left,
}

/// A drag in progress: which object, and where the cursor caught it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    /// The object being dragged.
    index: usize,
    /// Where the object stood when the drag began.
    origin: Vec3,
    /// Where the cursor was on the ground when the drag began.
    grabbed_at: Vec3,
    /// Whether the object has actually moved yet, so a click that does not move anything
    /// does not push an undo snapshot.
    moved: bool,
}

/// The editor.
#[derive(Debug)]
pub struct Editor {
    active: bool,
    camera: FlyCamera,
    /// What is in `assets/`, and which of it the next click places.
    browser: browser::AssetBrowser,
    /// Which placements are selected, in the order they were added. The last is the primary
    /// one — what the inspector describes and what a verb reports on.
    selected: Vec<usize>,
    drag: Option<Drag>,
    /// Index into [`GRID_STEPS`].
    grid: usize,
    /// Index into [`YAW_STEPS`].
    yaw_step: usize,
    /// Whether the terrain brush has the pointer instead of placement and selection.
    sculpting: bool,
    /// What the brush does.
    brush: BrushMode,
    /// How wide the brush is, in metres.
    brush_radius: f32,
    /// How much one stroke moves the ground, in metres.
    brush_strength: f32,
    /// Whether a nudge runs along the object's own axes rather than the world's.
    local_space: bool,
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
            drag: None,
            grid: 0,
            yaw_step: 2,
            sculpting: false,
            brush: BrushMode::Raise,
            brush_radius: 6.0,
            brush_strength: 0.5,
            local_space: false,
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

    /// The primary selection: the placement the inspector describes.
    ///
    /// The *last* one added rather than the first, because the thing an author just clicked
    /// on is the thing they are looking at.
    #[must_use]
    pub fn selection(&self) -> Option<usize> {
        self.selected.last().copied()
    }

    /// Every selected placement, in the order they were added.
    #[must_use]
    pub fn selected(&self) -> &[usize] {
        &self.selected
    }

    /// Whether a placement is selected.
    #[must_use]
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    /// How far one press of `R` turns an object, in degrees.
    #[must_use]
    pub fn yaw_step(&self) -> f32 {
        YAW_STEPS[self.yaw_step % YAW_STEPS.len()]
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
        GRID_STEPS[self.grid % GRID_STEPS.len()]
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
        self.selected.clear();
        self.drag = None;
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
        self.drag = None;
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
                self.reveal_selection(viewport);
                return outcome;
            }
        }

        let mut changed = if browser_took_the_keys || self.browser.is_typing() {
            false
        } else {
            self.update_keys(input, scene)
        };
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

        // Whatever just happened, the selected object is worth seeing in the list.
        self.reveal_selection(viewport);

        if changed {
            EditorOutcome::SceneChanged
        } else {
            EditorOutcome::Idle
        }
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
            self.status = format!(
                "{} at {:.0} {:.0}",
                self.brush.as_str(),
                ground.x,
                ground.z
            );
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
            self.browser.scroll_by(if wheel > 0.0 { -step } else { step }, visible);
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
        // Scrolled by whole rows, and never past the end: a list that can be scrolled into
        // emptiness looks like a list that has lost its contents.
        let wheel = input.wheel_delta();
        if over_hierarchy && wheel != 0.0 {
            let last = scene.len().saturating_sub(visible);
            let step = wheel.abs().max(1.0) as usize;
            self.hierarchy_scroll = if wheel > 0.0 {
                self.hierarchy_scroll.saturating_sub(step)
            } else {
                (self.hierarchy_scroll + step).min(last)
            };
        }
        self.hierarchy_scroll = self.hierarchy_scroll.min(scene.len().saturating_sub(visible));

        if over_hierarchy && ui_input.pointer_pressed {
            for slot in 0..visible {
                if !ui::hierarchy_row_rect(self.scale, viewport, slot).contains(x, y) {
                    continue;
                }
                let index = self.hierarchy_scroll + slot;
                if let Some(object) = scene.objects.get(index) {
                    let control = input.is_key_down(KeyCode::LeftControl)
                        || input.is_key_down(KeyCode::RightControl);
                    self.select(index, control);
                    self.drag = None;
                    self.status = format!(
                        "selected {} ({index}){}",
                        object.asset.name(),
                        if self.selected.len() > 1 {
                            format!(" of {}", self.selected.len())
                        } else {
                            String::new()
                        }
                    );
                }
                break;
            }
        }
        true
    }

    /// Adds `index` to the selection, or replaces the selection with it.
    ///
    /// `add` is what CTRL does: it *toggles*, so control-clicking a selected object takes it
    /// out again, which is the one gesture every editor agrees on.
    fn select(&mut self, index: usize, add: bool) {
        if !add {
            self.selected.clear();
            self.selected.push(index);
            return;
        }
        if let Some(position) = self.selected.iter().position(|known| *known == index) {
            self.selected.remove(position);
        } else {
            self.selected.push(index);
        }
    }

    /// Drops selected indices that are past the end of the scene.
    ///
    /// Undo replaces the object list wholesale, so a selection made before it can point past
    /// the end afterwards.
    fn prune_selection(&mut self, scene: &EnvironmentScene) {
        self.selected.retain(|index| *index < scene.objects.len());
    }

    /// Keeps the selected object in view, so a keyboard verb's result is visible.
    fn reveal_selection(&mut self, viewport: (u32, u32)) {
        let Some(index) = self.selection() else {
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
            self.selected.clear();
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
        if input.was_key_pressed(KeyCode::B) {
            self.sculpting = !self.sculpting;
            self.selected.clear();
            self.drag = None;
            self.status = if self.sculpting {
                "sculpting: drag to shape the ground".to_owned()
            } else {
                "placing".to_owned()
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
            self.selected = (0..scene.objects.len()).collect();
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

        if input.was_key_pressed(KeyCode::X) {
            self.snapshot(scene);
            // Highest index first, so removing one does not move the next one under it.
            let mut doomed = self.selected.clone();
            doomed.sort_unstable();
            for index in doomed.into_iter().rev() {
                scene.objects.remove(index);
            }
            self.status = format!("deleted {}", self.selected.len());
            self.selected.clear();
            return true;
        }
        if input.was_key_pressed(KeyCode::C) {
            self.snapshot(scene);
            // Offset by one grid step, or a metre when the grid is off, so the copy is
            // visible instead of hiding exactly inside the original.
            let step = if self.grid() > 0.0 { self.grid() } else { 1.0 };
            let mut copies = Vec::with_capacity(self.selected.len());
            for index in &self.selected {
                let Some(mut copy) = scene.objects.get(*index).copied() else {
                    continue;
                };
                copy.transform.translation += Vec3::new(step, 0.0, step);
                copies.push(scene.objects.len());
                scene.objects.push(copy);
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
            for index in self.selected.clone() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                // Snapped to a multiple of the step rather than added to: an object turned by
                // hand and then stepped ends up aligned with everything else stepped by the
                // same amount, which is the whole point of a rotation snap.
                last = snap_angle(yaw_of(object.transform) + step, self.yaw_step().to_radians());
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
            for index in self.selected.clone() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                object.transform.scale *= factor;
                last = object.transform.scale.y;
            }
            self.status = format!("scale {last:.2}");
            return true;
        }
        if input.was_key_pressed(KeyCode::Digit0) {
            self.snapshot(scene);
            for index in self.selected.clone() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                // Where it stands is what an author placed; how it is turned and how big it is
                // are what they have been fiddling with. Reset means the second, not the first.
                object.transform.rotation = engine_core::math::Quat::IDENTITY;
                object.transform.scale = Vec3::ONE;
            }
            self.status = "transform reset".to_owned();
            return true;
        }
        if let Some(step) = self.nudge(input, shift) {
            self.snapshot(scene);
            for index in self.selected.clone() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                // In local space the step runs along the object's own facing, so nudging a
                // yawed wall moves it along the wall rather than along the world's X.
                let moved = if self.local_space {
                    let yaw = yaw_of(object.transform);
                    let (sin, cos) = yaw.sin_cos();
                    Vec3::new(
                        step.x * cos + step.z * sin,
                        step.y,
                        -step.x * sin + step.z * cos,
                    )
                } else {
                    step
                };
                object.transform.translation += moved;
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

    /// Selecting, placing and dragging. Returns whether the scene changed.
    fn update_pointer(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_panel: bool,
        add_to_selection: bool,
    ) -> bool {
        let ground = engine_presentation::ui::targeting::ground_under_cursor(
            ui_input.pointer,
            view_projection,
            viewport,
        );

        // A drag in progress owns the pointer until the button comes up.
        if let Some(drag) = self.drag {
            if !ui_input.pointer_down {
                self.drag = None;
                return false;
            }
            let Some(ground) = ground else {
                return false;
            };
            let Some(object) = scene.objects.get_mut(drag.index) else {
                self.drag = None;
                return false;
            };
            let moved = self.snap(drag.origin + (ground - drag.grabbed_at));
            if moved == object.transform.translation {
                return false;
            }
            if !drag.moved {
                // The snapshot is taken on the first movement, not on the press: a click
                // that selects and does not drag must not fill the undo stack.
                let before = Snapshot::of(scene);
                self.push_undo(before);
                self.drag = Some(Drag {
                    moved: true,
                    ..drag
                });
            }
            // Everything selected moves by the delta the dragged object moved by, so a group
            // keeps its shape instead of collapsing onto the cursor.
            let Some(dragged) = scene.objects.get(drag.index) else {
                return false;
            };
            let delta = moved - dragged.transform.translation;
            for index in self.selected.clone() {
                let Some(object) = scene.objects.get_mut(index) else {
                    continue;
                };
                object.transform.translation += delta;
            }
            if !self.is_selected(drag.index)
                && let Some(object) = scene.objects.get_mut(drag.index)
            {
                object.transform.translation = moved;
            }
            self.status = if self.selected.len() > 1 {
                format!("moved {}", self.selected.len())
            } else {
                "moved".to_owned()
            };
            return true;
        }

        if !ui_input.pointer_pressed || over_panel {
            return false;
        }

        let Some((origin, direction)) = engine_presentation::ui::targeting::cursor_ray(
            ui_input.pointer,
            view_projection,
            viewport,
        ) else {
            return false;
        };

        if let Some(index) = pick::object_under_ray(scene, origin, direction) {
            // CTRL adds to the selection instead of replacing it; clicking an object that is
            // already selected keeps the whole group, so a group can be dragged by any of its
            // members rather than only by the last one clicked.
            if !self.is_selected(index) {
                self.select(index, add_to_selection);
            }
            let object = scene.objects[index];
            self.drag = Some(Drag {
                index,
                origin: object.transform.translation,
                grabbed_at: ground.unwrap_or(object.transform.translation),
                moved: false,
            });
            self.status = if self.selected.len() > 1 {
                format!("selected {} objects", self.selected.len())
            } else {
                format!("selected {}", object.asset.name())
            };
            return false;
        }

        // Nothing under the cursor: place.
        let Some(ground) = ground else {
            return false;
        };
        self.snapshot(scene);
        let asset = self.asset();
        // What it blocks is decided once, here, and written into the level: a built-in has an
        // authored answer and a file gets a box around its geometry. The level carries it
        // either way, so a placement keeps what it was placed with even if the rule changes.
        let collision = self.browser.selected_collision();
        scene.push(EnvironmentObject {
            asset,
            transform: Transform::from_translation(self.snap(ground)),
            collision,
        });
        self.selected.clear();
        self.selected.push(scene.objects.len() - 1);
        self.status = format!("placed {}", asset.name());
        true
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
        self.drag = None;
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
    use engine_environment::object::default_collision;
    use engine_environment::asset::{PILLAR, ROCK_SMALL};
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

    #[test]
    fn a_click_on_a_hierarchy_row_selects_that_object_and_places_nothing() {
        let mut editor = editor();
        let mut scene = scene_of(4);
        let point = hierarchy_point(&editor, 2);

        let outcome = update(&mut editor, &InputState::new(), &press(point), &mut scene);

        assert_eq!(editor.selection(), Some(2));
        assert_eq!(scene.len(), 4, "the click through the panel placed an object");
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
        assert_eq!(editor.selection(), Some(4), "row 1 of a list scrolled by 3");
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
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let last = scene.len() - 1;
        assert_eq!(editor.selection(), Some(last));
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
    fn a_click_on_empty_ground_places_the_palettes_asset_once() {
        let mut editor = editor();
        let mut scene = scene();
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert_eq!(scene.len(), 1, "exactly one placement per click");
        assert_eq!(scene.objects[0].asset, editor.asset());
        let position = scene.objects[0].position();
        assert!(
            position.x.abs() < 0.5 && position.z.abs() < 0.5,
            "it went where the cursor was: {position:?}"
        );
        assert_eq!(editor.selection(), Some(0), "and it is selected");
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
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        assert!(
            scene.objects[0].collision.blocks(),
            "a pillar placed in the editor is a pillar you cannot walk through"
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
        assert_eq!(editor.selection(), Some(0));
    }

    #[test]
    fn dragging_a_selected_object_moves_that_object_and_nothing_else() {
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
        let (x, y) = centre();
        let outcome = update(
            &mut editor,
            &InputState::new(),
            &hold((x + 160.0, y)),
            &mut scene,
        );

        assert_eq!(outcome, EditorOutcome::SceneChanged);
        assert!(
            scene.objects[0].position().x > 1.0,
            "the pillar followed the cursor: {:?}",
            scene.objects[0].position()
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
        assert_eq!(editor.selection(), None, "the selection went with it");
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

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
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
            &press((folder.x + folder.width * 0.5, folder.y + folder.height * 0.5)),
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
        assert_eq!(editor.selection(), Some(0));

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
        assert_eq!(editor.selected(), [0]);

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
        assert_eq!(editor.selected(), [0, 2], "control did not add");
        assert_eq!(editor.selection(), Some(2), "the last one clicked is primary");

        update(&mut editor, &control, &press(second), &mut scene);
        assert_eq!(editor.selected(), [0], "control did not toggle it back off");
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
        assert_eq!(editor.selected(), [0, 1, 2]);

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
        assert_eq!(editor.selected(), [3, 4, 5]);

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
        assert_eq!(editor.selected(), [0, 2]);

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

        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
        let moved = (VIEWPORT.0 as f32 / 2.0 + 120.0, VIEWPORT.1 as f32 / 2.0);
        update(&mut editor, &InputState::new(), &hold(moved), &mut scene);

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
        assert!(moved.x.abs() < 1e-5, "local +X still moved world X: {moved:?}");
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
            assert_eq!(object.transform.translation, before[index], "object {index}");
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
        assert!((editor.brush_radius() - radius).abs() < 1e-4, "1 and 2 disagree");

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
        update(&mut editor, &keyed(KeyCode::Tab), &UiInput::new(), &mut scene);
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
        assert!(scene.terrain.as_ref().is_some_and(|terrain| {
            terrain.height_at(Vec3::ZERO) > 0.0
        }));

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
        update(
            &mut editor,
            &InputState::new(),
            &press(centre()),
            &mut scene,
        );
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
