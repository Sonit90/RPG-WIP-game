//! Everything the editor can be asked to do, as values.
//!
//! A key, a toolbar button, a menu item and a context-menu item all name a [`Command`], and
//! [`Editor::execute`] is the one place each command is carried out. That is what keeps
//! `CTRL+D`, *Edit → Duplicate* and right-click → *Duplicate* the same verb with the same undo
//! behind it, rather than three implementations that drift apart.

use engine_core::math::{Quat, Vec3};
use engine_environment::{EnvironmentObject, EnvironmentScene};

use crate::{
    Editor, GRID_STEPS, HIDDEN, LOCKED, MODULE_GRID, MODULE_YAW, SCALE_STEP, Selected, YAW_STEPS,
    inspector, pick, pose_of, position_of, set_pose, snap_angle,
};

/// What the left button does to what is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Click to select; no handles.
    Select,
    /// Drag a handle to move along one axis.
    #[default]
    Move,
    /// Drag the ring to turn about +Y.
    Turn,
    /// Drag a handle to stretch along one axis, the centre to grow on all three.
    Scale,
}

impl Tool {
    /// The gizmo this tool shows, if it shows one.
    #[must_use]
    pub fn gizmo(self) -> Option<crate::gizmo::GizmoMode> {
        match self {
            Self::Select => None,
            Self::Move => Some(crate::gizmo::GizmoMode::Translate),
            Self::Turn => Some(crate::gizmo::GizmoMode::Rotate),
            Self::Scale => Some(crate::gizmo::GizmoMode::Scale),
        }
    }

    /// What the toolbar and the status bar call it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Move => "Move",
            Self::Turn => "Rotate",
            Self::Scale => "Scale",
        }
    }
}

/// What a turn or a gizmo stands on when several things are selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pivot {
    /// The primary selection: the one the inspector describes.
    #[default]
    Active,
    /// The middle of everything selected.
    Center,
}

impl Pivot {
    /// What the toolbar calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Center => "Center",
        }
    }
}

/// How the content browser shows a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowserView {
    /// A tree of compact rows.
    #[default]
    List,
    /// The open folder's contents as tiles, with a breadcrumb above them.
    Tiles,
}

/// A request the editor can carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs, reason = "each variant is the action its name says")]
pub enum Command {
    Save,
    SaveAs,
    Undo,
    Redo,
    Play,
    Duplicate,
    Copy,
    Paste,
    Delete,
    SelectAll,
    SelectNone,
    SelectSameAsset,
    Frame,
    ResetTransform,
    DropToGround,
    Hide,
    ShowAll,
    ToggleLock,
    SetTool(Tool),
    TogglePlacing,
    ToggleSculpt,
    ToggleSpace,
    TogglePivot,
    ToggleSnap,
    SetGrid(usize),
    SetRotationStep(usize),
    ToggleModule,
    ToggleDebug,
    ToggleShortcuts,
    SetBrowserView(BrowserView),
    RevealAsset,
    ToggleHidden(usize),
    ToggleLocked(usize),
    MakePrimarySpawn,
    ToggleShadow,
    ResetProperty(inspector::Property),
    PutWater,
    PutSpawn,
    PutLight,
    ResetLayout,
}

