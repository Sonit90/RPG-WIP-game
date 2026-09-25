//! Edit HUD: the interface made draggable.
//!
//! ```text
//! Playing --EDIT HUD--> Editing --DONE/ESC--> Playing
//!                          |
//!                          +-- drag an element      -> a new normalized offset
//!                          +-- SHOWN                -> enabled / disabled
//!                          +-- SMALLER / BIGGER     -> scale by one step
//!                          +-- HORIZONTAL/VERTICAL  -> a bar turned on its side
//!                          +-- NARROWER / WIDER     -> the cast bar's width, one step
//!                          +-- SHORTER / TALLER     -> the cast bar's height, one step
//! ```
//!
//! ## It is not a panel over the game
//!
//! Every other screen in this interface is a rectangle drawn on top of the world. This one is
//! the HUD itself: the same action bars and the same frames, in the same places, outlined and
//! named and answering the mouse differently. There is nothing to lay out except the control
//! strip, because everything the editor edits is already where the layout put it.
//!
//! ## Nothing it does is gameplay
//!
//! The editor never produces a [`PlayerCommand`](engine_game::command::PlayerCommand), and
//! the application does not run the bar's own click handling while it is active — so a click
//! that moves a bar cannot also cast what is in the slot under the cursor. The simulation
//! itself keeps running, exactly as it does behind the pause menu: this is a multiplayer
//! world and it does not stop because one player is tidying their screen.
//!
//! ## Nothing can be lost
//!
//! Dragging writes through [`HudLayout::move_to`], which clamps, and the clamp runs again on
//! every resolve. An element cannot be pushed off the edge, and one that was already off the
//! edge — a layout arranged on a larger display — comes back on screen the moment it is
//! resolved for the smaller one.

use engine_render_data::ui::{UiDrawList, UiRect, UiScale};

use super::hud_layout::{HudElementId, HudLayout};
use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, Widget, WidgetEvent};

/// Layout constants for the control strip, in logical pixels.
mod layout {
    /// Width of one control button.
    pub const BUTTON_WIDTH: f32 = 132.0;
    /// Height of one control button.
    pub const BUTTON_HEIGHT: f32 = 34.0;
    /// Gap between buttons.
    pub const BUTTON_GAP: f32 = 8.0;
    /// Distance from the top-left corner to the control strip.
    pub const STRIP_MARGIN: f32 = 12.0;
    /// Height of the banner that says the game is in edit mode.
    pub const BANNER_HEIGHT: f32 = 28.0;
}

/// The controls the editor offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditItem {
    /// Shows or hides the selected element.
    Toggle,
    /// One step smaller.
    Smaller,
    /// One step bigger.
    Bigger,
    /// Turns the selected action bar the other way. Disabled for a frame.
    Orientation,
    /// Makes the cast bar one step narrower. Only for an element whose shape can change.
    Narrower,
    /// Makes the cast bar one step wider.
    Wider,
    /// Makes the cast bar one step shorter.
    Shorter,
    /// Makes the cast bar one step taller.
    Taller,
    /// Puts the whole layout back to the default arrangement.
    Reset,
    /// Leaves edit mode.
    Done,
}

impl EditItem {
    /// Every control, in the order the strip lays them out.
    pub const ALL: [Self; 10] = [
        Self::Toggle,
        Self::Smaller,
        Self::Bigger,
        Self::Orientation,
        Self::Narrower,
        Self::Wider,
        Self::Shorter,
        Self::Taller,
        Self::Reset,
        Self::Done,
    ];

    /// The label the button prints.
    ///
    /// `SHOWN`/`HIDDEN` reads as the state the element is in rather than as an instruction,
    /// which is what the player is looking for when they glance at it.
    #[must_use]
    pub fn label(self, enabled: bool) -> &'static str {
        match self {
            Self::Toggle if enabled => "SHOWN",
            Self::Toggle => "HIDDEN",
            Self::Smaller => "SMALLER",
            Self::Bigger => "BIGGER",
            Self::Orientation => "TURN",
            Self::Narrower => "NARROWER",
            Self::Wider => "WIDER",
            Self::Shorter => "SHORTER",
            Self::Taller => "TALLER",
            Self::Reset => "RESET ALL",
            Self::Done => "DONE",
        }
    }
}

