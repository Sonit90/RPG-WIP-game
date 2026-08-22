//! The level editor: placing, moving and deleting environment objects inside the running game.
//!
//! ```text
//!  F6 ──► Editing ───────────────► EditorOutcome
//!   ▲        │  left click ground   place the palette's asset
//!   │        │  left click object   select it
//!   │        │  drag                move it, snapped to the grid
//!   │        │  R / SHIFT+R         yaw
//!   │        │  T / SHIFT+T         scale
//!   │        │  X                   delete
//!   │        │  C                   duplicate
//!   │        │  Z                   undo
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
//! That is what makes the whole thing testable. Every verb below has a test that clicks,
//! drags or presses a key and asserts what the scene became, with no window open.
//!
//! # It is not in a player's build
//!
//! `engine-app` names this crate as an optional dependency behind its `editor` feature. A
//! shipping build compiles none of it — no palette, no fly camera, no scene writer.
//!
//! # Why the whole scene is the undo unit
//!
//! `Z` restores the previous `Vec<EnvironmentObject>` wholesale rather than inverting the last
//! verb. A scene is a few hundred 44-byte records, so a snapshot is a few kilobytes and a
//! memcpy; inverting verbs means a second implementation of every verb, written backwards,
//! which is where undo bugs live.

#![deny(missing_docs)]

pub mod camera;
pub mod pick;
pub mod ui;

use std::path::{Path, PathBuf};

use engine_core::input::{InputState, KeyCode, MouseButton};
use engine_core::math::{Mat4, Vec3};
use engine_core::transform::Transform;
use engine_environment::asset::LIBRARY;
use engine_environment::object::default_collision;
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene, scene_file};
use engine_presentation::ui::input::UiInput;
use engine_presentation::ui::screen::UiScreen;
use engine_presentation::ui::widget::{Button, WidgetEvent};
use engine_render_data::{Camera, RenderDataError, UiScale};

pub use camera::FlyCamera;

/// How many snapshots `Z` can walk back through.
const UNDO_DEPTH: usize = 64;

/// The grid sizes `G` cycles through, in metres. Zero is off.
const GRID_STEPS: [f32; 5] = [1.0, 2.0, 4.0, 0.25, 0.0];

/// How far one press of `R` turns an object: an eighth of a turn.
const YAW_STEP: f32 = std::f32::consts::FRAC_PI_4 / 2.0;

/// How much one press of `T` changes an object's size.
const SCALE_STEP: f32 = 1.1;

