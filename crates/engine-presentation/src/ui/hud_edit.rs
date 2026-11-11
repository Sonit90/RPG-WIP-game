//! Edit HUD: the interface made draggable, with an inspector for everything else.
//!
//! ```text
//! Playing --EDIT HUD--> Editing --DONE/ESC--> Playing
//!                          |
//!                          +-- drag an element       -> a new position (snaps, if on)
//!                          +-- click an element      -> select it; the inspector shows it
//!                          +-- inspector
//!                                POSITION   anchor grid
//!                                SIZE       scale, width, height
//!                                LAYOUT     orientation, directions, spacing
//!                                VISIBILITY shown / hidden
//!                                Reset element · Done
//!                          +-- nothing selected: every element by name (the way back to
//!                              one that is hidden), Nameplates, Enemy Debuffs, grid and snapping, Reset all
//! ```
//!
//! ## It is not a panel over the game
//!
//! The HUD itself is what is being edited: the same action bars and frames in the same places,
//! drawn as they will look, with a light outline and a name chip each. The only surface the
//! editor adds is the inspector, a translucent card docked to whichever side of the screen
//! the selected element is not on. The world stays visible and keeps running behind it,
//! exactly as it does behind the pause menu.
//!
//! ## Every control is generic
//!
//! What the inspector offers for an element is what [`HudElementId::controls`] says the
//! element has: a direction control is one control whether it turns the buff bar or the raid
//! frames round. The controls are drawn from [`super::kit`], the same kit the tooltip and every
//! other screen use.
//!
//! ## Nothing it does is gameplay
//!
//! The editor never produces a [`PlayerCommand`](engine_game::command::PlayerCommand), and
//! the application does not run the bar's own click handling while it is active — so a click
//! that moves a bar cannot also cast what is in the slot under the cursor.
//!
//! ## Nothing can be lost
//!
//! Dragging writes through [`HudLayout::move_to`], which clamps, and the clamp runs again on
//! every resolve. An element cannot be pushed off the edge, and one hidden or tucked under
//! another is always one click away in the inspector's element list.

use std::f32::consts::PI;

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};
use engine_settings::hud::{HudAnchor, HudOrientation};
use engine_settings::{NameplateSetting, NameplateSettings};

use super::flow::FlowDirection;
use super::hud_layout::{HudElementId, HudLayout};
use super::input::UiInput;
use super::kit::{self, ButtonVariant, Interaction, Kit, Segment, tokens};
use super::map_style::{color, type_scale, write_title_case};

/// Layout constants for the inspector, in logical pixels.
mod layout {
    /// The inspector's width.
    pub const PANEL_WIDTH: f32 = 320.0;
    /// Space inside the inspector.
    pub const PAD: f32 = 16.0;
    /// Space between the inspector and the screen's edge.
    pub const MARGIN: f32 = 16.0;
    /// One control row's height.
    pub const ROW: f32 = 30.0;
    /// Between rows.
    pub const GAP: f32 = 8.0;
    /// The header's height.
    pub const HEADER: f32 = 52.0;
    /// The anchor grid's edge.
    pub const ANCHOR_GRID: f32 = 96.0;
    /// One element in the inspector's list.
    pub const LIST_ROW: f32 = 28.0;
    /// How close an edge must come to a guide to snap to it.
    pub const SNAP: f32 = 8.0;
    /// The grid's pitch.
    pub const GRID: f32 = 32.0;
}

/// What the inspector is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// One HUD element.
    Element(HudElementId),
    /// The enemy nameplates in the world: not a rectangle on the screen, but arranged here.
    Nameplates,
    /// The debuff rows over every enemy's nameplate: a frame of its own, about the real
    /// enemies in the fight and not about the target.
    EnemyDebuffs,
}

/// One control of the inspector, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Leaves edit mode.
    Done,
    /// Puts the whole layout back to the default arrangement.
    ResetAll,
    /// Puts the selected element back to its default.
    ResetElement,
    /// Back to the element list.
    Back,
    /// Shows or hides the selected element.
    Visible,
    /// Hangs the selected element from anchor `n` (row-major, 0 is top-left).
    Anchor(u8),
    /// One step of scale.
    Scale(i8),
    /// One step of width.
    Width(i8),
    /// One step of height.
    Height(i8),
    /// One step of spacing.
    Spacing(i8),
    /// Turns a bar to run this way.
    Orientation(HudOrientation),
    /// Items run right to left (`true`) or left to right.
    FlowX(bool),
    /// Items run bottom to top (`true`) or top to bottom.
    FlowY(bool),
    /// The alignment grid on or off.
    Grid,
    /// Snapping on or off.
    Snap,
    /// Shows this in the inspector.
    Select(EditTarget),
    /// One step of the nameplates' size.
    PlateSize(i8),
    /// One step of the enemy debuff icons' size.
    PlateDebuffs(i8),
    /// One step of the enemy buff icons' size.
    PlateBuffs(i8),
}

/// What the application should do about a frame of editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// Nothing changed.
    Idle,
    /// The layout changed and is worth saving.
    Changed,
    /// The nameplate settings changed: [`HudEditor::nameplates`] holds them.
    NameplatesChanged,
    /// Edit mode ended. Save.
    Closed,
}

/// A drag in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    /// Which element is being moved.
    element: HudElementId,
    /// Where inside the element the pointer went down, in pixels: kept so the element does
    /// not jump to put its corner under the cursor.
    grab: (f32, f32),
    /// Whether the layout has actually changed since the press.
    moved: bool,
}

/// A section heading drawn inside the inspector.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Heading {
    top: f32,
    title: &'static str,
}

/// Edit mode: what is selected, what is being dragged, and the inspector.
#[derive(Debug)]
pub struct HudEditor {
    active: bool,
    selected: Option<EditTarget>,
    drag: Option<Drag>,
    scale: UiScale,
    viewport: (u32, u32),
    /// The inspector's rectangle this frame.
    panel: UiRect,
    /// Every control of the inspector and where it is. Rebuilt in place.
    controls: Vec<(Control, UiRect)>,
    /// The inspector's section headings. Rebuilt in place.
    headings: Vec<Heading>,
    /// The guides a drag snapped to this frame: a vertical line's x, a horizontal line's y.
    guides: (Option<f32>, Option<f32>),
    /// Where the pointer is and whether it is down, for hover and pressed states.
    pointer: (f32, f32),
    pointer_down: bool,
    /// The nameplate settings the Nameplates section edits.
    nameplates: NameplateSettings,
    /// Where the debuff rows over the enemies on screen are this frame: what the Enemy
    /// Debuffs frame outlines, and where a click selects it. Rebuilt in place.
    enemy_debuffs: Vec<UiRect>,
}