/// What the application should do about a frame of editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// Nothing changed.
    Idle,
    /// The layout changed and is worth saving.
    Changed,
    /// Edit mode ended. Save.
    Closed,
}

/// A drag in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    /// Which element is being moved.
    element: HudElementId,
    /// Where inside the element the pointer went down, in pixels.
    ///
    /// Kept so the element does not jump so that its corner is under the cursor the moment
    /// the drag starts — the single thing that makes dragging feel wrong.
    grab: (f32, f32),
    /// Whether the layout has actually changed since the press.
    moved: bool,
}

/// Edit mode: what is selected, what is being dragged, and the controls.
#[derive(Debug)]
pub struct HudEditor {
    active: bool,
    selected: Option<HudElementId>,
    drag: Option<Drag>,
    controls: UiScreen<EditItem>,
    scale: UiScale,
    viewport: (u32, u32),
}

impl Default for HudEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl HudEditor {
    /// A closed editor with its controls built.
    #[must_use]
    pub fn new() -> Self {
        let mut controls = UiScreen::new();
        for item in EditItem::ALL {
            controls.push(item, Button::new(item.label(true)));
        }
        Self {
            active: false,
            selected: None,
            drag: None,
            controls,
            scale: UiScale::default(),
            viewport: (0, 0),
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
        self.selected
    }

    /// Whether an element is being dragged right now.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The controls, for drawing.
    #[must_use]
    pub fn controls(&self) -> &UiScreen<EditItem> {
        &self.controls
    }

    /// The interface size the strip is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.scale
    }

    /// Enters edit mode.
    pub fn open(&mut self) {
        self.active = true;
        self.selected = None;
        self.drag = None;
        self.controls.reset();
    }

    /// Leaves edit mode, ending any drag in progress.
    ///
    /// A drag that was in progress keeps whatever it had already written: the layout was
    /// being updated as the pointer moved, so there is nothing to roll back and nothing to
    /// commit — which is also why leaving cannot lose a move.
    pub fn close(&mut self) {
        self.active = false;
        self.selected = None;
        self.drag = None;
        self.controls.reset();
    }

    /// Lays the control strip out for a viewport and an interface size.
    pub fn layout(&mut self, scale: UiScale, viewport: (u32, u32)) {
        if scale == self.scale && viewport == self.viewport {
            return;
        }
        self.scale = scale;
        self.viewport = viewport;
        let width = scale.px(layout::BUTTON_WIDTH);
        let height = scale.px(layout::BUTTON_HEIGHT);
        let gap = scale.px(layout::BUTTON_GAP);
        let margin = scale.px(layout::STRIP_MARGIN);
        // A column in the top-left corner rather than a strip across the top. The default
        // layout puts the target and target-of-target frames along the top edge, and a strip
        // there would sit on top of two of the things the editor exists to move.
        let top = margin + scale.px(layout::BANNER_HEIGHT);
        for (index, item) in EditItem::ALL.into_iter().enumerate() {
            self.controls.set_rect(
                item,
                UiRect::new(margin, top + index as f32 * (height + gap), width, height),
            );
        }
    }

    /// The banner that says the game is in edit mode, above the controls.
    ///
    /// Decoration: it is not part of [`HudEditor::pointer_over_controls`], so it takes no
    /// clicks and covers nothing the player might want to drag.
    #[must_use]
    pub fn banner(&self) -> UiRect {
        let margin = self.scale.px(layout::STRIP_MARGIN);
        UiRect::new(
            margin,
            margin,
            self.scale.px(layout::BUTTON_WIDTH),
            self.scale.px(layout::BANNER_HEIGHT),
        )
    }

    /// Applies one frame of input to `layout`.
    ///
    /// The control strip is read first: it is drawn on top of the HUD, so a click that lands
    /// on a button is a button press and not the start of a drag on whatever is underneath.
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
        self.refresh_controls(layout);