/// What the application should do about the frame the editor just read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorOutcome {
    /// Nothing that the application has to answer for.
    Idle,
    /// The scene changed: re-place it.
    SceneChanged,
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
    palette: UiScreen<usize>,
    /// Which library asset the next click places.
    asset: usize,
    /// Which placement is selected, if any.
    selection: Option<usize>,
    drag: Option<Drag>,
    /// Index into [`GRID_STEPS`].
    grid: usize,
    /// Snapshots for `Z`, oldest first.
    undo: Vec<Vec<EnvironmentObject>>,
    /// Where `CTRL+S` writes.
    save_path: PathBuf,
    /// The last thing that happened, printed under the palette.
    status: String,
    scale: UiScale,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// A closed editor with its palette built.
    ///
    /// Built once, like every other screen in this project: entering the editor allocates
    /// nothing, because a palette is a list of rectangles.
    #[must_use]
    pub fn new() -> Self {
        let mut palette = UiScreen::new();
        for (index, asset) in LIBRARY.into_iter().enumerate() {
            palette.push(index, Button::new(asset.name()));
        }
        Self {
            active: false,
            camera: FlyCamera::default(),
            palette,
            asset: 0,
            selection: None,
            drag: None,
            grid: 0,
            undo: Vec::new(),
            save_path: PathBuf::from("assets/scenes/untitled.json"),
            status: String::new(),
            scale: UiScale::default(),
        }
    }

    /// Whether the editor has the input and the camera.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The palette, for drawing.
    #[must_use]
    pub fn palette(&self) -> &UiScreen<usize> {
        &self.palette
    }

    /// Which asset the next click places.
    #[must_use]
    pub fn asset(&self) -> EnvironmentAsset {
        LIBRARY[self.asset.min(LIBRARY.len() - 1)]
    }

    /// Which placement is selected.
    #[must_use]
    pub fn selection(&self) -> Option<usize> {
        self.selection
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

    /// The interface scale the palette is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.scale
    }

    /// Sets the interface scale and lays the palette out again.
    pub fn set_scale(&mut self, scale: UiScale, viewport: (u32, u32)) {
        self.scale = scale;
        ui::layout_palette(&mut self.palette, scale, viewport);
    }

    /// Enters the editor, looking at `focus` and saving to a file named after the scene.
    pub fn enter(&mut self, focus: Vec3, scene_name: &str, viewport: (u32, u32)) {
        self.active = true;
        self.camera = FlyCamera::overlooking(focus);
        self.selection = None;
        self.drag = None;
        self.undo.clear();
        self.palette.reset();
        self.save_path = PathBuf::from(format!("assets/scenes/{scene_name}.json"));
        self.status = format!("editing '{scene_name}'");
        ui::layout_palette(&mut self.palette, self.scale, viewport);
    }

    /// Leaves the editor. The scene keeps whatever was done to it.
    pub fn leave(&mut self) {
        self.active = false;
        self.drag = None;
        self.palette.reset();
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

        self.camera.update(input, seconds);
        ui::layout_palette(&mut self.palette, self.scale, viewport);

        // The palette first: a click on a button is a choice of asset, never a placement in
        // the world behind it.
        let mut chose_asset = false;
        for (index, event) in self.palette.update(ui_input) {
            if *event == WidgetEvent::Activated {
                self.asset = *index;
                chose_asset = true;
            }
        }
        if chose_asset {
            self.status = format!("placing {}", self.asset().name());
            return EditorOutcome::Idle;
        }
        let over_palette = self.palette.hovered().is_some();

        let mut changed = self.update_keys(input, scene);
        if self.update_pointer(ui_input, scene, view_projection, viewport, over_palette) {
            changed = true;
        }

        if changed {
            EditorOutcome::SceneChanged
        } else {
            EditorOutcome::Idle
        }
    }

    /// The keyboard verbs. Returns whether the scene changed.
    fn update_keys(&mut self, input: &InputState, scene: &mut EnvironmentScene) -> bool {
        let control =
            input.is_key_down(KeyCode::LeftControl) || input.is_key_down(KeyCode::RightControl);
        let shift = input.is_key_down(KeyCode::LeftShift) || input.is_key_down(KeyCode::RightShift);

        if input.was_key_pressed(KeyCode::Escape) {
            self.selection = None;
            return false;
        }
        if input.was_key_pressed(KeyCode::G) {
            self.grid = (self.grid + 1) % GRID_STEPS.len();
            let step = self.grid();
            self.status = if step > 0.0 {
                format!("grid {step} m")
            } else {
                "grid off".to_owned()
            };
            return false;
        }
        if input.was_key_pressed(KeyCode::Tab) {
            self.asset = (self.asset + 1) % LIBRARY.len();
            self.status = format!("placing {}", self.asset().name());
            return false;
        }
        if control && input.was_key_pressed(KeyCode::S) {
            self.save(scene);
            return false;
        }
        if input.was_key_pressed(KeyCode::Z) {
            return self.undo(scene);
        }

        // Everything below needs something selected.
        let Some(index) = self.selection else {
            return false;
        };
        if index >= scene.objects.len() {
            self.selection = None;
            return false;
        }

        if input.was_key_pressed(KeyCode::X) {
            self.snapshot(scene);
            scene.objects.remove(index);
            self.selection = None;
            self.status = "deleted".to_owned();
            return true;
        }
        if input.was_key_pressed(KeyCode::C) {
            self.snapshot(scene);
            // Offset by one grid step, or a metre when the grid is off, so the copy is
            // visible instead of hiding exactly inside the original.
            let step = if self.grid() > 0.0 { self.grid() } else { 1.0 };
            let mut copy = scene.objects[index];
            copy.transform.translation += Vec3::new(step, 0.0, step);
            scene.objects.push(copy);
            self.selection = Some(scene.objects.len() - 1);
            self.status = "duplicated".to_owned();
            return true;
        }
        if input.was_key_pressed(KeyCode::R) {
            self.snapshot(scene);
            let step = if shift { -YAW_STEP } else { YAW_STEP };
            let object = &mut scene.objects[index];
            let yaw = yaw_of(object.transform) + step;
            object.transform.rotation = engine_core::math::Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0);
            self.status = format!("yaw {:.0} deg", yaw.to_degrees());
            return true;
        }
        if input.was_key_pressed(KeyCode::T) {
            self.snapshot(scene);
            let factor = if shift { 1.0 / SCALE_STEP } else { SCALE_STEP };
            let object = &mut scene.objects[index];
            object.transform.scale *= factor;
            self.status = format!("scale {:.2}", object.transform.scale.y);
            return true;
        }
        false
    }

    /// Selecting, placing and dragging. Returns whether the scene changed.
    fn update_pointer(
        &mut self,
        ui_input: &UiInput,
        scene: &mut EnvironmentScene,
        view_projection: &Mat4,
        viewport: (u32, u32),
        over_palette: bool,
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
                let before = scene.objects.clone();
                self.push_undo(before);
                self.drag = Some(Drag {
                    moved: true,
                    ..drag
                });
            }
            let Some(object) = scene.objects.get_mut(drag.index) else {
                return false;
            };
            object.transform.translation = moved;
            self.status = "moved".to_owned();
            return true;
        }

        if !ui_input.pointer_pressed || over_palette {
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
            self.selection = Some(index);
            let object = scene.objects[index];
            self.drag = Some(Drag {
                index,
                origin: object.transform.translation,
                grabbed_at: ground.unwrap_or(object.transform.translation),
                moved: false,
            });
            self.status = format!("selected {}", object.asset.name());
            return false;
        }

        // Nothing under the cursor: place.
        let Some(ground) = ground else {
            return false;
        };
        self.snapshot(scene);
        let asset = self.asset();
        scene.push(EnvironmentObject {
            asset,
            transform: Transform::from_translation(self.snap(ground)),
            collision: default_collision(asset),
        });
        self.selection = Some(scene.objects.len() - 1);
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
        let objects = scene.objects.clone();
        self.push_undo(objects);
    }

    /// Pushes a snapshot, dropping the oldest once the stack is full.
    fn push_undo(&mut self, objects: Vec<EnvironmentObject>) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(objects);
    }

    /// Puts the last snapshot back. Returns whether anything changed.
    fn undo(&mut self, scene: &mut EnvironmentScene) -> bool {
        let Some(objects) = self.undo.pop() else {
            self.status = "nothing to undo".to_owned();
            return false;
        };
        scene.objects = objects;
        self.selection = None;
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

/// How far a placement is turned about +Y.
fn yaw_of(transform: Transform) -> f32 {
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
        // The palette starts on ground_plain, which blocks nothing; move it to a pillar.
        while editor.asset() != PILLAR {
            update(
                &mut editor,
                &keyed(KeyCode::Tab),
                &UiInput::new(),
                &mut scene,
            );
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
    fn a_click_on_the_palette_chooses_an_asset_and_places_nothing() {
        let mut editor = editor();
        let mut scene = scene();
        editor.set_scale(UiScale::default(), VIEWPORT);
        let Some(widget) = editor.palette().widget(3) else {
            panic!("the palette has sixteen entries");
        };
        let rect = widget.rect();
        let pointer = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);

        update(&mut editor, &InputState::new(), &press(pointer), &mut scene);
        let release = UiInput {
            pointer,
            pointer_released: true,
            ..UiInput::new()
        };
        update(&mut editor, &InputState::new(), &release, &mut scene);

        assert_eq!(editor.asset(), LIBRARY[3], "the palette chose the asset");
        assert!(scene.is_empty(), "and put nothing in the world");
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