impl Default for HudEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl HudEditor {
    /// A closed editor.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: false,
            selected: None,
            drag: None,
            scale: UiScale::default(),
            viewport: (0, 0),
            panel: UiRect::new(0.0, 0.0, 0.0, 0.0),
            controls: Vec::with_capacity(64),
            headings: Vec::with_capacity(8),
            guides: (None, None),
            pointer: (0.0, 0.0),
            pointer_down: false,
            nameplates: NameplateSettings::default(),
            enemy_debuffs: Vec::with_capacity(32),
        }
    }

    /// Whether the game is in edit mode.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Which element the player has selected, if any.
    #[must_use]
    pub fn selected(&self) -> Option<HudElementId> {
        match self.selected {
            Some(EditTarget::Element(id)) => Some(id),
            _ => None,
        }
    }

    /// What the inspector is showing.
    #[must_use]
    pub fn target(&self) -> Option<EditTarget> {
        self.selected
    }

    /// Whether an element is being dragged right now.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The interface size the inspector is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.scale
    }

    /// The inspector's rectangle.
    #[must_use]
    pub fn panel(&self) -> UiRect {
        self.panel
    }

    /// Where `control` is this frame, if the inspector is showing it.
    #[must_use]
    pub fn control_rect(&self, control: Control) -> Option<UiRect> {
        self.controls
            .iter()
            .find(|(candidate, _)| *candidate == control)
            .map(|(_, rect)| *rect)
    }

    /// The nameplate settings as the Nameplates section has left them.
    #[must_use]
    pub fn nameplates(&self) -> NameplateSettings {
        self.nameplates
    }

    /// Hands the editor the nameplate settings in force, for its Nameplates section.
    pub fn set_nameplates(&mut self, settings: NameplateSettings) {
        self.nameplates = settings;
    }

    /// Tells the editor where the debuff rows over the enemies on screen are this frame.
    pub fn set_enemy_debuff_rows(&mut self, rows: impl IntoIterator<Item = UiRect>) {
        self.enemy_debuffs.clear();
        self.enemy_debuffs.extend(rows);
    }

    /// Shows `target` in the inspector — what clicking it does. For a development flag and
    /// tests; the player selects by clicking.
    pub fn select(&mut self, target: Option<EditTarget>) {
        self.selected = target;
    }

    /// The target a written name means: `player`, `target`, `tot`, `party`, `raid`, `buffs`,
    /// `cast_bar`, `tooltip`, `bar1`..`bar8`, `nameplates`.
    #[must_use]
    pub fn target_named(name: &str) -> Option<EditTarget> {
        let element = match name {
            "nameplates" => return Some(EditTarget::Nameplates),
            "enemy_debuffs" => return Some(EditTarget::EnemyDebuffs),
            "player" => HudElementId::Player,
            "target" => HudElementId::Target,
            "tot" | "target_of_target" => HudElementId::TargetOfTarget,
            "party" => HudElementId::Party,
            "raid" | "group" => HudElementId::Raid,
            "buffs" => HudElementId::Buffs,
            "debuffs" => HudElementId::Debuffs,
            "target_buffs" => HudElementId::TargetBuffs,
            "target_debuffs" => HudElementId::TargetDebuffs,
            "resource" => HudElementId::Resource,
            "energy" => HudElementId::Energy,
            "cast_bar" => HudElementId::CastBar,
            "tooltip" => HudElementId::Tooltip,
            bar => {
                let index: usize = bar.strip_prefix("bar")?.parse().ok()?;
                if !(1..=engine_settings::hud::ACTION_BARS).contains(&index) {
                    return None;
                }
                HudElementId::ActionBar(index - 1)
            }
        };
        Some(EditTarget::Element(element))
    }

    /// Enters edit mode.
    pub fn open(&mut self) {
        self.active = true;
        self.selected = None;
        self.drag = None;
        self.guides = (None, None);
    }

    /// Leaves edit mode, ending any drag in progress.
    ///
    /// A drag that was in progress keeps whatever it had already written: the layout was
    /// being updated as the pointer moved, so there is nothing to roll back.
    pub fn close(&mut self) {
        self.active = false;
        self.selected = None;
        self.drag = None;
        self.guides = (None, None);
    }

    /// Lays the inspector out for a viewport, an interface size and the layout as it is.
    pub fn layout(&mut self, scale: UiScale, viewport: (u32, u32)) {
        self.scale = scale;
        self.viewport = viewport;
    }

    /// Applies one frame of input to `layout`.
    ///
    /// The inspector is read first: it is drawn on top of the HUD, so a click on it is a
    /// control and never the start of a drag on whatever it covers.
    pub fn update(
        &mut self,
        input: &UiInput,
        layout: &mut HudLayout,
        scale: UiScale,
        viewport: (u32, u32),
    ) -> EditOutcome {
        if !self.active {
            return EditOutcome::Idle;
        }
        self.layout(scale, viewport);
        self.pointer = input.pointer;
        self.pointer_down = input.pointer_down;
        self.layout_panel(layout);

        if input.cancel {
            // Escape leaves edit mode, exactly as DONE does.
            self.close();
            return EditOutcome::Closed;
        }

        if self.drag.is_none() && input.pointer_pressed {
            if let Some(control) = self.control_at(input.pointer) {
                let outcome = self.apply(control, layout);
                self.layout_panel(layout);
                return outcome;
            }
        }
        if self.pointer_over_controls(input.pointer) && self.drag.is_none() {
            return EditOutcome::Idle;
        }
        let outcome = self.update_drag(input, layout, scale, viewport);
        self.layout_panel(layout);
        outcome
    }

    /// The control under `pointer`, if there is one that does something.
    fn control_at(&self, pointer: (f32, f32)) -> Option<Control> {
        self.controls
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(pointer.0, pointer.1))
            .map(|(control, _)| *control)
    }

    /// Carries out a control.
    fn apply(&mut self, control: Control, layout: &mut HudLayout) -> EditOutcome {
        let (scale, viewport) = (self.scale, self.viewport);
        let step = |steps: i8| i32::from(steps);
        match control {
            Control::Done => {
                self.close();
                return EditOutcome::Closed;
            }
            Control::ResetAll => {
                let prefs = layout.editor_prefs();
                layout.set_config(engine_settings::hud::HudConfig::default());
                layout.set_editor_prefs(prefs);
                self.selected = None;
                return EditOutcome::Changed;
            }
            Control::Back => {
                self.selected = None;
                return EditOutcome::Idle;
            }
            Control::Select(target) => {
                self.selected = Some(target);
                return EditOutcome::Idle;
            }
            Control::Grid => {
                let mut prefs = layout.editor_prefs();
                prefs.grid = !prefs.grid;
                layout.set_editor_prefs(prefs);
                return EditOutcome::Changed;
            }
            Control::Snap => {
                let mut prefs = layout.editor_prefs();
                prefs.snap = !prefs.snap;
                layout.set_editor_prefs(prefs);
                return EditOutcome::Changed;
            }
            Control::PlateSize(steps)
            | Control::PlateDebuffs(steps)
            | Control::PlateBuffs(steps) => {
                let settings = self.nameplates;
                self.nameplates = match control {
                    Control::PlateSize(_) => {
                        let setting = NameplateSetting::Size;
                        settings.with(
                            setting,
                            settings.get(setting) + setting.step() * f32::from(steps),
                        )
                    }
                    Control::PlateDebuffs(_) => NameplateSettings {
                        debuff_size: nudge_percent(settings.debuff_size, steps),
                        ..settings
                    },
                    _ => NameplateSettings {
                        buff_size: nudge_percent(settings.buff_size, steps),
                        ..settings
                    },
                };
                return if self.nameplates == settings {
                    EditOutcome::Idle
                } else {
                    EditOutcome::NameplatesChanged
                };
            }
            Control::ResetElement if self.selected == Some(EditTarget::Nameplates) => {
                let fresh = NameplateSettings::default();
                let settings = self.nameplates;
                self.nameplates = NameplateSettings {
                    size: fresh.size,
                    buff_size: fresh.buff_size,
                    ..settings
                };
                return EditOutcome::NameplatesChanged;
            }
            Control::ResetElement if self.selected == Some(EditTarget::EnemyDebuffs) => {
                self.nameplates = NameplateSettings {
                    debuff_size: NameplateSettings::default().debuff_size,
                    ..self.nameplates
                };
                return EditOutcome::NameplatesChanged;
            }
            _ => {}
        }
        let Some(EditTarget::Element(id)) = self.selected else {
            return EditOutcome::Idle;
        };
        let before = layout.config().clone();
        match control {
            Control::ResetElement => layout.reset_element(id),
            Control::Visible => {
                layout.toggle(id);
            }
            Control::Anchor(index) => {
                if let Some(anchor) = HudAnchor::ALL.get(usize::from(index)) {
                    layout.set_anchor(id, *anchor, scale, viewport);
                }
            }
            Control::Scale(steps) => layout.nudge_scale(id, step(steps)),
            Control::Width(steps) => layout.nudge_extent(id, step(steps), 0, scale, viewport),
            Control::Height(steps) => layout.nudge_extent(id, 0, step(steps), scale, viewport),
            Control::Spacing(steps) => layout.nudge_spacing(id, step(steps), scale, viewport),
            Control::Orientation(orientation) => {
                if let Some(index) = id.bar_index()
                    && layout.orientation(index) != orientation
                {
                    layout.flip_orientation(index);
                }
            }
            Control::FlowX(right_to_left) => {
                let direction = layout.direction(id);
                layout.set_direction(
                    id,
                    FlowDirection {
                        right_to_left,
                        ..direction
                    },
                );
            }
            Control::FlowY(bottom_to_top) => {
                let direction = layout.direction(id);
                layout.set_direction(
                    id,
                    FlowDirection {
                        bottom_to_top,
                        ..direction
                    },
                );
            }
            _ => {}
        }
        if layout.config() == &before {
            EditOutcome::Idle
        } else {
            EditOutcome::Changed
        }
    }

    /// Selects, drags, snaps and drops.
    fn update_drag(
        &mut self,
        input: &UiInput,
        layout: &mut HudLayout,
        scale: UiScale,
        viewport: (u32, u32),
    ) -> EditOutcome {
        let (x, y) = input.pointer;

        if input.pointer_pressed && self.drag.is_none() {
            // Topmost first, so an element dragged on top of another is the one that is
            // picked up next time — the only ordering a player can predict.
            if let Some(element) = Self::element_at(layout, (x, y), scale, viewport) {
                let rect = layout.rect_of(element, scale, viewport);
                self.selected = Some(EditTarget::Element(element));
                self.drag = Some(Drag {
                    element,
                    grab: (x - rect.x, y - rect.y),
                    moved: false,
                });
            } else if self.enemy_debuffs.iter().any(|rect| rect.contains(x, y)) {
                // The debuffs over an enemy: not draggable — they go where the enemy goes —
                // but selectable, for their size.
                self.selected = Some(EditTarget::EnemyDebuffs);
            } else {
                // A press on empty space deselects, so the inspector stops acting on
                // something the player has stopped looking at.
                self.selected = None;
            }
            return EditOutcome::Idle;
        }

        let Some(drag) = self.drag else {
            return EditOutcome::Idle;
        };

        if input.pointer_released || !input.pointer_down {
            self.drag = None;
            self.guides = (None, None);
            return if drag.moved && input.pointer_released {
                EditOutcome::Changed
            } else {
                EditOutcome::Idle
            };
        }

        let element = drag.element;
        let before = layout.rect_of(element, scale, viewport);
        let mut target = (x - drag.grab.0, y - drag.grab.1);
        self.guides = (None, None);
        if layout.editor_prefs().snap {
            let (snapped, guides) = self.snap(layout, element, target, before, scale, viewport);
            target = snapped;
            self.guides = guides;
        }
        layout.move_to(
            element,
            target,
            layout.element_size(element),
            scale,
            viewport,
        );
        let after = layout.rect_of(element, scale, viewport);
        if after == before {
            return EditOutcome::Idle;
        }
        // Marked here rather than on the press, so a click that selects without moving does
        // not mark the configuration dirty and rewrite the file.
        if let Some(drag) = self.drag.as_mut() {
            drag.moved = true;
        }
        EditOutcome::Changed
    }

    /// Pulls a dragged rectangle's edges or centre onto the nearest guide within reach: the
    /// screen's edges and centre lines, the other shown elements' edges and centres, and the
    /// grid when it is on. Returns the new top-left and the guides it used.
    fn snap(
        &self,
        layout: &HudLayout,
        element: HudElementId,
        target: (f32, f32),
        current: UiRect,
        scale: UiScale,
        viewport: (u32, u32),
    ) -> ((f32, f32), (Option<f32>, Option<f32>)) {
        let (width, height) = (current.width, current.height);
        let (screen_w, screen_h) = (viewport.0 as f32, viewport.1 as f32);
        let reach = scale.px(layout::SNAP);
        let grid = layout.editor_prefs().grid.then(|| scale.px(layout::GRID));

        let best = |position: f32,
                    size: f32,
                    screen: f32,
                    others: &mut dyn Iterator<Item = (f32, f32)>| {
            let mut found: Option<(f32, f32, f32)> = None; // (distance, new position, guide)
            let mut consider = |line: f32| {
                for (offset, _) in [(0.0, 0), (size * 0.5, 1), (size, 2)] {
                    let edge = position + offset;
                    let distance = (edge - line).abs();
                    if distance <= reach && found.is_none_or(|(best, _, _)| distance < best) {
                        found = Some((distance, line - offset, line));
                    }
                }
            };
            for line in [0.0, screen * 0.5, screen] {
                consider(line);
            }
            for (start, length) in others {
                for line in [start, start + length * 0.5, start + length] {
                    consider(line);
                }
            }
            if let Some(pitch) = grid {
                let nearest = (position / pitch).round() * pitch;
                consider(nearest);
                let nearest_end = ((position + size) / pitch).round() * pitch;
                consider(nearest_end);
            }
            found.map(|(_, position, guide)| (position, guide))
        };

        let others = || {
            HudElementId::all()
                .filter(move |id| *id != element && layout.is_enabled(*id))
                .map(move |id| layout.rect_of(id, scale, viewport))
        };
        let x = best(
            target.0,
            width,
            screen_w,
            &mut others().map(|rect| (rect.x, rect.width)),
        );
        let y = best(
            target.1,
            height,
            screen_h,
            &mut others().map(|rect| (rect.y, rect.height)),
        );
        (
            (
                x.map_or(target.0, |(position, _)| position),
                y.map_or(target.1, |(position, _)| position),
            ),
            (x.map(|(_, guide)| guide), y.map(|(_, guide)| guide)),
        )
    }

    /// Whether a point is on the inspector.
    #[must_use]
    pub fn pointer_over_controls(&self, pointer: (f32, f32)) -> bool {
        self.active && self.panel.contains(pointer.0, pointer.1)
    }

    /// Which element is under a point, topmost first.
    ///
    /// Hidden elements are still pickable: an element the player hid has to be selectable
    /// again or it could never be shown. This is the one place a hidden element is
    /// interactive, and only while the editor is open.
    #[must_use]
    pub fn element_at(
        layout: &HudLayout,
        point: (f32, f32),
        scale: UiScale,
        viewport: (u32, u32),
    ) -> Option<HudElementId> {
        let mut found = None;
        for id in HudElementId::all() {
            if layout
                .rect_of(id, scale, viewport)
                .contains(point.0, point.1)
            {
                found = Some(id);
            }
        }
        found
    }

    // ------------------------------------------------------------------ the inspector

    /// Works out where every control of the inspector is, for the current selection.
    fn layout_panel(&mut self, layout: &HudLayout) {
        let kit = Kit::new(self.scale);
        let px = |v: f32| kit.px(v);
        let (screen_w, screen_h) = (self.viewport.0 as f32, self.viewport.1 as f32);
        let width = px(layout::PANEL_WIDTH);
        let pad = px(layout::PAD);
        let row = px(layout::ROW);
        let gap = px(layout::GAP);
        let inner = width - pad * 2.0;

        self.controls.clear();
        self.headings.clear();
        // Laid out at the origin, then moved into place once its height is known.
        let mut y = 0.0;
        let header = px(layout::HEADER);
        let section = px(22.0) + gap * 0.5;
        let push = |controls: &mut Vec<(Control, UiRect)>, control: Control, rect: UiRect| {
            controls.push((control, rect));
        };

        match self.selected {
            Some(EditTarget::Element(id)) => {
                let controls = id.controls();
                push(
                    &mut self.controls,
                    Control::Back,
                    UiRect::new(pad, pad, px(28.0), px(28.0)),
                );
                y += pad + header;

                self.headings.push(Heading {
                    top: y,
                    title: "POSITION",
                });
                y += section;
                let grid = px(layout::ANCHOR_GRID);
                for index in 0..9u8 {
                    let cell =
                        kit::anchor_cell(UiRect::new(pad, y, grid, grid), usize::from(index));
                    push(&mut self.controls, Control::Anchor(index), cell);
                }
                y += grid + gap * 1.5;

                self.headings.push(Heading {
                    top: y,
                    title: "SIZE",
                });
                y += section;
                self.push_stepper(
                    Control::Scale(-1),
                    Control::Scale(1),
                    UiRect::new(pad, y, inner, row),
                    &kit,
                );
                y += row + gap;
                if controls.width.is_some() {
                    self.push_stepper(
                        Control::Width(-1),
                        Control::Width(1),
                        UiRect::new(pad, y, inner, row),
                        &kit,
                    );
                    y += row + gap;
                }
                if controls.height.is_some() {
                    self.push_stepper(
                        Control::Height(-1),
                        Control::Height(1),
                        UiRect::new(pad, y, inner, row),
                        &kit,
                    );
                    y += row + gap;
                }
                y += gap * 0.5;

                if controls.has_layout() {
                    self.headings.push(Heading {
                        top: y,
                        title: "LAYOUT",
                    });
                    y += section;
                    let label = px(92.0);
                    let field = UiRect::new(pad + label, y, inner - label, row);
                    if controls.orientation {
                        for (index, orientation) in
                            [HudOrientation::Horizontal, HudOrientation::Vertical]
                                .into_iter()
                                .enumerate()
                        {
                            push(
                                &mut self.controls,
                                Control::Orientation(orientation),
                                kit::segment_rect(field, 2, index),
                            );
                        }
                        y += row + gap;
                    }
                    if controls.flow_x {
                        let field = UiRect::new(pad + label, y, inner - label, row);
                        for (index, rtl) in [false, true].into_iter().enumerate() {
                            push(
                                &mut self.controls,
                                Control::FlowX(rtl),
                                kit::segment_rect(field, 2, index),
                            );
                        }
                        y += row + gap;
                    }
                    if controls.flow_y {
                        let field = UiRect::new(pad + label, y, inner - label, row);
                        for (index, btt) in [false, true].into_iter().enumerate() {
                            push(
                                &mut self.controls,
                                Control::FlowY(btt),
                                kit::segment_rect(field, 2, index),
                            );
                        }
                        y += row + gap;
                    }
                    if controls.spacing {
                        self.push_stepper(
                            Control::Spacing(-1),
                            Control::Spacing(1),
                            UiRect::new(pad, y, inner, row),
                            &kit,
                        );
                        y += row + gap;
                    }
                    y += gap * 0.5;
                }

                self.headings.push(Heading {
                    top: y,
                    title: "VISIBILITY",
                });
                y += section;
                push(
                    &mut self.controls,
                    Control::Visible,
                    UiRect::new(pad, y, inner, row),
                );
                y += row + gap * 2.0;

                let half = (inner - gap) * 0.5;
                push(
                    &mut self.controls,
                    Control::ResetElement,
                    UiRect::new(pad, y, half, px(34.0)),
                );
                push(
                    &mut self.controls,
                    Control::Done,
                    UiRect::new(pad + half + gap, y, half, px(34.0)),
                );
                y += px(34.0) + pad;
                let _ = layout;
            }
            Some(target @ (EditTarget::Nameplates | EditTarget::EnemyDebuffs)) => {
                push(
                    &mut self.controls,
                    Control::Back,
                    UiRect::new(pad, pad, px(28.0), px(28.0)),
                );
                y += pad + header;
                let (title, steppers): (&str, &[(Control, Control)]) =
                    if target == EditTarget::EnemyDebuffs {
                        (
                            "ON EVERY ENEMY",
                            &[(Control::PlateDebuffs(-1), Control::PlateDebuffs(1))],
                        )
                    } else {
                        (
                            "NAMEPLATES",
                            &[
                                (Control::PlateSize(-1), Control::PlateSize(1)),
                                (Control::PlateBuffs(-1), Control::PlateBuffs(1)),
                            ],
                        )
                    };
                self.headings.push(Heading { top: y, title });
                y += section;
                for &(minus, plus) in steppers {
                    self.push_stepper(minus, plus, UiRect::new(pad, y, inner, row), &kit);
                    y += row + gap;
                }
                y += gap * 1.5;
                let half = (inner - gap) * 0.5;
                push(
                    &mut self.controls,
                    Control::ResetElement,
                    UiRect::new(pad, y, half, px(34.0)),
                );
                push(
                    &mut self.controls,
                    Control::Done,
                    UiRect::new(pad + half + gap, y, half, px(34.0)),
                );
                y += px(34.0) + pad;
            }
            None => {
                y += pad + header;
                self.headings.push(Heading {
                    top: y,
                    title: "ELEMENTS",
                });
                y += section;
                let column = (inner - gap) * 0.5;
                let list_row = px(layout::LIST_ROW);
                let mut index: usize = 0;
                for id in list_order() {
                    let x = pad + (index % 2) as f32 * (column + gap);
                    let top = y + (index / 2) as f32 * (list_row + px(4.0));
                    push(
                        &mut self.controls,
                        Control::Select(EditTarget::Element(id)),
                        UiRect::new(x, top, column, list_row),
                    );
                    index += 1;
                }
                for target in [EditTarget::Nameplates, EditTarget::EnemyDebuffs] {
                    let x = pad + (index % 2) as f32 * (column + gap);
                    let top = y + (index / 2) as f32 * (list_row + px(4.0));
                    push(
                        &mut self.controls,
                        Control::Select(target),
                        UiRect::new(x, top, column, list_row),
                    );
                    index += 1;
                }
                y += index.div_ceil(2) as f32 * (list_row + px(4.0)) + gap;

                self.headings.push(Heading {
                    top: y,
                    title: "EDITOR",
                });
                y += section;
                push(
                    &mut self.controls,
                    Control::Grid,
                    UiRect::new(pad, y, inner, row),
                );
                y += row + gap * 0.5;
                push(
                    &mut self.controls,
                    Control::Snap,
                    UiRect::new(pad, y, inner, row),
                );
                y += row + gap * 2.0;
                let half = (inner - gap) * 0.5;
                push(
                    &mut self.controls,
                    Control::ResetAll,
                    UiRect::new(pad, y, half, px(34.0)),
                );
                push(
                    &mut self.controls,
                    Control::Done,
                    UiRect::new(pad + half + gap, y, half, px(34.0)),
                );
                y += px(34.0) + pad;
            }
        }

        // Into place: the right edge, unless what is selected is there — then the left.
        let height = y;
        let margin = px(layout::MARGIN);
        let top = ((screen_h - height) * 0.5).max(margin);
        let mut x = screen_w - margin - width;
        if let Some(EditTarget::Element(id)) = self.selected {
            let rect = layout.rect_of(id, self.scale, self.viewport);
            let right = UiRect::new(x, top, width, height);
            if overlaps(rect, right) {
                x = margin;
            }
        }
        self.panel = UiRect::new(x.round(), top.round(), width, height.round());
        for (_, rect) in &mut self.controls {
            rect.x = (rect.x + self.panel.x).round();
            rect.y = (rect.y + self.panel.y).round();
        }
        for heading in &mut self.headings {
            heading.top = (heading.top + self.panel.y).round();
        }
    }

    /// A stepper's two buttons, where [`kit::stepper_parts`] puts them in `rect`. The painter
    /// rebuilds the row from the two: its label on the left, the value between them.
    fn push_stepper(&mut self, minus: Control, plus: Control, rect: UiRect, kit: &Kit) {
        let parts = kit::stepper_parts(kit, rect);
        self.controls.push((minus, parts.minus));
        self.controls.push((plus, parts.plus));
    }

    fn interaction(&self, rect: UiRect) -> Interaction {
        let hovered = rect.contains(self.pointer.0, self.pointer.1) && self.drag.is_none();
        Interaction {
            hovered,
            pressed: hovered && self.pointer_down,
            active: false,
            disabled: false,
        }
    }
}