        if input.cancel {
            // Escape leaves edit mode, exactly as DONE does. The key that opens a thing is
            // the key that closes it, everywhere in this interface.
            self.close();
            return EditOutcome::Closed;
        }

        match self.update_controls(input, layout) {
            EditOutcome::Idle => {}
            outcome => return outcome,
        }
        if self.pointer_over_controls(input.pointer) {
            // The strip is drawn over the HUD, so a press on it is a button press and never
            // the start of a drag on whatever the strip happens to be covering. Checked as
            // well as reading the buttons first, because a *disabled* button consumes
            // nothing — and without this, clicking a greyed-out control would pick up the
            // element underneath and change what the strip is acting on mid-click.
            return EditOutcome::Idle;
        }
        self.update_drag(input, layout, scale, viewport)
    }

    /// Puts the control strip into the state the selected element is actually in.
    fn refresh_controls(&mut self, layout: &HudLayout) {
        let selected = self.selected;
        let enabled = selected.is_some_and(|id| layout.is_enabled(id));
        for item in EditItem::ALL {
            let usable = match item {
                // Always available: leaving, and starting over.
                EditItem::Done | EditItem::Reset => true,
                // A bar can be turned; a frame has no orientation to turn.
                EditItem::Orientation => selected.is_some_and(|id| id.bar_index().is_some()),
                // Only the cast bar has a width and a height of its own.
                EditItem::Narrower | EditItem::Wider | EditItem::Shorter | EditItem::Taller => {
                    selected.is_some_and(HudElementId::is_resizable)
                }
                // The rest need something selected to act on.
                _ => selected.is_some(),
            };
            if let Some(Widget::Button(button)) = self.controls.widget_mut(item) {
                button.state.enabled = usable;
                button.label = item.label(enabled);
            }
        }
    }

    /// Reads the control strip. Returns what the button that was pressed means.
    fn update_controls(&mut self, input: &UiInput, layout: &mut HudLayout) -> EditOutcome {
        let mut pressed = None;
        for (item, event) in self.controls.update(input) {
            if *event == WidgetEvent::Activated {
                pressed = Some(*item);
            }
        }
        let Some(item) = pressed else {
            return EditOutcome::Idle;
        };
        match item {
            EditItem::Done => {
                self.close();
                return EditOutcome::Closed;
            }
            EditItem::Reset => {
                layout.set_config(engine_settings::hud::HudConfig::default());
                self.selected = None;
                return EditOutcome::Changed;
            }
            _ => {}
        }
        let Some(selected) = self.selected else {
            return EditOutcome::Idle;
        };
        match item {
            EditItem::Toggle => {
                layout.toggle(selected);
            }
            EditItem::Smaller => layout.nudge_scale(selected, -1),
            EditItem::Bigger => layout.nudge_scale(selected, 1),
            EditItem::Orientation => {
                let Some(index) = selected.bar_index() else {
                    return EditOutcome::Idle;
                };
                layout.flip_orientation(index);
            }
            EditItem::Narrower | EditItem::Wider | EditItem::Shorter | EditItem::Taller => {
                if !selected.is_resizable() {
                    return EditOutcome::Idle;
                }
                let (width, height) = match item {
                    EditItem::Narrower => (-1, 0),
                    EditItem::Wider => (1, 0),
                    EditItem::Shorter => (0, -1),
                    _ => (0, 1),
                };
                layout.nudge_cast_bar(width, height, self.scale, self.viewport);
            }
            EditItem::Done | EditItem::Reset => unreachable!("handled above"),
        }
        EditOutcome::Changed
    }

    /// Selects, drags and drops.
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
            // picked up next time — which is the only ordering a player can predict.
            if let Some(element) = Self::element_at(layout, (x, y), scale, viewport) {
                let rect = layout.rect_of(element, scale, viewport);
                self.selected = Some(element);
                self.drag = Some(Drag {
                    element,
                    grab: (x - rect.x, y - rect.y),
                    moved: false,
                });
            } else {
                // A press on empty space deselects, so the controls stop acting on something
                // the player has stopped looking at.
                self.selected = None;
            }
            return EditOutcome::Idle;
        }

        let Some(drag) = self.drag.as_mut() else {
            return EditOutcome::Idle;
        };

        if input.pointer_released {
            let changed = drag.moved;
            self.drag = None;
            return if changed {
                EditOutcome::Changed
            } else {
                EditOutcome::Idle
            };
        }

        if !input.pointer_down {
            // The button came up while the pointer was somewhere this interface never saw.
            self.drag = None;
            return EditOutcome::Idle;
        }

        let element = drag.element;
        let target = (x - drag.grab.0, y - drag.grab.1);
        let before = layout.rect_of(element, scale, viewport);
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

    /// Whether a point is on the control strip or its banner.
    #[must_use]
    pub fn pointer_over_controls(&self, pointer: (f32, f32)) -> bool {
        let (x, y) = pointer;
        self.controls
            .widgets()
            .any(|(_, widget)| widget.rect().contains(x, y))
    }

    /// Which element is under a point, topmost first.
    ///
    /// Hidden elements are still pickable: an element the player disabled has to be
    /// selectable again or `SHOWN` could never be pressed a second time. This is the one
    /// place a disabled element is interactive, and only while the editor is open.
    #[must_use]
    pub fn element_at(
        layout: &HudLayout,
        point: (f32, f32),
        scale: UiScale,
        viewport: (u32, u32),
    ) -> Option<HudElementId> {
        HudElementId::all()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .find(|id| {
                layout
                    .rect_of(*id, scale, viewport)
                    .contains(point.0, point.1)
            })
    }
}