impl Editor {
    /// Carries out `command`. Returns whether the level changed — what is drawn included, so
    /// hiding something counts.
    pub fn execute(&mut self, command: Command, scene: &mut EnvironmentScene) -> bool {
        self.prune_selection(scene);
        match command {
            Command::Save => {
                self.save(scene);
                false
            }
            Command::SaveAs => {
                let stem = self
                    .save_path
                    .file_stem()
                    .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
                self.save_as = Some(crate::field::TextField::new(
                    &stem,
                    crate::field::Charset::FileName,
                ));
                self.status = "Save as: type a name, Enter saves".to_owned();
                false
            }
            Command::Undo => self.undo(scene),
            Command::Redo => self.redo(scene),
            Command::Play => {
                self.leave_requested = true;
                false
            }
            Command::Duplicate => self.duplicate(scene),
            Command::Copy => {
                self.copy(scene);
                false
            }
            Command::Paste => self.paste(scene),
            Command::Delete => self.delete(scene),
            Command::SelectAll => {
                // Every object, and no water: a selection holds one kind. Hidden and locked
                // objects are left out, for the reason a click passes through them.
                self.selected = (0..scene.objects.len())
                    .map(Selected::Object)
                    .filter(|what| self.pickable(*what))
                    .collect();
                self.status = format!("Selected {}", self.selected.len());
                false
            }
            Command::SelectNone => {
                self.selected.clear();
                self.cancel_manipulation();
                false
            }
            Command::SelectSameAsset => {
                let Some(Selected::Object(index)) = self.selection() else {
                    return false;
                };
                let asset = scene.objects[index].asset;
                self.selected = scene
                    .objects
                    .iter()
                    .enumerate()
                    .filter(|(at, object)| {
                        object.asset == asset && self.pickable(Selected::Object(*at))
                    })
                    .map(|(at, _)| Selected::Object(at))
                    .collect();
                // The one that was clicked stays the primary.
                if let Some(at) = self
                    .selected
                    .iter()
                    .position(|what| *what == Selected::Object(index))
                {
                    let primary = self.selected.remove(at);
                    self.selected.push(primary);
                }
                self.status = format!("Selected {} of this asset", self.selected.len());
                false
            }
            Command::Frame => {
                self.frame_selection(scene);
                false
            }
            Command::ResetTransform => self.reset_transform(scene),
            Command::DropToGround => self.drop_to_ground(scene),
            Command::Hide => self.hide_selection(),
            Command::ShowAll => {
                let any = self.has_hidden();
                for flags in &mut self.flags {
                    *flags &= !HIDDEN;
                }
                self.status = "Everything shown".to_owned();
                any
            }
            Command::ToggleLock => {
                let objects = self.selected_objects();
                let lock = objects.iter().any(|index| !self.is_locked(*index));
                self.set_locked(&objects, lock);
                false
            }
            Command::SetTool(tool) => {
                self.tool = tool;
                self.placing = false;
                self.sculpting = false;
                self.cancel_manipulation();
                self.status = format!("{} tool", tool.label());
                false
            }
            Command::TogglePlacing => {
                self.placing = !self.placing;
                self.sculpting = false;
                self.cancel_manipulation();
                self.status = if self.placing {
                    format!(
                        "Placing {}: click to put one down",
                        crate::theme::display_name(self.browser.selected_name())
                    )
                } else {
                    "Select and move".to_owned()
                };
                false
            }
            Command::ToggleSculpt => {
                self.sculpting = !self.sculpting;
                // The two world modes are exclusive: the brush and the placing click both
                // want every press in the viewport.
                self.placing = false;
                self.selected.clear();
                self.cancel_manipulation();
                self.status = if self.sculpting {
                    "Sculpting: drag to shape the ground".to_owned()
                } else {
                    "Select and move".to_owned()
                };
                false
            }
            Command::ToggleSpace => {
                self.local_space = !self.local_space;
                self.status = if self.local_space {
                    "Local axes".to_owned()
                } else {
                    "World axes".to_owned()
                };
                false
            }
            Command::TogglePivot => {
                self.pivot = match self.pivot {
                    crate::commands::Pivot::Active => crate::commands::Pivot::Center,
                    crate::commands::Pivot::Center => crate::commands::Pivot::Active,
                };
                self.status = format!("Pivot: {}", self.pivot.label());
                false
            }
            Command::ToggleSnap => {
                self.snap = !self.snap;
                if self.snap && GRID_STEPS[self.grid % GRID_STEPS.len()] <= 0.0 {
                    self.grid = 0;
                }
                self.status = if self.snap {
                    format!("Snap on, {} m", self.grid())
                } else {
                    "Snap off".to_owned()
                };
                false
            }
            Command::SetGrid(index) => {
                self.grid = index % GRID_STEPS.len();
                self.snap = GRID_STEPS[self.grid] > 0.0;
                self.module_snap = false;
                self.module_snap_pinned = true;
                self.status = format!("Grid {} m", self.grid());
                false
            }
            Command::SetRotationStep(index) => {
                self.yaw_step = index % YAW_STEPS.len();
                self.status = format!("Rotate by {} degrees", self.yaw_step());
                false
            }
            Command::ToggleModule => {
                // Pressing it takes the decision off the editor for the rest of the session.
                self.module_snap = !self.module_snap;
                self.module_snap_pinned = true;
                self.status = if self.module_snap {
                    format!("Module snap: {MODULE_GRID} m grid, {MODULE_YAW} degree turns")
                } else {
                    "Module snap off".to_owned()
                };
                false
            }
            Command::ToggleDebug => {
                self.show_debug = !self.show_debug;
                false
            }
            Command::ToggleShortcuts => {
                self.modal = match self.modal {
                    Some(crate::Modal::Shortcuts) => None,
                    _ => Some(crate::Modal::Shortcuts),
                };
                false
            }
            Command::SetBrowserView(view) => {
                self.browser_view = view;
                false
            }
            Command::RevealAsset => {
                let Some(Selected::Object(index)) = self.selection() else {
                    return false;
                };
                let name = scene.objects[index].asset.name().to_owned();
                if self.browser.reveal_named(&name) {
                    self.status = format!(
                        "{} is chosen in the browser",
                        crate::theme::display_name(&name)
                    );
                } else {
                    self.status = "That asset is not in the browser".to_owned();
                }
                false
            }
            Command::ToggleHidden(index) => {
                let Some(flags) = self.flags.get_mut(index) else {
                    return false;
                };
                *flags ^= HIDDEN;
                if *flags & HIDDEN != 0 {
                    self.selected
                        .retain(|what| *what != Selected::Object(index));
                }
                true
            }
            Command::ToggleLocked(index) => {
                let Some(flags) = self.flags.get_mut(index) else {
                    return false;
                };
                *flags ^= LOCKED;
                false
            }
            Command::MakePrimarySpawn => {
                let Some(Selected::Spawn(index)) = self.selection() else {
                    return false;
                };
                if index == 0 || index >= scene.spawn_points.len() {
                    return false;
                }
                self.snapshot(scene);
                let spawn = scene.spawn_points.remove(index);
                scene.spawn_points.insert(0, spawn);
                self.selected = vec![Selected::Spawn(0)];
                self.water_touched = true;
                self.status = "This spawn point is where the player starts".to_owned();
                true
            }
            Command::ToggleShadow => {
                let lights: Vec<usize> = self
                    .selected
                    .iter()
                    .filter_map(|what| what.light())
                    .collect();
                if lights.is_empty() {
                    return false;
                }
                self.snapshot(scene);
                let cast = !lights.iter().all(|index| scene.lights[*index].casts_shadow);
                for index in lights {
                    scene.lights[index].casts_shadow = cast;
                }
                true
            }
            Command::ResetProperty(property) => {
                let Some(primary) = self.selection() else {
                    return false;
                };
                let value = match property {
                    inspector::Property::Yaw => 0.0,
                    inspector::Property::ScaleX
                    | inspector::Property::ScaleY
                    | inspector::Property::ScaleZ => 1.0,
                    _ => return false,
                };
                let before = crate::Snapshot::of(scene, &self.flags);
                let selection = self.selected.clone();
                let mut changed = false;
                let properties: &[inspector::Property] = match property {
                    inspector::Property::Yaw => &[inspector::Property::Yaw],
                    _ => &[
                        inspector::Property::ScaleX,
                        inspector::Property::ScaleY,
                        inspector::Property::ScaleZ,
                    ],
                };
                for property in properties {
                    changed |= inspector::set(scene, &selection, primary, *property, value);
                }
                if changed {
                    self.push_undo(before);
                    self.water_touched = !primary.is_scene_kind();
                }
                changed
            }
            Command::PutWater | Command::PutSpawn | Command::PutLight => {
                let pointer = self.menu_origin.unwrap_or(self.pointer);
                let ui = engine_presentation::ui::input::UiInput {
                    pointer,
                    ..engine_presentation::ui::input::UiInput::new()
                };
                let (view_projection, viewport) = (self.view_projection, self.viewport);
                match command {
                    Command::PutWater => {
                        self.pour_water(&ui, scene, &view_projection, viewport, false)
                    }
                    Command::PutSpawn => {
                        self.put_spawn(&ui, scene, &view_projection, viewport, false)
                    }
                    _ => self.put_light(&ui, scene, &view_projection, viewport, false),
                }
            }
            Command::ResetLayout => {
                self.panels = crate::layout::PanelSizes::default();
                false
            }
        }
    }