/// The elements in the order the inspector lists them: frames first, then the rest, then the
/// bars.
fn list_order() -> impl Iterator<Item = HudElementId> {
    HudElementId::FRAMES
        .into_iter()
        .chain([
            HudElementId::Resource,
            HudElementId::Energy,
            HudElementId::Raid,
            HudElementId::Buffs,
            HudElementId::Debuffs,
            HudElementId::TargetBuffs,
            HudElementId::TargetDebuffs,
            HudElementId::CastBar,
            HudElementId::Tooltip,
        ])
        .chain((0..engine_settings::hud::ACTION_BARS).map(HudElementId::ActionBar))
}

/// Whether two rectangles share any area.
fn overlaps(a: UiRect, b: UiRect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// A percentage setting one step of ten further, held to the icons' range.
fn nudge_percent(value: f32, steps: i8) -> f32 {
    use engine_settings::nameplate::{
        NAMEPLATE_AURA_SIZE_MAX, NAMEPLATE_AURA_SIZE_MIN, NAMEPLATE_AURA_SIZE_STEP,
    };
    (value + NAMEPLATE_AURA_SIZE_STEP * f32::from(steps))
        .clamp(NAMEPLATE_AURA_SIZE_MIN, NAMEPLATE_AURA_SIZE_MAX)
}

/// The anchor an element hangs from, as the inspector prints it.
fn anchor_phrase(anchor: HudAnchor) -> &'static str {
    match anchor {
        HudAnchor::TopLeft => "Anchored top left",
        HudAnchor::Top => "Anchored top",
        HudAnchor::TopRight => "Anchored top right",
        HudAnchor::Left => "Anchored left",
        HudAnchor::Center => "Anchored centre",
        HudAnchor::Right => "Anchored right",
        HudAnchor::BottomLeft => "Anchored bottom left",
        HudAnchor::Bottom => "Anchored bottom",
        HudAnchor::BottomRight => "Anchored bottom right",
    }
}