/// Draws edit mode over the HUD: a banner, an outline and a name for every element, and the
/// control strip.
///
/// Outlines rather than filled boxes: the player is arranging the interface they will play
/// with, so what they are moving has to look like what it will look like. An element that
/// turned into a grey rectangle while being placed would be placed by guesswork.
pub fn push_hud_editor(
    list: &mut UiDrawList,
    editor: &HudEditor,
    layout: &HudLayout,
    theme: &super::paint::MenuTheme,
    label: &mut String,
    scale: UiScale,
    viewport: (u32, u32),
) {
    if !editor.is_active() {
        return;
    }
    let text = scale.text(theme.label_scale);
    let border = scale.px(theme.border_pixels);

    for id in HudElementId::all() {
        let rect = layout.rect_of(id, scale, viewport);
        let selected = editor.selected() == Some(id);
        let enabled = layout.is_enabled(id);
        // A hidden element is drawn faintly rather than not at all: it has to be findable to
        // be brought back, and this is the only place it is.
        if !enabled {
            list.rect(rect, theme.button_disabled);
        }
        let colour = if selected {
            theme.title
        } else if enabled {
            theme.panel_border
        } else {
            theme.separator
        };
        list.outline(rect, if selected { border * 2.0 } else { border }, colour);

        id.write_label(label);
        if !enabled {
            label.push_str(" (HIDDEN)");
        }
        list.text_at(
            (rect.x + border * 2.0, rect.y + border * 2.0),
            label,
            colour,
            text,
        );
    }

    let banner = editor.banner();
    list.rect(banner, theme.panel);
    list.outline(banner, border, theme.panel_border);
    list.text_at(
        (banner.x + border * 2.0, banner.y + border * 2.0),
        "EDIT HUD",
        theme.title,
        text,
    );
    for (_, widget) in editor.controls().widgets() {
        super::paint::push_widget(list, widget, theme, scale);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_settings::hud::{HUD_SCALE_STEP, HudConfig, HudOrientation};

    const FHD: (u32, u32) = (1920, 1080);

    fn editor() -> (HudEditor, HudLayout) {
        let mut editor = HudEditor::new();
        editor.open();
        let mut layout = HudLayout::default();
        // Everything on, so every element can be picked up in these tests.
        for id in HudElementId::all() {
            layout.set_enabled(id, true);
        }
        editor.layout(UiScale::default(), FHD);
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

    /// The centre of an element as it is currently drawn.
    fn centre(layout: &HudLayout, id: HudElementId) -> (f32, f32) {
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// Drags `id` from wherever it is to `target`, the way a player would.
    fn drag_to(
        editor: &mut HudEditor,
        layout: &mut HudLayout,
        id: HudElementId,
        target: (f32, f32),
    ) -> EditOutcome {
        let from = centre(layout, id);
        editor.update(&press(from), layout, UiScale::default(), FHD);
        let moved = editor.update(&held(target), layout, UiScale::default(), FHD);
        editor.update(&release(target), layout, UiScale::default(), FHD);
        moved
    }

    /// A control button's rectangle.
    fn control(editor: &HudEditor, item: EditItem) -> UiRect {
        editor
            .controls()
            .widget(item)
            .expect("the control exists")
            .rect()
    }

    /// Clicks a control button.
    fn click(editor: &mut HudEditor, layout: &mut HudLayout, item: EditItem) -> EditOutcome {
        let rect = control(editor, item);
        let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        editor.update(&press(centre), layout, UiScale::default(), FHD);
        editor.update(&release(centre), layout, UiScale::default(), FHD)
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
        // The brief's list, in full: eight bars, and all four frames.
        for id in HudElementId::all() {
            let (mut editor, mut layout) = editor();
            let before = layout.rect_of(id, UiScale::default(), FHD);
            let target = (620.0, 520.0);
            assert_eq!(
                drag_to(&mut editor, &mut layout, id, target),
                EditOutcome::Changed,
                "{id:?} did not move"
            );
            assert_eq!(editor.selected(), Some(id), "{id:?} was not selected");
            let after = layout.rect_of(id, UiScale::default(), FHD);
            assert_ne!(before, after, "{id:?} stayed where it was");
            // It landed under the cursor, keeping the grab offset.
            let centre = (after.x + after.width * 0.5, after.y + after.height * 0.5);
            assert!(
                (centre.0 - target.0).abs() < 1.0 && (centre.1 - target.1).abs() < 1.0,
                "{id:?} landed at {centre:?} rather than {target:?}"
            );
        }
    }

    #[test]
    fn an_element_does_not_jump_to_the_cursor_when_the_drag_starts() {
        let (mut editor, mut layout) = editor();
        let id = HudElementId::Player;
        let rect = layout.rect_of(id, UiScale::default(), FHD);
        // Press near the element's corner rather than its centre.
        let grab = (rect.x + 4.0, rect.y + 4.0);
        editor.update(&press(grab), &mut layout, UiScale::default(), FHD);
        let after = layout.rect_of(id, UiScale::default(), FHD);
        assert_eq!(after, rect, "the press alone moved it");

        // A one-pixel move moves it one pixel, not to wherever the cursor is.
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
        let point = centre(&layout, HudElementId::Party);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        assert_eq!(
            editor.update(&release(point), &mut layout, UiScale::default(), FHD),
            EditOutcome::Idle,
            "a click was reported as a change"
        );
        assert_eq!(layout, before);
        assert_eq!(editor.selected(), Some(HudElementId::Party));
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
                drag_to(&mut editor, &mut layout, id, target);
                let rect = layout.rect_of(id, UiScale::default(), FHD);
                let visible_x = (rect.right().min(1920.0) - rect.x.max(0.0)).max(0.0);
                let visible_y = (rect.bottom().min(1080.0) - rect.y.max(0.0)).max(0.0);
                assert!(
                    visible_x > 1.0 && visible_y > 1.0,
                    "{id:?} left the screen: {rect:?}"
                );
                // And it can be picked up again, which is the point of the clamp.
                assert_eq!(
                    HudEditor::element_at(
                        &layout,
                        (
                            rect.x.max(0.0) + visible_x * 0.5,
                            rect.y.max(0.0) + visible_y * 0.5
                        ),
                        UiScale::default(),
                        FHD,
                    ),
                    Some(id),
                    "{id:?} cannot be picked up at {rect:?}"
                );
            }
        }
    }

    #[test]
    fn the_controls_act_on_the_selected_element_and_nothing_else() {
        let (mut editor, mut layout) = editor();
        let id = HudElementId::Target;
        let other = HudElementId::Player;
        let point = centre(&layout, id);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        editor.update(&release(point), &mut layout, UiScale::default(), FHD);

        let before_other = layout.placement(other);
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Bigger),
            EditOutcome::Changed
        );
        assert!((layout.scale_of(id) - (1.0 + HUD_SCALE_STEP)).abs() < 1e-5);
        assert_eq!(
            layout.placement(other),
            before_other,
            "the wrong element changed"
        );

        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Smaller),
            EditOutcome::Changed
        );
        assert!((layout.scale_of(id) - 1.0).abs() < 1e-5);

        assert!(layout.is_enabled(id));
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Toggle),
            EditOutcome::Changed
        );
        assert!(!layout.is_enabled(id), "the element was not hidden");
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Toggle),
            EditOutcome::Changed
        );
        assert!(
            layout.is_enabled(id),
            "the element could not be shown again"
        );
    }

    #[test]
    fn only_a_bar_can_be_turned() {
        let (mut editor, mut layout) = editor();
        let bar = HudElementId::ActionBar(0);
        let point = centre(&layout, bar);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        editor.update(&release(point), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), Some(bar));
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Orientation),
            EditOutcome::Changed
        );
        assert_eq!(layout.orientation(0), HudOrientation::Vertical);

        // With a frame selected the control is disabled, and a disabled widget emits nothing.
        let frame = centre(&layout, HudElementId::Party);
        editor.update(&press(frame), &mut layout, UiScale::default(), FHD);
        editor.update(&release(frame), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), Some(HudElementId::Party));
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Orientation),
            EditOutcome::Idle,
            "a frame was turned"
        );
    }

    #[test]
    fn a_control_with_nothing_selected_does_nothing() {
        let (mut editor, mut layout) = editor();
        assert_eq!(editor.selected(), None);
        let before = layout.clone();
        for item in [EditItem::Toggle, EditItem::Bigger, EditItem::Smaller] {
            assert_eq!(click(&mut editor, &mut layout, item), EditOutcome::Idle);
        }
        assert_eq!(layout, before);
    }

    #[test]
    fn done_and_escape_both_leave_edit_mode() {
        let (mut editor, mut layout) = editor();
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Done),
            EditOutcome::Closed
        );
        assert!(!editor.is_active());

        editor.open();
        assert!(editor.is_active());
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
    fn reset_puts_the_whole_layout_back() {
        let (mut editor, mut layout) = editor();
        drag_to(
            &mut editor,
            &mut layout,
            HudElementId::Player,
            (900.0, 200.0),
        );
        layout.set_scale(HudElementId::Target, 1.8);
        assert_ne!(layout.config(), &HudConfig::default());

        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Reset),
            EditOutcome::Changed
        );
        assert_eq!(layout.config(), &HudConfig::default());
        assert_eq!(editor.selected(), None);
        assert!(editor.is_active(), "reset left edit mode");
    }

    #[test]
    fn a_press_on_a_control_does_not_start_a_drag_on_what_is_under_it() {
        // The control strip is drawn on top of the HUD, so it has to be read first.
        let mut editor = HudEditor::new();
        editor.open();
        let mut layout = HudLayout::default();
        editor.layout(UiScale::default(), FHD);
        // Put a frame underneath the DONE button.
        let done = control(&editor, EditItem::Done);
        layout.move_to(
            HudElementId::Player,
            (done.x - 4.0, done.y - 4.0),
            layout.element_size(HudElementId::Player),
            UiScale::default(),
            FHD,
        );
        let before = layout.rect_of(HudElementId::Player, UiScale::default(), FHD);
        assert!(
            before.contains(done.x + done.width * 0.5, done.y + done.height * 0.5),
            "the test did not put the frame under the button"
        );

        let centre = (done.x + done.width * 0.5, done.y + done.height * 0.5);
        editor.update(&press(centre), &mut layout, UiScale::default(), FHD);
        assert!(!editor.is_dragging(), "the button started a drag");
        assert_eq!(
            layout.rect_of(HudElementId::Player, UiScale::default(), FHD),
            before
        );
    }

    #[test]
    fn a_press_on_empty_space_deselects() {
        let (mut editor, mut layout) = editor();
        let point = centre(&layout, HudElementId::Party);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        editor.update(&release(point), &mut layout, UiScale::default(), FHD);
        assert!(editor.selected().is_some());

        // Somewhere nothing is. Found rather than guessed: eight bars and four frames are
        // enabled here, and a hard-coded point would silently stop being empty the next time
        // a default moved.
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
    fn a_hidden_element_can_still_be_selected_and_shown_again() {
        // Otherwise `HIDDEN` would be a one-way door.
        let (mut editor, mut layout) = editor();
        let id = HudElementId::ActionBar(3);
        layout.set_enabled(id, false);
        let point = centre(&layout, id);
        editor.update(&press(point), &mut layout, UiScale::default(), FHD);
        editor.update(&release(point), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), Some(id));
        click(&mut editor, &mut layout, EditItem::Toggle);
        assert!(layout.is_enabled(id));
    }

    #[test]
    fn the_control_strip_follows_the_interface_size() {
        let mut editor = HudEditor::new();
        editor.open();
        editor.layout(UiScale::default(), FHD);
        let small = control(&editor, EditItem::Done);
        editor.layout(UiScale::new(2.0), FHD);
        let large = control(&editor, EditItem::Done);
        assert!(large.width > small.width);
        assert!(
            large.right() <= FHD.0 as f32 + 0.01,
            "the strip ran off the screen: {large:?}"
        );
    }

    #[test]
    fn editing_allocates_nothing_after_the_first_frame() {
        let (mut editor, mut layout) = editor();
        for step in 0..1000 {
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
        // The controls are built once; nothing here grows them.
        assert_eq!(editor.controls().len(), EditItem::ALL.len());
    }

    #[test]
    fn the_cast_bar_can_be_moved_and_made_wider_taller_narrower_and_shorter() {
        let (mut editor, mut layout) = editor();
        let bar = centre(&layout, HudElementId::CastBar);
        editor.update(&press(bar), &mut layout, UiScale::default(), FHD);
        editor.update(&release(bar), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), Some(HudElementId::CastBar));

        let start = layout.cast_bar_size();
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Wider),
            EditOutcome::Changed
        );
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Taller),
            EditOutcome::Changed
        );
        let grown = layout.cast_bar_size();
        assert!(
            grown.0 > start.0 && grown.1 > start.1,
            "{start:?} -> {grown:?}"
        );
        click(&mut editor, &mut layout, EditItem::Narrower);
        click(&mut editor, &mut layout, EditItem::Shorter);
        assert_eq!(
            layout.cast_bar_size(),
            start,
            "the steps are exact inverses"
        );

        // And it moves like everything else.
        let before = layout.rect_of(HudElementId::CastBar, UiScale::default(), FHD);
        let point = centre(&layout, HudElementId::CastBar);
        drag_to(
            &mut editor,
            &mut layout,
            HudElementId::CastBar,
            (point.0 - 300.0, point.1 - 200.0),
        );
        let after = layout.rect_of(HudElementId::CastBar, UiScale::default(), FHD);
        assert!((after.x - before.x).abs() > 100.0 && (after.y - before.y).abs() > 100.0);
    }

    #[test]
    fn only_the_cast_bar_can_be_reshaped() {
        let (mut editor, mut layout) = editor();
        let frame = centre(&layout, HudElementId::Player);
        editor.update(&press(frame), &mut layout, UiScale::default(), FHD);
        editor.update(&release(frame), &mut layout, UiScale::default(), FHD);
        assert_eq!(editor.selected(), Some(HudElementId::Player));
        let before = layout.config().clone();
        assert_eq!(
            click(&mut editor, &mut layout, EditItem::Wider),
            EditOutcome::Idle
        );
        assert_eq!(layout.config(), &before);
    }
}