    /// Deletes everything selected. Returns whether anything was.
    pub(crate) fn delete(&mut self, scene: &mut EnvironmentScene) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        let count = self.selected.len();
        // Highest index first, so removing one does not move the next one under it. Each
        // kind is removed from its own list, and a selection holds one kind, so a delete
        // never reaches the other.
        let mut doomed = self.selected.clone();
        doomed.sort_unstable_by_key(|what| std::cmp::Reverse(crate::index_of(*what)));
        for what in doomed {
            match what {
                Selected::Object(index) => {
                    scene.objects.remove(index);
                    if index < self.flags.len() {
                        self.flags.remove(index);
                    }
                }
                Selected::Water(index) => {
                    scene.water.remove(index);
                }
                Selected::Spawn(index) => {
                    scene.spawn_points.remove(index);
                }
                Selected::Light(index) => {
                    scene.lights.remove(index);
                }
            }
        }
        self.status = format!("Deleted {count}");
        self.selected.clear();
        true
    }

    /// Copies everything selected, offset by one grid step, and selects the copies.
    pub(crate) fn duplicate(&mut self, scene: &mut EnvironmentScene) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        // Offset by one grid step, or a metre when the grid is off, so the copy is visible
        // instead of hiding exactly inside the original.
        let step = if self.grid() > 0.0 { self.grid() } else { 1.0 };
        let offset = Vec3::new(step, 0.0, step);
        let items = self.clip(scene);
        let copies = self.insert_clip(scene, &items, offset);
        self.status = format!("Duplicated {}", copies.len());
        // The copies become the selection, so dragging straight afterwards moves the new ones
        // rather than the originals.
        self.selected = copies;
        true
    }

    /// Puts what is selected on the editor's clipboard.
    pub(crate) fn copy(&mut self, scene: &EnvironmentScene) {
        self.clipboard = self.clip(scene);
        self.status = format!("Copied {}", self.clipboard.len());
    }

    /// Puts the clipboard into the level: under the pointer when it is over the viewport,
    /// keeping the copies' layout, and one grid step over from the originals otherwise.
    pub(crate) fn paste(&mut self, scene: &mut EnvironmentScene) -> bool {
        if self.clipboard.is_empty() {
            self.status = "Nothing to paste".to_owned();
            return false;
        }
        let items = self.clipboard.clone();
        let anchor = items.last().map(Clip::position).unwrap_or(Vec3::ZERO);
        let pointer = self.menu_origin.unwrap_or(self.pointer);
        let target = if self.layout().viewport.contains(pointer.0, pointer.1) {
            self.drop_point(pointer, scene, &self.view_projection, self.viewport)
        } else {
            None
        };
        let offset = match target {
            Some(target) => {
                let snapped = self.snap(target);
                Vec3::new(
                    snapped.x - anchor.x,
                    target.y - anchor.y,
                    snapped.z - anchor.z,
                )
            }
            None => {
                let step = if self.grid() > 0.0 { self.grid() } else { 1.0 };
                Vec3::new(step, 0.0, step)
            }
        };
        self.snapshot(scene);
        let pasted = self.insert_clip(scene, &items, offset);
        self.water_touched = pasted.iter().all(|what| !what.is_scene_kind());
        self.status = format!("Pasted {}", pasted.len());
        self.selected = pasted;
        true
    }

    /// What is selected, as things that can be put back into a level.
    fn clip(&self, scene: &EnvironmentScene) -> Vec<Clip> {
        self.selected
            .iter()
            .filter_map(|what| match *what {
                Selected::Object(index) => scene.objects.get(index).copied().map(Clip::Object),
                Selected::Water(index) => scene.water.get(index).copied().map(Clip::Water),
                Selected::Spawn(index) => scene.spawn_points.get(index).copied().map(Clip::Spawn),
                Selected::Light(index) => scene.lights.get(index).copied().map(Clip::Light),
            })
            .collect()
    }

    /// Adds `items` to the level moved by `offset`. Returns what they became.
    fn insert_clip(
        &mut self,
        scene: &mut EnvironmentScene,
        items: &[Clip],
        offset: Vec3,
    ) -> Vec<Selected> {
        let mut added = Vec::with_capacity(items.len());
        for item in items {
            added.push(match *item {
                Clip::Object(mut object) => {
                    object.transform.translation += offset;
                    scene.objects.push(object);
                    self.flags.push(0);
                    Selected::Object(scene.objects.len() - 1)
                }
                Clip::Water(mut body) => {
                    body.center += offset;
                    scene.water.push(body);
                    Selected::Water(scene.water.len() - 1)
                }
                Clip::Spawn(mut spawn) => {
                    spawn.position += offset;
                    scene.spawn_points.push(spawn);
                    Selected::Spawn(scene.spawn_points.len() - 1)
                }
                Clip::Light(mut light) => {
                    light.position += offset;
                    scene.lights.push(light);
                    Selected::Light(scene.lights.len() - 1)
                }
            });
        }
        added
    }

    /// Turns the selection by one rotation step, snapped. `back` turns the other way.
    pub(crate) fn turn_step(&mut self, scene: &mut EnvironmentScene, back: bool) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        let step = self.yaw_step().to_radians() * if back { -1.0 } else { 1.0 };
        let mut last = 0.0;
        for what in self.selected.clone() {
            // Snapped to a multiple of the step rather than added to: an object turned by
            // hand and then stepped ends up aligned with everything else stepped by the same
            // amount, which is the whole point of a rotation snap.
            let Some(mut pose) = pose_of(scene, what) else {
                continue;
            };
            last = snap_angle(pose.yaw + step, self.yaw_step().to_radians());
            pose.yaw = last;
            set_pose(scene, what, pose);
        }
        self.status = format!("Yaw {:.0} degrees", last.to_degrees());
        true
    }

    /// Grows the selection by one scale step, or shrinks it.
    pub(crate) fn scale_step(&mut self, scene: &mut EnvironmentScene, shrink: bool) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        let factor = if shrink { 1.0 / SCALE_STEP } else { SCALE_STEP };
        let mut last = 1.0;
        for what in self.selected.clone() {
            let Some(mut pose) = pose_of(scene, what) else {
                continue;
            };
            // A body of water has no scale: it has a size, and growing it is what `T` means
            // for one — its footprint, not its depth. A light's size is how far it reaches.
            pose.scale = match what {
                Selected::Water(_) => {
                    Vec3::new(pose.scale.x * factor, pose.scale.y, pose.scale.z * factor)
                }
                _ => pose.scale * factor,
            };
            last = pose.scale.x;
            set_pose(scene, what, pose);
        }
        self.status = match self.selection() {
            Some(Selected::Water(_)) => format!("Water {last:.1} m"),
            Some(Selected::Light(_)) => format!("Range {last:.1} m"),
            _ => format!("Scale {last:.2}"),
        };
        true
    }

    /// Puts rotation and scale back, leaving where things stand alone.
    pub(crate) fn reset_transform(&mut self, scene: &mut EnvironmentScene) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        for what in self.selected.clone() {
            let Some(mut pose) = pose_of(scene, what) else {
                continue;
            };
            // Where it stands is what an author placed; how it is turned and how big it is
            // are what they have been fiddling with — except for water, whose size is what
            // somebody dragged out on purpose.
            pose.yaw = 0.0;
            if let Selected::Object(_) = what {
                pose.scale = Vec3::ONE;
            }
            set_pose(scene, what, pose);
        }
        self.status = "Transform reset".to_owned();
        true
    }

    /// Moves the selection by `step`, along each thing's own facing in local space.
    pub(crate) fn nudge_by(&mut self, scene: &mut EnvironmentScene, step: Vec3) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        self.snapshot(scene);
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        let local = self.local_space;
        // In local space the step runs along the thing's own facing, so nudging a yawed wall
        // moves it along the wall rather than along the world's X.
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
        for what in self.selected.clone() {
            let Some(pose) = pose_of(scene, what) else {
                continue;
            };
            crate::move_by(scene, what, along(pose.yaw));
        }
        self.status = format!(
            "Moved {:.2} {:.2} {:.2} ({})",
            step.x,
            step.y,
            step.z,
            if self.local_space { "local" } else { "world" }
        );
        true
    }

    /// Down onto whatever is under each selected thing.
    pub(crate) fn drop_to_ground(&mut self, scene: &mut EnvironmentScene) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        // A crate lifted by the Y handle comes back to the floor, and a rock buried by a brush
        // stroke comes back up out of the hill.
        self.water_touched = self.selected.iter().all(|what| !what.is_scene_kind());
        let before = crate::Snapshot::of(scene, &self.flags);
        let moving = self.selected_objects();
        let mut moved = 0;
        for what in self.selected.clone() {
            let Some(position) = position_of(scene, what) else {
                continue;
            };
            let ground = scene.ground_at(position);
            let rest = match what {
                Selected::Object(_) => {
                    pick::rest_below(scene, position, ground, |index| moving.contains(&index))
                }
                Selected::Light(_) => continue,
                _ => ground,
            };
            if (rest - position.y).abs() > 1e-4 {
                crate::move_by(scene, what, Vec3::new(0.0, rest - position.y, 0.0));
                moved += 1;
            }
        }
        if moved > 0 {
            self.push_undo(before);
        }
        self.status = format!("Dropped {moved} onto what is under them");
        moved > 0
    }

    /// Hides the selected placements. Returns whether any were.
    pub(crate) fn hide_selection(&mut self) -> bool {
        let objects = self.selected_objects();
        for index in &objects {
            if let Some(flags) = self.flags.get_mut(*index) {
                *flags |= HIDDEN;
            }
        }
        self.selected.clear();
        self.water_touched = false;
        self.status = format!("Hid {} (Shift+H shows them)", objects.len());
        !objects.is_empty()
    }

    /// Locks or unlocks placements.
    pub(crate) fn set_locked(&mut self, objects: &[usize], lock: bool) {
        for index in objects {
            if let Some(flags) = self.flags.get_mut(*index) {
                if lock {
                    *flags |= LOCKED;
                } else {
                    *flags &= !LOCKED;
                }
            }
        }
        self.status = if lock {
            format!(
                "Locked {}: clicks in the viewport pass through",
                objects.len()
            )
        } else {
            format!("Unlocked {}", objects.len())
        };
    }

    /// Where the gizmo and a turn stand, given the pivot setting.
    pub(crate) fn pivot_point(&self, scene: &EnvironmentScene) -> Option<Vec3> {
        let primary = position_of(scene, self.selection()?)?;
        if self.pivot == Pivot::Active || self.selected.len() < 2 {
            return Some(primary);
        }
        let mut sum = Vec3::ZERO;
        let mut count = 0.0;
        for what in &self.selected {
            if let Some(at) = position_of(scene, *what) {
                sum += at;
                count += 1.0;
            }
        }
        Some(if count > 0.0 {
            sum * (1.0 / count)
        } else {
            primary
        })
    }

    /// The rotation the scale gizmo's handles run along: the primary's own.
    pub(crate) fn primary_rotation(&self, scene: &EnvironmentScene) -> Quat {
        self.selection()
            .and_then(|primary| pose_of(scene, primary))
            .map_or(Quat::IDENTITY, |pose| {
                Quat::from_yaw_pitch_roll(pose.yaw, 0.0, 0.0)
            })
    }
}

/// One thing on the editor's clipboard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Clip {
    /// A placement.
    Object(EnvironmentObject),
    /// A body of water.
    Water(engine_environment::water::WaterBody),
    /// A spawn point.
    Spawn(engine_environment::spawn::SpawnPoint),
    /// A light.
    Light(engine_environment::lighting::SceneLight),
}

impl Clip {
    /// Where it stood when it was copied.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        match self {
            Self::Object(object) => object.transform.translation,
            Self::Water(body) => body.center,
            Self::Spawn(spawn) => spawn.position,
            Self::Light(light) => light.position,
        }
    }
}