/// Draws edit mode: the grid, every element's outline and name, the guides a drag is
/// snapping to, the mode's badge and the inspector.
///
/// Outlines rather than filled boxes: the player is arranging the interface they will play
/// with, so what they are moving has to look like what it will look like.
#[allow(
    clippy::too_many_lines,
    reason = "one pass over the editor's surfaces, in painter's order"
)]
pub fn push_hud_editor(
    list: &mut UiDrawList,
    editor: &HudEditor,
    layout: &HudLayout,
    label: &mut String,
    scale: UiScale,
    viewport: (u32, u32),
) {
    if !editor.is_active() {
        return;
    }
    let kit = Kit::new(scale);
    let (screen_w, screen_h) = (viewport.0 as f32, viewport.1 as f32);

    // The grid: faint, and only when asked for.
    if layout.editor_prefs().grid {
        let pitch = kit.px(layout::GRID).max(8.0);
        let line = [1.0, 1.0, 1.0, 0.045];
        let mut x = pitch;
        while x < screen_w {
            list.rect(UiRect::new(x.round(), 0.0, 1.0, screen_h), line);
            x += pitch;
        }
        let mut y = pitch;
        while y < screen_h {
            list.rect(UiRect::new(0.0, y.round(), screen_w, 1.0), line);
            y += pitch;
        }
        let centre = [1.0, 1.0, 1.0, 0.10];
        list.rect(
            UiRect::new((screen_w * 0.5).round(), 0.0, 1.0, screen_h),
            centre,
        );
        list.rect(
            UiRect::new(0.0, (screen_h * 0.5).round(), screen_w, 1.0),
            centre,
        );
    }

    // Every element: a whisper of a fill, a hairline, its name. The selected one in the
    // accent, with handles at its corners.
    let selected = editor.selected();
    let hovered = (!editor.pointer_over_controls(editor.pointer) && !editor.is_dragging())
        .then(|| HudEditor::element_at(layout, editor.pointer, scale, viewport))
        .flatten();
    for id in HudElementId::all() {
        if Some(id) == selected {
            continue;
        }
        let rect = layout.rect_of(id, scale, viewport);
        let enabled = layout.is_enabled(id);
        let is_hovered = hovered == Some(id);
        let corner = kit.px(4.0);
        // A hidden element is a faint ghost until the pointer finds it: seven unused action
        // bars stacked over the world would be noise, and the inspector lists them by name.
        if !enabled && !is_hovered {
            list.panel_stroke(rect, corner, 1.0, [1.0, 1.0, 1.0, 0.07]);
            continue;
        }
        let (fill, stroke) = match (enabled, is_hovered) {
            (_, true) => ([1.0, 1.0, 1.0, 0.08], [1.0, 1.0, 1.0, 0.55]),
            (true, false) => ([1.0, 1.0, 1.0, 0.035], [1.0, 1.0, 1.0, 0.22]),
            (false, false) => ([0.0, 0.0, 0.0, 0.30], [1.0, 1.0, 1.0, 0.12]),
        };
        list.panel(rect, corner, fill, fill);
        list.panel_stroke(rect, corner, 1.0, stroke);
        id.write_label(label);
        title_case_in_place(label);
        if !enabled {
            label.push_str(" · Hidden");
        }
        let text = if enabled {
            color::TEXT_PRIMARY
        } else {
            color::TEXT_MUTED
        };
        kit::chip(
            list,
            &kit,
            rect.x + kit.px(4.0),
            rect.y + kit.px(4.0),
            label,
            text,
            [0.03, 0.035, 0.05, 0.78],
        );
    }
    if let Some(id) = selected {
        let rect = layout.rect_of(id, scale, viewport);
        let corner = kit.px(4.0);
        list.glow(
            rect,
            corner,
            kit.px(10.0),
            color::alpha(tokens::ACCENT, 0.30),
        );
        list.panel(
            rect,
            corner,
            color::alpha(tokens::ACCENT, 0.08),
            color::alpha(tokens::ACCENT, 0.08),
        );
        list.panel_stroke(rect, corner, kit.px(2.0).max(2.0), tokens::ACCENT);
        let handle = kit.px(8.0);
        for (x, y) in [
            (rect.x, rect.y),
            (rect.right(), rect.y),
            (rect.x, rect.bottom()),
            (rect.right(), rect.bottom()),
        ] {
            let square = UiRect::new(
                (x - handle * 0.5).round(),
                (y - handle * 0.5).round(),
                handle,
                handle,
            );
            list.panel(square, kit.px(2.0), tokens::ACCENT, tokens::ACCENT);
            list.panel_stroke(square, kit.px(2.0), 1.0, tokens::ON_ACCENT);
        }
        // The name and size above it — below it when it is against the top of the screen.
        use std::fmt::Write as _;
        id.write_label(label);
        title_case_in_place(label);
        let _ = write!(label, "  {}×{}", rect.width.round(), rect.height.round());
        let chip_height = kit.px(20.0);
        let top = if rect.y - chip_height - kit.px(6.0) > 0.0 {
            rect.y - chip_height - kit.px(6.0)
        } else {
            rect.bottom() + kit.px(6.0)
        };
        kit::chip(
            list,
            &kit,
            rect.x,
            top,
            label,
            tokens::ON_ACCENT,
            tokens::ACCENT,
        );
    }

    // The guides a drag is snapping to.
    if let Some(x) = editor.guides.0 {
        list.rect(
            UiRect::new(x.round(), 0.0, 1.0, screen_h),
            color::alpha(tokens::GUIDE, 0.8),
        );
    }
    if let Some(y) = editor.guides.1 {
        list.rect(
            UiRect::new(0.0, y.round(), screen_w, 1.0),
            color::alpha(tokens::GUIDE, 0.8),
        );
    }

    // The debuff rows over every enemy in the fight: a frame of their own, tied to the real
    // plates rather than to the target. Outlined faintly always, so they can be found and
    // clicked, and in the accent while they are what the inspector is editing.
    let enemy_selected = editor.target() == Some(EditTarget::EnemyDebuffs);
    for rect in &editor.enemy_debuffs {
        let pad = kit.px(3.0);
        let outline = UiRect::new(
            rect.x - pad,
            rect.y - pad,
            rect.width + pad * 2.0,
            rect.height + pad * 2.0,
        );
        if enemy_selected {
            list.panel(
                outline,
                kit.px(3.0),
                color::alpha(tokens::ACCENT, 0.10),
                color::alpha(tokens::ACCENT, 0.10),
            );
            list.panel_stroke(outline, kit.px(3.0), kit.px(2.0), tokens::ACCENT);
        } else {
            list.panel_stroke(outline, kit.px(3.0), 1.0, [1.0, 1.0, 1.0, 0.30]);
        }
    }

    // The mode's badge, top centre: says where the player is without covering the world.
    let badge_text = "Edit Mode";
    let badge_w = kit.width(type_scale::UI, badge_text) + kit.px(40.0);
    let badge = UiRect::new(
        ((screen_w - badge_w) * 0.5).round(),
        kit.px(8.0),
        badge_w.round(),
        kit.px(26.0),
    );
    list.panel(
        badge,
        badge.height * 0.5,
        color::alpha(tokens::ACCENT, 0.92),
        color::alpha(tokens::ACCENT_DIM, 0.92),
    );
    list.icon(
        UiRect::new(
            badge.x + kit.px(10.0),
            badge.y + kit.px(5.0),
            kit.px(16.0),
            kit.px(16.0),
        ),
        UiIcon::ControlMove,
        tokens::ON_ACCENT,
    );
    kit.text(
        list,
        badge.x + kit.px(30.0),
        badge.y + badge.height * 0.5,
        badge_text,
        type_scale::UI,
        tokens::ON_ACCENT,
        TextAlign::Left,
    );

    push_inspector(list, editor, layout, &kit, label);
}

/// The inspector card and every control on it.
#[allow(
    clippy::too_many_lines,
    reason = "the inspector is one card drawn top to bottom"
)]
fn push_inspector(
    list: &mut UiDrawList,
    editor: &HudEditor,
    layout: &HudLayout,
    kit: &Kit,
    label: &mut String,
) {
    use std::fmt::Write as _;
    let panel = editor.panel;
    kit::panel(list, kit, panel, color::alpha(tokens::ACCENT, 0.28));
    let pad = kit.px(layout::PAD);
    let inner = panel.width - pad * 2.0;
    let rect_of = |control: Control| editor.control_rect(control);
    let state = |control: Control| rect_of(control).map(|rect| editor.interaction(rect));

    // Header.
    let title_x = if rect_of(Control::Back).is_some() {
        panel.x + pad + kit.px(36.0)
    } else {
        panel.x + pad
    };
    let (title, subtitle): (&str, &str) = match editor.selected {
        Some(EditTarget::Element(id)) => {
            id.write_label(label);
            title_case_in_place(label);
            ("", anchor_phrase(layout.placement(id).anchor))
        }
        Some(EditTarget::Nameplates) => ("Nameplates", "Enemy plates in the world"),
        Some(EditTarget::EnemyDebuffs) => ("Enemy Debuffs", "Over every enemy in the fight"),
        None => ("Edit Mode", "Drag anything on screen. Click it for more."),
    };
    let title_text: &str = if title.is_empty() {
        label.as_str()
    } else {
        title
    };
    kit.text(
        list,
        title_x,
        panel.y + pad + kit.px(10.0),
        title_text,
        tokens::PANEL_TITLE,
        color::TEXT_TITLE,
        TextAlign::Left,
    );
    kit.text(
        list,
        title_x,
        panel.y + pad + kit.px(31.0),
        subtitle,
        type_scale::META,
        color::TEXT_MUTED,
        TextAlign::Left,
    );
    if let (Some(rect), Some(state)) = (rect_of(Control::Back), state(Control::Back)) {
        kit::icon_button(
            list,
            kit,
            rect,
            UiIcon::ControlArrow,
            PI,
            ButtonVariant::Ghost,
            state,
        );
    }

    for heading in &editor.headings {
        kit::section(list, kit, panel.x + pad, heading.top, inner, heading.title);
    }

    let selected = editor.selected();
    for &(control, rect) in &editor.controls {
        let interaction = editor.interaction(rect);
        match control {
            Control::Back
            | Control::Scale(1)
            | Control::Width(1)
            | Control::Height(1)
            | Control::Spacing(1)
            | Control::PlateSize(1)
            | Control::PlateDebuffs(1)
            | Control::PlateBuffs(1) => {}
            Control::Visible => {
                let shown = selected.is_some_and(|id| layout.is_enabled(id));
                kit.text(
                    list,
                    rect.x,
                    rect.y + rect.height * 0.5,
                    "Show on screen",
                    type_scale::UI,
                    color::TEXT_SECONDARY,
                    TextAlign::Left,
                );
                kit::switch(list, kit, rect, shown, interaction);
            }
            Control::Grid | Control::Snap => {
                let prefs = layout.editor_prefs();
                let (on, words, icon) = if control == Control::Grid {
                    (prefs.grid, "Alignment grid", UiIcon::ControlGrid)
                } else {
                    (prefs.snap, "Snap to guides", UiIcon::ControlSnap)
                };
                list.icon(
                    UiRect::new(
                        rect.x,
                        rect.y + (rect.height - kit.px(16.0)) * 0.5,
                        kit.px(16.0),
                        kit.px(16.0),
                    ),
                    icon,
                    color::TEXT_SECONDARY,
                );
                kit.text(
                    list,
                    rect.x + kit.px(24.0),
                    rect.y + rect.height * 0.5,
                    words,
                    type_scale::UI,
                    color::TEXT_SECONDARY,
                    TextAlign::Left,
                );
                kit::switch(list, kit, rect, on, interaction);
            }
            Control::Anchor(index) => {
                if index == 0 {
                    let grid =
                        UiRect::new(rect.x, rect.y, rect.width * 3.0 + 0.0, rect.height * 3.0);
                    let chosen = selected.map(|id| layout.placement(id).anchor);
                    let mut states = [Interaction::default(); 9];
                    for (cell, anchor) in states.iter_mut().zip(HudAnchor::ALL) {
                        *cell = Interaction::default().active(Some(anchor) == chosen);
                    }
                    for (cell_index, cell) in states.iter_mut().enumerate() {
                        if let Some(rect) = rect_of(Control::Anchor(cell_index as u8)) {
                            let live = editor.interaction(rect);
                            cell.hovered = live.hovered;
                            cell.pressed = live.pressed;
                        }
                    }
                    kit::anchor_grid(list, kit, grid, &states);
                }
            }
            Control::Scale(_)
            | Control::Width(_)
            | Control::Height(_)
            | Control::Spacing(_)
            | Control::PlateSize(_)
            | Control::PlateDebuffs(_)
            | Control::PlateBuffs(_) => {
                // The minus half: the stepper is drawn once, from here.
                let plus = match control {
                    Control::Scale(_) => Control::Scale(1),
                    Control::Width(_) => Control::Width(1),
                    Control::Height(_) => Control::Height(1),
                    Control::Spacing(_) => Control::Spacing(1),
                    Control::PlateSize(_) => Control::PlateSize(1),
                    Control::PlateDebuffs(_) => Control::PlateDebuffs(1),
                    _ => Control::PlateBuffs(1),
                };
                let Some(plus_rect) = rect_of(plus) else {
                    continue;
                };
                let left = panel.x + pad;
                let row = UiRect::new(left, rect.y, plus_rect.right() - left, rect.height);
                let (name, value) = stepper_text(control, selected, editor, layout, label);
                label.clear();
                label.push_str(&value);
                kit::stepper(
                    list,
                    kit,
                    row,
                    name,
                    label,
                    interaction,
                    editor.interaction(plus_rect),
                );
            }
            Control::Orientation(orientation) => {
                if orientation == HudOrientation::Horizontal {
                    let current = selected
                        .and_then(HudElementId::bar_index)
                        .map(|index| layout.orientation(index));
                    segmented_row(
                        list,
                        kit,
                        editor,
                        panel,
                        rect,
                        "Orientation",
                        [
                            (
                                Control::Orientation(HudOrientation::Horizontal),
                                "Row",
                                None,
                                current == Some(HudOrientation::Horizontal),
                            ),
                            (
                                Control::Orientation(HudOrientation::Vertical),
                                "Column",
                                None,
                                current == Some(HudOrientation::Vertical),
                            ),
                        ],
                    );
                }
            }
            Control::FlowX(right_to_left) => {
                if !right_to_left {
                    let current = selected.map(|id| layout.direction(id).right_to_left);
                    segmented_row(
                        list,
                        kit,
                        editor,
                        panel,
                        rect,
                        "Fill across",
                        [
                            (
                                Control::FlowX(false),
                                "From left",
                                None,
                                current == Some(false),
                            ),
                            (
                                Control::FlowX(true),
                                "From right",
                                None,
                                current == Some(true),
                            ),
                        ],
                    );
                }
            }
            Control::FlowY(bottom_to_top) => {
                if !bottom_to_top {
                    let current = selected.map(|id| layout.direction(id).bottom_to_top);
                    segmented_row(
                        list,
                        kit,
                        editor,
                        panel,
                        rect,
                        "Fill down",
                        [
                            (
                                Control::FlowY(false),
                                "From top",
                                None,
                                current == Some(false),
                            ),
                            (
                                Control::FlowY(true),
                                "From bottom",
                                None,
                                current == Some(true),
                            ),
                        ],
                    );
                }
            }
            Control::Select(target) => {
                let (hidden, words) = match target {
                    EditTarget::Element(id) => {
                        id.write_label(label);
                        title_case_in_place(label);
                        (!layout.is_enabled(id), label.as_str())
                    }
                    EditTarget::Nameplates => (false, "Nameplates"),
                    EditTarget::EnemyDebuffs => (false, "Enemy Debuffs"),
                };
                kit::button(
                    list,
                    kit,
                    rect,
                    words,
                    hidden.then_some((UiIcon::ControlEyeOff, 0.0)),
                    ButtonVariant::Secondary,
                    interaction,
                );
            }
            Control::ResetElement => kit::button(
                list,
                kit,
                rect,
                "Reset",
                Some((UiIcon::ControlReset, 0.0)),
                ButtonVariant::Secondary,
                interaction,
            ),
            Control::ResetAll => kit::button(
                list,
                kit,
                rect,
                "Reset all",
                Some((UiIcon::ControlReset, 0.0)),
                ButtonVariant::Danger,
                interaction,
            ),
            Control::Done => kit::button(
                list,
                kit,
                rect,
                "Done",
                Some((UiIcon::Check, 0.0)),
                ButtonVariant::Primary,
                interaction,
            ),
        }
    }
    let _ = write!(label, "");
}

/// One option of an inspector segmented row: what it does, its words, its icon, whether it
/// is the one in force.
type SegmentOption<'a> = (Control, &'a str, Option<(UiIcon, f32)>, bool);

/// A labelled two-option segmented control on one row of the inspector.
#[allow(
    clippy::too_many_arguments,
    reason = "a labelled control row is this many facts"
)]
fn segmented_row(
    list: &mut UiDrawList,
    kit: &Kit,
    editor: &HudEditor,
    panel: UiRect,
    first: UiRect,
    label: &str,
    options: [SegmentOption<'_>; 2],
) {
    let pad = kit.px(layout::PAD);
    kit.text(
        list,
        panel.x + pad,
        first.y + first.height * 0.5,
        label,
        type_scale::UI,
        color::TEXT_SECONDARY,
        TextAlign::Left,
    );
    let Some(last) = editor.control_rect(options[1].0) else {
        return;
    };
    let field = UiRect::new(first.x, first.y, last.right() - first.x, first.height);
    let segments = [
        Segment {
            label: options[0].1,
            icon: options[0].2,
        },
        Segment {
            label: options[1].1,
            icon: options[1].2,
        },
    ];
    let states = [
        editor.interaction(first).active(options[0].3),
        editor.interaction(last).active(options[1].3),
    ];
    kit::segmented(list, kit, field, &segments, &states);
}

/// A stepper's name and the value it prints.
fn stepper_text(
    control: Control,
    selected: Option<HudElementId>,
    editor: &HudEditor,
    layout: &HudLayout,
    scratch: &mut String,
) -> (&'static str, String) {
    use std::fmt::Write as _;
    let _ = scratch;
    let mut value = String::new();
    let name = match (control, selected) {
        (Control::Scale(_), Some(id)) => {
            let _ = write!(value, "{}%", (layout.scale_of(id) * 100.0).round());
            "Scale"
        }
        (Control::Width(_), Some(id)) => {
            let spec = id.controls().width;
            let _ = write!(
                value,
                "{} {}",
                layout.extent(id).0.unwrap_or(0.0).round(),
                spec.map_or("", |spec| spec.unit)
            );
            "Width"
        }
        (Control::Height(_), Some(id)) => {
            let spec = id.controls().height;
            let _ = write!(
                value,
                "{} {}",
                layout.extent(id).1.unwrap_or(0.0).round(),
                spec.map_or("", |spec| spec.unit)
            );
            "Height"
        }
        (Control::Spacing(_), Some(id)) => {
            let _ = write!(value, "{:.2}×", layout.spacing(id));
            "Spacing"
        }
        (Control::PlateSize(_), _) => {
            let _ = write!(value, "{}", editor.nameplates.size.round());
            "Nameplate scale"
        }
        (Control::PlateDebuffs(_), _) => {
            let _ = write!(value, "{}%", editor.nameplates.debuff_size.round());
            "Enemy debuff size"
        }
        (Control::PlateBuffs(_), _) => {
            let _ = write!(value, "{}%", editor.nameplates.buff_size.round());
            "Enemy buff size"
        }
        _ => "",
    };
    (name, value)
}

/// Title-cases an upper-case label in place: "ACTION BAR 1" reads "Action Bar 1".
fn title_case_in_place(label: &mut String) {
    let upper = std::mem::take(label);
    write_title_case(label, &upper);
}

/// The colour an element's chip is drawn in. Kept for callers outside the editor that want
/// the same accent.
#[must_use]
pub fn accent() -> UiColor {
    tokens::ACCENT
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_settings::hud::{HUD_SCALE_STEP, HudConfig, HudFlowX};

    const FHD: (u32, u32) = (1920, 1080);

    fn editor() -> (HudEditor, HudLayout) {
        let mut editor = HudEditor::new();
        editor.open();
        let mut layout = HudLayout::default();
        // Everything on, so every element can be picked up in these tests; snapping off, so
        // a drag lands exactly where the pointer put it.
        for id in HudElementId::all() {
            layout.set_enabled(id, true);
        }
        let mut prefs = layout.editor_prefs();
        prefs.snap = false;
        layout.set_editor_prefs(prefs);
        editor.layout(UiScale::default(), FHD);
        editor.layout_panel(&layout);
        (editor, layout)
    }

    fn at(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            ..UiInput::new()
        }
    }

    fn press(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        }
    }

    fn held(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_down: true,
            ..UiInput::new()
        }
    }

    fn release(pointer: (f32, f32)) -> UiInput {
        UiInput {
            pointer,
            pointer_released: true,
            ..UiInput::new()
        }
    }

    fn centre(layout: &HudLayout, id: HudElementId) -> (f32, f32) {
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// Selects `id` the way a player would: from the inspector's list, so an element hidden
    /// under the inspector or another element is still reachable.
    fn select(editor: &mut HudEditor, layout: &mut HudLayout, id: HudElementId) {
        if editor.selected().is_some() {
            click(editor, layout, Control::Back);
        }
        click(editor, layout, Control::Select(EditTarget::Element(id)));
        assert_eq!(editor.selected(), Some(id));
    }

    fn click(editor: &mut HudEditor, layout: &mut HudLayout, control: Control) -> EditOutcome {
        let rect = editor
            .control_rect(control)
            .expect("the control is showing");
        let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let outcome = editor.update(&press(centre), layout, UiScale::default(), FHD);
        editor.update(&release(centre), layout, UiScale::default(), FHD);
        outcome
    }

    /// A point on `id` that the inspector does not cover.
    fn uncovered_point(editor: &HudEditor, layout: &HudLayout, id: HudElementId) -> (f32, f32) {
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        let mut best = centre(layout, id);
        for fx in [0.5, 0.2, 0.8, 0.05, 0.95] {
            for fy in [0.5, 0.2, 0.8] {
                let point = (rect.x + rect.width * fx, rect.y + rect.height * fy);
                if !editor.pointer_over_controls(point)
                    && HudEditor::element_at(layout, point, UiScale::default(), FHD) == Some(id)
                {
                    return point;
                }
                best = point;
            }
        }
        best
    }

    #[test]
    fn a_closed_editor_does_nothing_at_all() {
        let mut editor = HudEditor::new();
        let mut layout = HudLayout::default();
        let before = layout.clone();
        assert!(!editor.is_active());
        assert_eq!(
            editor.update(&press((100.0, 100.0)), &mut layout, UiScale::default(), FHD),
            EditOutcome::Idle
        );
        assert_eq!(layout, before, "a closed editor moved something");
    }

    #[test]
    fn every_element_can_be_selected_and_moved() {
        for id in HudElementId::all() {
            let (mut editor, mut layout) = editor();
            // Somewhere the inspector is not.
            let target = (620.0, 520.0);
            let from = uncovered_point(&editor, &layout, id);
            let rect = layout.rect_of(id, UiScale::default(), FHD);
            let grab = (from.0 - rect.x, from.1 - rect.y);
            editor.update(&press(from), &mut layout, UiScale::default(), FHD);
            assert_eq!(editor.selected(), Some(id), "{id:?} was not selected");
            let moved = editor.update(&held(target), &mut layout, UiScale::default(), FHD);
            editor.update(&release(target), &mut layout, UiScale::default(), FHD);
            assert_eq!(moved, EditOutcome::Changed, "{id:?} did not move");
            let after = layout.rect_of(id, UiScale::default(), FHD);
            assert!(
                (after.x - (target.0 - grab.0)).abs() < 1.0
                    && (after.y - (target.1 - grab.1)).abs() < 1.0,
                "{id:?} landed at {after:?}"
            );
        }
    }

    #[test]
    fn an_element_does_not_jump_to_the_cursor_when_the_drag_starts() {
        let (mut editor, mut layout) = editor();
        let id = HudElementId::Player;
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        let grab = (rect.x + 4.0, rect.y + 4.0);
        editor.update(&press(grab), &mut layout, UiScale::default(), FHD);
        assert_eq!(
            layout.rect_of(id, UiScale::default(), FHD),
            rect,
            "the press alone moved it"
        );
        editor.update(
            &held((grab.0 + 1.0, grab.1)),
            &mut layout,
            UiScale::default(),
            FHD,
        );
        let moved = layout.rect_of(id, UiScale::default(), FHD);
        assert!(
            (moved.x - rect.x - 1.0).abs() < 0.5,
            "{rect:?} -> {moved:?}"
        );
    }

    #[test]
    fn a_click_that_selects_without_moving_changes_nothing() {
        let (mut editor, mut layout) = editor();
        let before = layout.clone();
        let point = uncovered_point(&editor, &layout, HudElementId::Player);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        assert_eq!(
            editor.update(&release(point), &mut layout, UiScale::default(), FHD),
            EditOutcome::Idle,
            "a click was reported as a change"
        );
        assert_eq!(layout, before);
        assert_eq!(editor.selected(), Some(HudElementId::Player));
    }

    #[test]
    fn nothing_can_be_dragged_out_of_reach() {
        let (mut editor, mut layout) = editor();
        for id in HudElementId::all() {
            for target in [
                (-9000.0, -9000.0),
                (9000.0, 9000.0),
                (-9000.0, 9000.0),
                (9000.0, -9000.0),
            ] {
                select(&mut editor, &mut layout, id);
                // Grabbed through the inspector-proof path: a press on the element itself.
                let from = uncovered_point(&editor, &layout, id);
                editor.update(&press(from), &mut layout, UiScale::default(), FHD);
                editor.update(&held(target), &mut layout, UiScale::default(), FHD);
                editor.update(&release(target), &mut layout, UiScale::default(), FHD);
                let rect = layout.rect_of(id, UiScale::default(), FHD);
                let visible_x = (rect.right().min(1920.0) - rect.x.max(0.0)).max(0.0);
                let visible_y = (rect.bottom().min(1080.0) - rect.y.max(0.0)).max(0.0);
                assert!(
                    visible_x > 1.0 && visible_y > 1.0,
                    "{id:?} left the screen: {rect:?}"
                );
                // And it can always be found again by name.
                select(&mut editor, &mut layout, id);
            }
        }
    }

    #[test]
    fn the_inspector_acts_on_the_selected_element_and_nothing_else() {
        let (mut editor, mut layout) = editor();
        let id = HudElementId::Target;
        let other = HudElementId::Player;
        select(&mut editor, &mut layout, id);

        let before_other = layout.placement(other);
        assert_eq!(
            click(&mut editor, &mut layout, Control::Scale(1)),
            EditOutcome::Changed
        );
        assert!((layout.scale_of(id) - (1.0 + HUD_SCALE_STEP)).abs() < 1e-5);
        assert_eq!(
            layout.placement(other),
            before_other,
            "the wrong element changed"
        );
        assert_eq!(
            click(&mut editor, &mut layout, Control::Scale(-1)),
            EditOutcome::Changed
        );
        assert!((layout.scale_of(id) - 1.0).abs() < 1e-5);

        assert!(layout.is_enabled(id));
        assert_eq!(
            click(&mut editor, &mut layout, Control::Visible),
            EditOutcome::Changed
        );
        assert!(!layout.is_enabled(id), "the element was not hidden");
        assert_eq!(
            click(&mut editor, &mut layout, Control::Visible),
            EditOutcome::Changed
        );
        assert!(
            layout.is_enabled(id),
            "the element could not be shown again"
        );
    }

    #[test]
    fn only_a_bar_offers_an_orientation_and_it_turns_the_bar() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::ActionBar(0));
        assert_eq!(
            click(
                &mut editor,
                &mut layout,
                Control::Orientation(HudOrientation::Vertical)
            ),
            EditOutcome::Changed
        );
        assert_eq!(layout.orientation(0), HudOrientation::Vertical);

        select(&mut editor, &mut layout, HudElementId::Party);
        assert!(
            editor
                .control_rect(Control::Orientation(HudOrientation::Vertical))
                .is_none()
        );
        // A frame with no items has no layout section at all.
        select(&mut editor, &mut layout, HudElementId::Player);
        assert!(editor.control_rect(Control::FlowX(true)).is_none());
        assert!(editor.control_rect(Control::Spacing(1)).is_none());
    }

    #[test]
    fn the_buff_bar_can_be_turned_round_on_both_axes_and_it_persists() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::Buffs);
        // Its natural direction: right to left, top to bottom.
        assert!(layout.direction(HudElementId::Buffs).right_to_left);
        assert_eq!(
            click(&mut editor, &mut layout, Control::FlowX(false)),
            EditOutcome::Changed
        );
        assert_eq!(
            click(&mut editor, &mut layout, Control::FlowY(true)),
            EditOutcome::Changed
        );
        let direction = layout.direction(HudElementId::Buffs);
        assert!(!direction.right_to_left && direction.bottom_to_top);
        assert_eq!(layout.config().frames.buffs.flow_x, HudFlowX::LeftToRight);
        // And it is in the file the layout writes.
        let reloaded = HudLayout::new(layout.config().clone());
        assert_eq!(reloaded.direction(HudElementId::Buffs), direction);
    }

    #[test]
    fn width_height_and_spacing_are_one_generic_control_each() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::Buffs);
        let (columns, rows) = layout.extent(HudElementId::Buffs);
        click(&mut editor, &mut layout, Control::Width(-1));
        click(&mut editor, &mut layout, Control::Height(1));
        let (narrower, taller) = layout.extent(HudElementId::Buffs);
        assert_eq!(narrower, columns.map(|value| value - 1.0));
        assert_eq!(taller, rows.map(|value| value + 1.0));
        let spaced = layout.spacing(HudElementId::Buffs);
        click(&mut editor, &mut layout, Control::Spacing(1));
        assert!(layout.spacing(HudElementId::Buffs) > spaced);

        // The cast bar's width is the same control, in pixels.
        select(&mut editor, &mut layout, HudElementId::CastBar);
        let start = layout.cast_bar_size();
        click(&mut editor, &mut layout, Control::Width(1));
        click(&mut editor, &mut layout, Control::Height(1));
        let grown = layout.cast_bar_size();
        assert!(
            grown.0 > start.0 && grown.1 > start.1,
            "{start:?} -> {grown:?}"
        );
        click(&mut editor, &mut layout, Control::Width(-1));
        click(&mut editor, &mut layout, Control::Height(-1));
        assert_eq!(
            layout.cast_bar_size(),
            start,
            "the steps are exact inverses"
        );
    }

    #[test]
    fn an_anchor_chosen_keeps_the_element_where_it_is() {
        let (mut editor, mut layout) = editor();
        let id = HudElementId::Player;
        select(&mut editor, &mut layout, id);
        let before = layout.rect_of(id, UiScale::default(), FHD);
        assert_eq!(
            click(&mut editor, &mut layout, Control::Anchor(8)),
            EditOutcome::Changed
        );
        assert_eq!(layout.placement(id).anchor, HudAnchor::BottomRight);
        let after = layout.rect_of(id, UiScale::default(), FHD);
        assert!((after.x - before.x).abs() < 0.5 && (after.y - before.y).abs() < 0.5);
    }

    #[test]
    fn done_and_escape_both_leave_edit_mode() {
        let (mut editor, mut layout) = editor();
        assert_eq!(
            click(&mut editor, &mut layout, Control::Done),
            EditOutcome::Closed
        );
        assert!(!editor.is_active());
        editor.open();
        let escape = UiInput {
            cancel: true,
            ..UiInput::new()
        };
        assert_eq!(
            editor.update(&escape, &mut layout, UiScale::default(), FHD),
            EditOutcome::Closed
        );
        assert!(!editor.is_active());
    }

    #[test]
    fn reset_all_puts_the_whole_layout_back_and_reset_puts_back_one_element() {
        let (mut editor, mut layout) = editor();
        let prefs = layout.editor_prefs();
        layout.set_scale(HudElementId::Target, 1.8);
        layout.set_scale(HudElementId::Player, 1.5);
        select(&mut editor, &mut layout, HudElementId::Target);
        click(&mut editor, &mut layout, Control::ResetElement);
        assert!((layout.scale_of(HudElementId::Target) - 1.0).abs() < 1e-6);
        assert!(
            (layout.scale_of(HudElementId::Player) - 1.5).abs() < 1e-6,
            "reset reached another element"
        );

        click(&mut editor, &mut layout, Control::Back);
        assert_eq!(
            click(&mut editor, &mut layout, Control::ResetAll),
            EditOutcome::Changed
        );
        let expected = HudConfig {
            editor: prefs,
            ..HudConfig::default()
        };
        assert_eq!(layout.config(), &expected);
        assert_eq!(editor.selected(), None);
        assert!(editor.is_active(), "reset left edit mode");
    }

    #[test]
    fn a_press_on_the_inspector_does_not_start_a_drag_on_what_is_under_it() {
        let (mut editor, mut layout) = editor();
        let done = editor.control_rect(Control::Done).expect("Done is showing");
        // Put a frame underneath the DONE button.
        layout.move_to(
            HudElementId::Player,
            (done.x - 4.0, done.y - 4.0),
            layout.element_size(HudElementId::Player),
            UiScale::default(),
            FHD,
        );
        let before = layout.rect_of(HudElementId::Player, UiScale::default(), FHD);
        let point = (editor.panel().x + 4.0, editor.panel().y + 4.0);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        assert!(!editor.is_dragging(), "the inspector started a drag");
        assert_eq!(
            layout.rect_of(HudElementId::Player, UiScale::default(), FHD),
            before
        );
    }

    #[test]
    fn a_press_on_empty_space_deselects() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::Party);
        let empty = (0..FHD.0)
            .step_by(17)
            .flat_map(|x| (0..FHD.1).step_by(17).map(move |y| (x as f32, y as f32)))
            .find(|point| {
                HudEditor::element_at(&layout, *point, UiScale::default(), FHD).is_none()
                    && !editor.pointer_over_controls(*point)
            })
            .expect("some part of the screen is empty");
        editor.update(&press(empty), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), None);
    }

    #[test]
    fn a_hidden_element_can_be_found_by_name_and_shown_again() {
        // Otherwise hiding would be a one-way door — the party list starts hidden.
        let mut editor = HudEditor::new();
        editor.open();
        let mut layout = HudLayout::default();
        editor.layout(UiScale::default(), FHD);
        editor.layout_panel(&layout);
        assert!(!layout.is_enabled(HudElementId::Party));
        select(&mut editor, &mut layout, HudElementId::Party);
        click(&mut editor, &mut layout, Control::Visible);
        assert!(layout.is_enabled(HudElementId::Party));
    }

    #[test]
    fn the_inspector_moves_out_of_the_way_of_what_is_selected() {
        let (mut editor, mut layout) = editor();
        // The buff bar lives top right, where the inspector docks.
        layout.move_to(
            HudElementId::Buffs,
            (1700.0, 500.0),
            layout.element_size(HudElementId::Buffs),
            UiScale::default(),
            FHD,
        );
        select(&mut editor, &mut layout, HudElementId::Buffs);
        let buffs = layout.rect_of(HudElementId::Buffs, UiScale::default(), FHD);
        assert!(
            !overlaps(buffs, editor.panel()),
            "{buffs:?} under {:?}",
            editor.panel()
        );
    }

    #[test]
    fn a_drag_snaps_to_the_centre_of_the_screen_when_snapping_is_on() {
        let (mut editor, mut layout) = editor();
        let mut prefs = layout.editor_prefs();
        prefs.snap = true;
        layout.set_editor_prefs(prefs);
        let id = HudElementId::Player;
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        let from = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        editor.update(&press(from), &mut layout, UiScale::default(), FHD);
        // Three pixels off the centre line.
        editor.update(&held((963.0, 400.0)), &mut layout, UiScale::default(), FHD);
        let after = layout.rect_of(id, UiScale::default(), FHD);
        assert!(
            (after.x + after.width * 0.5 - 960.0).abs() < 0.6,
            "it did not snap: {after:?}"
        );
        assert!(editor.guides.0.is_some(), "no guide was shown");
    }

    #[test]
    fn the_nameplate_section_edits_the_enemy_debuff_size_on_its_own() {
        let (mut editor, mut layout) = editor();
        click(
            &mut editor,
            &mut layout,
            Control::Select(EditTarget::EnemyDebuffs),
        );
        let before = editor.nameplates();
        assert_eq!(
            click(&mut editor, &mut layout, Control::PlateDebuffs(1)),
            EditOutcome::NameplatesChanged
        );
        let after = editor.nameplates();
        assert!(after.debuff_size > before.debuff_size);
        assert_eq!(after.size, before.size, "the plate scale moved with it");
        assert_eq!(after.buff_size, before.buff_size);
    }

    #[test]
    fn a_click_on_the_debuffs_over_an_enemy_selects_the_enemy_debuffs_frame() {
        let (mut editor, mut layout) = editor();
        // Somewhere no element is, as a plate over a monster would be.
        let row = UiRect::new(900.0, 300.0, 90.0, 26.0);
        assert!(HudEditor::element_at(&layout, (920.0, 310.0), UiScale::default(), FHD).is_none());
        editor.set_enemy_debuff_rows([row]);
        editor.update(&press((920.0, 310.0)), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.target(), Some(EditTarget::EnemyDebuffs));
        assert!(
            !editor.is_dragging(),
            "a plate's debuffs go where the enemy goes"
        );
        assert!(editor.control_rect(Control::PlateDebuffs(1)).is_some());
        assert!(editor.control_rect(Control::PlateSize(1)).is_none());
    }

    #[test]
    fn the_header_has_no_eye_and_the_position_has_no_arrows() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::Player);
        let visible: Vec<_> = editor
            .controls
            .iter()
            .filter(|(control, _)| *control == Control::Visible)
            .collect();
        assert_eq!(visible.len(), 1, "one show-on-screen switch, no header eye");
    }

    #[test]
    fn editing_allocates_nothing_after_the_first_frame() {
        let (mut editor, mut layout) = editor();
        let capacity = editor.controls.capacity();
        for step in 0..500 {
            let x = 400.0 + (step % 200) as f32;
            editor.update(&at((x, 500.0)), &mut layout, UiScale::default(), FHD);
            editor.update(&press((x, 500.0)), &mut layout, UiScale::default(), FHD);
            editor.update(
                &held((x + 10.0, 510.0)),
                &mut layout,
                UiScale::default(),
                FHD,
            );
            editor.update(
                &release((x + 10.0, 510.0)),
                &mut layout,
                UiScale::default(),
                FHD,
            );
        }
        assert_eq!(editor.controls.capacity(), capacity);
    }

    #[test]
    fn the_editor_draws_in_the_proportional_face_and_nothing_opaque_covers_the_screen() {
        let (mut editor, mut layout) = editor();
        select(&mut editor, &mut layout, HudElementId::Buffs);
        let mut list = UiDrawList::new();
        let mut label = String::new();
        push_hud_editor(
            &mut list,
            &editor,
            &layout,
            &mut label,
            UiScale::default(),
            FHD,
        );
        use engine_render_data::ui::UiCommand;
        assert!(
            !list
                .commands()
                .iter()
                .any(|c| matches!(c, UiCommand::Text { .. })),
            "the editor used the bitmap font"
        );
        for command in list.commands() {
            if let UiCommand::Rect { rect, color } = command {
                let covers = rect.width >= 1900.0 && rect.height >= 1000.0;
                assert!(
                    !(covers && color[3] > 0.2),
                    "an opaque full-screen overlay: {rect:?}"
                );
            }
        }
    }
}
