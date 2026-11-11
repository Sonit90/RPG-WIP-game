//! The widgets themselves: state, interaction, and the events they emit.
//!
//! ```text
//! UiInput  ->  Widget::update  ->  Option<WidgetEvent>
//! ```
//!
//! A widget is data plus one update function. It owns its rectangle, its interaction state
//! and — for a checkbox or a slider — its value. It owns nothing else: it never calls into
//! gameplay, never reads a setting, never draws itself. Drawing walks the same data in
//! [`super::paint`], which is what lets every rule below be tested without a device.
//!
//! ## Why events rather than callbacks
//!
//! `button.on_click = || open_settings()` would put a `&mut` game system inside every
//! widget, and the borrow checker would then push the whole menu into `Rc<RefCell<..>>`.
//! Returning [`WidgetEvent`] keeps the widget ignorant of what it means: the screen tags it
//! with its own item enum, and the application decides.

use engine_render_data::ui::UiRect;

use super::input::UiInput;

/// How a widget currently looks and whether it takes input.
///
/// Independent flags rather than one enum: a widget can be hovered *and* pressed at once, and
/// a state machine that had to name every combination would grow with the square of them.
///
/// There is no focus. The menu is driven by the pointer alone - see [`super::input`] for why -
/// so there is nothing for a focus to mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetState {
    /// The pointer is over the widget.
    pub hovered: bool,
    /// The pointer went down on the widget and has not been released.
    pub pressed: bool,
    /// Whether the widget takes input at all.
    pub enabled: bool,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            hovered: false,
            pressed: false,
            enabled: true,
        }
    }
}

impl WidgetState {
    /// Forgets any pointer interaction. Used when a widget is disabled or a screen closes.
    pub fn clear_interaction(&mut self) {
        self.hovered = false;
        self.pressed = false;
    }
}

/// What a widget did this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetEvent {
    /// A button was clicked.
    Activated,
    /// A checkbox changed to this value.
    Toggled(bool),
    /// A slider is being moved and now reads this value. May arrive every frame of a drag.
    ValueChanged(f32),
    /// A slider's interaction finished at this value. The moment worth persisting.
    ValueCommitted(f32),
    /// A dropdown's option at this index was chosen.
    Chosen(usize),
}

/// A labelled button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Button {
    /// What it says. `&'static str` so a menu allocates no strings.
    pub label: &'static str,
    /// Where it is, in client pixels.
    pub rect: UiRect,
    /// How it looks and whether it takes input.
    pub state: WidgetState,
}

impl Button {
    /// A button with the given label, positioned later by the screen's layout.
    #[must_use]
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            rect: UiRect::new(0.0, 0.0, 0.0, 0.0),
            state: WidgetState::default(),
        }
    }

    /// The same button, disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.state.enabled = false;
        self
    }

    /// Applies one frame of input.
    ///
    /// Activation is press *and* release on the widget, the convention every desktop UI
    /// follows: pressing on a button and releasing somewhere else must do nothing, so a
    /// player can change their mind.
    pub fn update(&mut self, input: &UiInput) -> Option<WidgetEvent> {
        if !self.state.enabled {
            self.state.clear_interaction();
            return None;
        }
        let hovered = self.rect.contains(input.pointer.0, input.pointer.1);
        self.state.hovered = hovered;

        if input.pointer_pressed && hovered {
            self.state.pressed = true;
        }

        let clicked = if input.pointer_released {
            let clicked = self.state.pressed && hovered;
            self.state.pressed = false;
            clicked
        } else {
            false
        };

        if clicked {
            return Some(WidgetEvent::Activated);
        }
        None
    }
}

/// A labelled on/off box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkbox {
    /// What it says.
    pub label: &'static str,
    /// The whole interactive row, box and label together.
    pub rect: UiRect,
    /// Whether it is currently checked.
    pub checked: bool,
    /// How it looks and whether it takes input.
    pub state: WidgetState,
}

impl Checkbox {
    /// An unchecked box with the given label.
    #[must_use]
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            rect: UiRect::new(0.0, 0.0, 0.0, 0.0),
            checked: false,
            state: WidgetState::default(),
        }
    }

    /// The same box, starting checked.
    #[must_use]
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// The same box, disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.state.enabled = false;
        self
    }

    /// Sets the value without emitting anything.
    ///
    /// How an owning screen pushes a value in — from settings that were loaded, or reset to
    /// defaults — without the checkbox reporting a change the player did not make.
    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }

    /// Applies one frame of input, toggling when activated.
    pub fn update(&mut self, input: &UiInput) -> Option<WidgetEvent> {
        // The interaction is a button's exactly; only the effect differs.
        let mut probe = Button {
            label: self.label,
            rect: self.rect,
            state: self.state,
        };
        let activated = probe.update(input);
        self.state = probe.state;

        if activated.is_some() {
            self.checked = !self.checked;
            return Some(WidgetEvent::Toggled(self.checked));
        }
        None
    }
}

/// A value between a minimum and a maximum, dragged along a track.
///
/// ```text
/// LABEL
/// 0.10 ------------o--------- 5.00
///                 1.25
/// ```
///
/// The range is part of the widget, not of whatever owns it: a slider that could report a
/// value outside `[min, max]` would push the checking into every caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    /// What the value is called.
    pub label: &'static str,
    /// The track's rectangle. The interactive area, and what the handle slides along.
    pub rect: UiRect,
    /// How it looks and whether it takes input.
    pub state: WidgetState,
    /// Whether the handle is being dragged.
    pub dragging: bool,
    minimum: f32,
    maximum: f32,
    step: f32,
    value: f32,
    /// How many decimal places the value is printed to.
    decimals: usize,
}

impl Slider {
    /// A slider over `[minimum, maximum]`, stepping by `step`, starting at `value`.
    ///
    /// A degenerate range — reversed, zero-width, or not finite — collapses to a slider that
    /// holds `minimum` and cannot move, rather than one that reports nonsense. A
    /// non-positive step means continuous.
    #[must_use]
    pub fn new(label: &'static str, minimum: f32, maximum: f32, step: f32, value: f32) -> Self {
        let (minimum, maximum) = if minimum.is_finite() && maximum.is_finite() && maximum >= minimum
        {
            (minimum, maximum)
        } else {
            (0.0, 0.0)
        };
        let step = if step.is_finite() && step > 0.0 {
            step
        } else {
            0.0
        };
        let mut slider = Self {
            label,
            rect: UiRect::new(0.0, 0.0, 0.0, 0.0),
            state: WidgetState::default(),
            dragging: false,
            minimum,
            maximum,
            step,
            value: minimum,
            decimals: 2,
        };
        slider.set_value(value);
        slider
    }

    /// The same slider, printing its value to `decimals` places.
    ///
    /// Two by default, which is what a sensitivity or a volume wants. A setting whose step is
    /// a whole number prints none: `11.00` for a size of eleven is three characters of noise,
    /// and a number printed to more places than its step can reach is one the player cannot
    /// land on.
    #[must_use]
    pub fn with_decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals.min(4);
        self
    }

    /// How many decimal places its value is printed to.
    #[must_use]
    pub fn decimals(&self) -> usize {
        self.decimals
    }

    /// The same slider, disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.state.enabled = false;
        self
    }

    /// The lowest value this slider offers.
    #[must_use]
    pub fn minimum(&self) -> f32 {
        self.minimum
    }

    /// The highest value this slider offers.
    #[must_use]
    pub fn maximum(&self) -> f32 {
        self.maximum
    }

    /// The granularity. Zero means continuous.
    #[must_use]
    pub fn step(&self) -> f32 {
        self.step
    }

    /// The current value. Always within `[minimum, maximum]`.
    #[must_use]
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Where the value sits in the range, in `0.0..=1.0`. What the handle's position is.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        let span = self.maximum - self.minimum;
        if span <= 0.0 {
            return 0.0;
        }
        ((self.value - self.minimum) / span).clamp(0.0, 1.0)
    }

    /// Sets the value, snapping it to the step and clamping it into the range.
    ///
    /// A value that is not a number leaves the slider where it was: there is no sensible
    /// place to put the handle for a NaN, and moving it somewhere arbitrary would look like
    /// the player did it.
    pub fn set_value(&mut self, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.value = self.snap(value);
    }

    /// `value` snapped onto the step and clamped into the range.
    #[must_use]
    fn snap(&self, value: f32) -> f32 {
        let clamped = value.clamp(self.minimum, self.maximum);
        if self.step <= 0.0 {
            return clamped;
        }
        let steps = ((clamped - self.minimum) / self.step).round();
        (self.minimum + steps * self.step).clamp(self.minimum, self.maximum)
    }

    /// The value a pointer at `x` selects.
    #[must_use]
    fn value_at(&self, x: f32) -> f32 {
        if self.rect.width <= 0.0 {
            return self.value;
        }
        let fraction = ((x - self.rect.x) / self.rect.width).clamp(0.0, 1.0);
        self.snap(self.minimum + fraction * (self.maximum - self.minimum))
    }

    /// Applies one frame of input.
    ///
    /// Clicking the track jumps the handle there and starts a drag, which is what every
    /// slider a player has used does.
    pub fn update(&mut self, input: &UiInput) -> Option<WidgetEvent> {
        if !self.state.enabled {
            self.state.clear_interaction();
            self.dragging = false;
            return None;
        }
        self.state.hovered = self.rect.contains(input.pointer.0, input.pointer.1);

        if input.pointer_pressed && self.state.hovered {
            self.dragging = true;
            self.state.pressed = true;
        }

        if self.dragging {
            if input.pointer_released || !input.pointer_down {
                self.dragging = false;
                self.state.pressed = false;
                // The release carries the final value, whether or not it moved this frame:
                // that is the event worth writing to disk.
                let value = self.value_at(input.pointer.0);
                self.value = value;
                return Some(WidgetEvent::ValueCommitted(value));
            }
            let value = self.value_at(input.pointer.0);
            if value != self.value {
                self.value = value;
                return Some(WidgetEvent::ValueChanged(value));
            }
            return None;
        }
        None
    }
}

/// A closed box showing one choice, which opens into a list of every choice below it.
///
/// The options' *text* is not the widget's: a resolution list is built from the monitor at
/// runtime, and a widget holding strings would stop being `Copy` and stop being a few bytes.
/// The widget holds how many options there are and which is chosen; whoever owns it supplies
/// the words when it is drawn.
///
/// While open it is modal: a click on an option chooses it, a click anywhere else — or
/// escape — closes it without choosing. The screen that owns it routes input to it alone
/// while it is open (see [`Dropdown::is_open`]), because its list is drawn over whatever is
/// below it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dropdown {
    /// What it is for, printed above it.
    pub label: &'static str,
    /// The closed box, in client pixels. The open list hangs below it, one box-height a row.
    pub rect: UiRect,
    /// How it looks and whether it takes input.
    pub state: WidgetState,
    /// The chosen option.
    pub selected: usize,
    /// How many options there are.
    pub len: usize,
    /// Whether the list is showing.
    pub open: bool,
    /// The option under the pointer while the list is showing.
    pub hovered_option: Option<usize>,
    /// Height of one row of the open list. The box's own height unless the layout had to
    /// squeeze the list onto a short screen.
    pub row_height: f32,
    /// Whether the list opens above the box: the layout's choice when there is no room below.
    pub upward: bool,
}

impl Dropdown {
    /// A dropdown of `len` options with `selected` chosen, positioned later by the layout.
    #[must_use]
    pub fn new(label: &'static str, len: usize, selected: usize) -> Self {
        Self {
            label,
            rect: UiRect::new(0.0, 0.0, 0.0, 0.0),
            state: WidgetState::default(),
            selected: selected.min(len.saturating_sub(1)),
            len,
            open: false,
            hovered_option: None,
            row_height: 0.0,
            upward: false,
        }
    }

    /// Sets where the open list goes on a screen `screen_height` tall: below the box if it
    /// fits, above it if that fits better, and with rows squeezed to fit if neither does.
    /// Called by the layout after the box is placed.
    pub fn fit_list(&mut self, screen_height: f32, margin: f32) {
        let rows = self.len.max(1) as f32;
        let below = (screen_height - margin - self.rect.bottom()).max(0.0);
        let above = (self.rect.y - margin).max(0.0);
        let wanted = self.rect.height * rows;
        self.upward = wanted > below && above > below;
        let room = if self.upward { above } else { below };
        self.row_height = (room / rows)
            .min(self.rect.height)
            .max(self.rect.height * 0.5);
    }

    fn row(&self) -> f32 {
        if self.row_height > 0.0 {
            self.row_height
        } else {
            self.rect.height
        }
    }

    /// Replaces the options' count and the choice: the list was rebuilt.
    pub fn set_options(&mut self, len: usize, selected: usize) {
        self.len = len;
        self.selected = selected.min(len.saturating_sub(1));
        self.open = false;
        self.hovered_option = None;
    }

    /// Whether the list is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Closes the list without choosing.
    pub fn close(&mut self) {
        self.open = false;
        self.hovered_option = None;
        self.state.pressed = false;
    }

    /// Where option `index` is drawn while the list is open.
    #[must_use]
    pub fn option_rect(&self, index: usize) -> UiRect {
        let list = self.list_rect();
        UiRect::new(
            list.x,
            list.y + self.row() * index as f32,
            list.width,
            self.row(),
        )
    }

    /// The whole open list.
    #[must_use]
    pub fn list_rect(&self) -> UiRect {
        let height = self.row() * self.len as f32;
        let y = if self.upward {
            self.rect.y - height
        } else {
            self.rect.bottom()
        };
        UiRect::new(self.rect.x, y, self.rect.width, height)
    }

    /// Applies one frame of input.
    ///
    /// A press on the closed box opens it — a press rather than a click, the way every
    /// desktop dropdown answers. While open, a press on an option chooses it and closes the
    /// list; a press anywhere else, or escape, only closes it.
    pub fn update(&mut self, input: &UiInput) -> Option<WidgetEvent> {
        if !self.state.enabled || self.len == 0 {
            self.close();
            self.state.clear_interaction();
            return None;
        }
        let (x, y) = input.pointer;
        self.state.hovered = self.rect.contains(x, y);
        if !self.open {
            self.hovered_option = None;
            if input.pointer_pressed && self.state.hovered {
                self.open = true;
                self.state.pressed = true;
            }
            if input.pointer_released {
                self.state.pressed = false;
            }
            return None;
        }
        self.hovered_option = (0..self.len).find(|index| self.option_rect(*index).contains(x, y));
        if input.cancel {
            self.close();
            return None;
        }
        if input.pointer_released {
            self.state.pressed = false;
        }
        if input.pointer_pressed {
            if let Some(index) = self.hovered_option {
                self.selected = index;
                self.close();
                return Some(WidgetEvent::Chosen(index));
            }
            self.close();
        }
        None
    }
}

/// One of the widget kinds, so a screen can hold a list of them.
///
/// An enum rather than `Box<dyn Widget>`: there are three kinds, they are `Copy`, and a
/// screen's widget list should be one contiguous allocation that never moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Widget {
    /// A button.
    Button(Button),
    /// A checkbox.
    Checkbox(Checkbox),
    /// A slider.
    Slider(Slider),
    /// A dropdown.
    Dropdown(Dropdown),
}

impl Widget {
    /// The widget's interaction state.
    #[must_use]
    pub fn state(&self) -> &WidgetState {
        match self {
            Self::Button(button) => &button.state,
            Self::Checkbox(checkbox) => &checkbox.state,
            Self::Slider(slider) => &slider.state,
            Self::Dropdown(dropdown) => &dropdown.state,
        }
    }

    /// The widget's interaction state, mutably. How a screen assigns focus.
    pub fn state_mut(&mut self) -> &mut WidgetState {
        match self {
            Self::Button(button) => &mut button.state,
            Self::Checkbox(checkbox) => &mut checkbox.state,
            Self::Slider(slider) => &mut slider.state,
            Self::Dropdown(dropdown) => &mut dropdown.state,
        }
    }

    /// Where the widget is.
    #[must_use]
    pub fn rect(&self) -> UiRect {
        match self {
            Self::Button(button) => button.rect,
            Self::Checkbox(checkbox) => checkbox.rect,
            Self::Slider(slider) => slider.rect,
            Self::Dropdown(dropdown) => dropdown.rect,
        }
    }

    /// Moves the widget.
    pub fn set_rect(&mut self, rect: UiRect) {
        match self {
            Self::Button(button) => button.rect = rect,
            Self::Checkbox(checkbox) => checkbox.rect = rect,
            Self::Slider(slider) => slider.rect = rect,
            Self::Dropdown(dropdown) => dropdown.rect = rect,
        }
    }

    /// Whether the widget takes input and can hold the focus.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.state().enabled
    }

    /// Applies one frame of input.
    pub fn update(&mut self, input: &UiInput) -> Option<WidgetEvent> {
        match self {
            Self::Button(button) => button.update(input),
            Self::Checkbox(checkbox) => checkbox.update(input),
            Self::Slider(slider) => slider.update(input),
            Self::Dropdown(dropdown) => dropdown.update(input),
        }
    }
}

impl From<Button> for Widget {
    fn from(button: Button) -> Self {
        Self::Button(button)
    }
}

impl From<Checkbox> for Widget {
    fn from(checkbox: Checkbox) -> Self {
        Self::Checkbox(checkbox)
    }
}

impl From<Slider> for Widget {
    fn from(slider: Slider) -> Self {
        Self::Slider(slider)
    }
}

impl From<Dropdown> for Widget {
    fn from(dropdown: Dropdown) -> Self {
        Self::Dropdown(dropdown)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn placed_dropdown() -> Dropdown {
        let mut dropdown = Dropdown::new("RESOLUTION", 4, 1);
        dropdown.rect = UiRect::new(100.0, 100.0, 300.0, 40.0);
        dropdown.fit_list(1080.0, 10.0);
        dropdown
    }

    #[test]
    fn a_dropdown_opens_on_a_press_and_chooses_the_option_pressed() {
        let mut dropdown = placed_dropdown();
        assert_eq!(dropdown.update(&press(150.0, 120.0)), None);
        assert!(dropdown.is_open());
        let third = dropdown.option_rect(2);
        assert_eq!(third.y, 100.0 + 40.0 * 3.0, "rows hang below the box");
        let hover = UiInput {
            pointer: (third.x + 5.0, third.y + 5.0),
            ..UiInput::default()
        };
        dropdown.update(&hover);
        assert_eq!(dropdown.hovered_option, Some(2));
        assert_eq!(
            dropdown.update(&press(third.x + 5.0, third.y + 5.0)),
            Some(WidgetEvent::Chosen(2))
        );
        assert_eq!(dropdown.selected, 2);
        assert!(!dropdown.is_open());
    }

    #[test]
    fn a_click_elsewhere_or_escape_closes_without_choosing() {
        let mut dropdown = placed_dropdown();
        dropdown.update(&press(150.0, 120.0));
        assert_eq!(dropdown.update(&press(900.0, 900.0)), None);
        assert!(!dropdown.is_open());
        assert_eq!(dropdown.selected, 1);
        dropdown.update(&press(150.0, 120.0));
        let escape = UiInput {
            cancel: true,
            ..UiInput::default()
        };
        assert_eq!(dropdown.update(&escape), None);
        assert!(!dropdown.is_open());
        assert_eq!(dropdown.selected, 1);
    }

    #[test]
    fn a_dropdown_near_the_bottom_opens_upward_and_a_long_one_is_squeezed() {
        let mut dropdown = Dropdown::new("RESOLUTION", 12, 0);
        dropdown.rect = UiRect::new(0.0, 600.0, 300.0, 40.0);
        dropdown.fit_list(720.0, 10.0);
        assert!(dropdown.upward);
        let list = dropdown.list_rect();
        assert!(
            list.y >= 10.0 - 0.01 && list.bottom() <= 600.0 + 0.01,
            "{list:?}"
        );
        let mut disabled = placed_dropdown();
        disabled.state.enabled = false;
        assert_eq!(disabled.update(&press(150.0, 120.0)), None);
        assert!(!disabled.is_open());
    }

    const RECT: UiRect = UiRect {
        x: 100.0,
        y: 100.0,
        width: 200.0,
        height: 40.0,
    };

    /// Input with the pointer somewhere and no buttons or intents.
    fn at(x: f32, y: f32) -> UiInput {
        UiInput::new().with_pointer(x, y)
    }

    fn press(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..at(x, y)
        }
    }

    fn hold(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            ..at(x, y)
        }
    }

    fn release(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_released: true,
            ..at(x, y)
        }
    }

    fn button() -> Button {
        let mut button = Button::new("SETTINGS");
        button.rect = RECT;
        button
    }

    #[test]
    fn a_button_is_normal_hovered_and_pressed_in_turn() {
        let mut button = button();
        assert!(!button.state.hovered && !button.state.pressed);

        assert_eq!(button.update(&at(150.0, 120.0)), None);
        assert!(button.state.hovered, "the pointer is over it");
        assert!(!button.state.pressed);

        assert_eq!(button.update(&press(150.0, 120.0)), None, "not yet");
        assert!(button.state.pressed);

        assert_eq!(
            button.update(&release(150.0, 120.0)),
            Some(WidgetEvent::Activated)
        );
        assert!(!button.state.pressed, "the press outlived the release");
    }

    #[test]
    fn a_button_outside_the_pointer_is_not_hovered() {
        let mut button = button();
        button.update(&at(150.0, 120.0));
        assert!(button.state.hovered);
        button.update(&at(10.0, 10.0));
        assert!(!button.state.hovered);
    }

    #[test]
    fn releasing_away_from_a_button_does_not_activate_it() {
        // Press, change your mind, move off, release: nothing happens.
        let mut button = button();
        button.update(&press(150.0, 120.0));
        button.update(&hold(10.0, 10.0));
        assert_eq!(button.update(&release(10.0, 10.0)), None);
        assert!(!button.state.pressed);
    }

    #[test]
    fn releasing_on_a_button_that_was_never_pressed_does_not_activate_it() {
        // The tail of a drag that began somewhere else must not click whatever it ends on.
        let mut button = button();
        assert_eq!(button.update(&release(150.0, 120.0)), None);
    }

    #[test]
    fn a_disabled_button_ignores_every_input_and_shows_nothing() {
        let mut button = button().disabled();
        assert_eq!(button.update(&press(150.0, 120.0)), None);
        assert_eq!(button.update(&release(150.0, 120.0)), None);
        assert!(!button.state.hovered, "a disabled button lit up");
        assert!(!button.state.pressed);
    }

    #[test]
    fn a_button_answers_the_pointer_and_nothing_else() {
        // No keyboard activation: the keys that would have done it belong to the game, which
        // is still being played behind the menu.
        let mut button = button();
        assert_eq!(button.update(&UiInput::new()), None);
        assert_eq!(
            button.update(&UiInput {
                cancel: true,
                ..UiInput::new()
            }),
            None,
            "escape activated a button"
        );
    }

    #[test]
    fn a_checkbox_toggles_on_each_activation_and_reports_its_new_value() {
        let mut checkbox = Checkbox::new("VSYNC");
        checkbox.rect = RECT;
        assert!(!checkbox.checked);

        checkbox.update(&press(150.0, 120.0));
        assert_eq!(
            checkbox.update(&release(150.0, 120.0)),
            Some(WidgetEvent::Toggled(true))
        );
        assert!(checkbox.checked);

        checkbox.update(&press(150.0, 120.0));
        assert_eq!(
            checkbox.update(&release(150.0, 120.0)),
            Some(WidgetEvent::Toggled(false))
        );
        assert!(!checkbox.checked);
    }

    #[test]
    fn a_checkbox_can_be_set_without_reporting_a_change() {
        // How a screen loads a value the player did not just type.
        let mut checkbox = Checkbox::new("BLOOM").checked(true);
        assert!(checkbox.checked);
        checkbox.set_checked(false);
        assert!(!checkbox.checked);
        assert_eq!(checkbox.update(&at(0.0, 0.0)), None);
    }

    #[test]
    fn a_disabled_checkbox_does_not_toggle() {
        let mut checkbox = Checkbox::new("FULLSCREEN").disabled();
        checkbox.rect = RECT;
        checkbox.update(&press(150.0, 120.0));
        assert_eq!(checkbox.update(&release(150.0, 120.0)), None);
        assert!(!checkbox.checked, "a disabled checkbox toggled");
    }

    fn slider() -> Slider {
        let mut slider = Slider::new("CAMERA SENSITIVITY", 0.1, 5.0, 0.05, 1.0);
        slider.rect = UiRect::new(100.0, 100.0, 200.0, 20.0);
        slider
    }

    #[test]
    fn a_slider_reports_its_range_and_its_starting_value() {
        let slider = slider();
        assert_eq!(slider.minimum(), 0.1);
        assert_eq!(slider.maximum(), 5.0);
        assert_eq!(slider.step(), 0.05);
        assert_eq!(slider.value(), 1.0);
    }

    #[test]
    fn clicking_the_track_moves_the_handle_there() {
        let mut slider = slider();
        // The far left of the track is the minimum.
        assert_eq!(
            slider.update(&press(100.0, 105.0)),
            Some(WidgetEvent::ValueChanged(0.1))
        );
        assert_eq!(slider.value(), slider.minimum());
        assert!(slider.dragging);

        // The far right is the maximum.
        slider.update(&hold(299.0, 105.0));
        assert_eq!(slider.value(), slider.maximum());

        // The middle is the midpoint, snapped to the step.
        slider.update(&hold(200.0, 105.0));
        let midpoint = (slider.minimum() + slider.maximum()) * 0.5;
        assert!(
            (slider.value() - midpoint).abs() <= slider.step(),
            "{}",
            slider.value()
        );
    }

    #[test]
    fn dragging_past_either_end_clamps_rather_than_overshooting() {
        let mut slider = slider();
        slider.update(&press(200.0, 105.0));

        slider.update(&hold(-10_000.0, 105.0));
        assert_eq!(slider.value(), slider.minimum());

        slider.update(&hold(10_000.0, 105.0));
        assert_eq!(slider.value(), slider.maximum());

        // And every value along the way is inside the range.
        for x in (0..400).map(|step| step as f32) {
            slider.update(&hold(x, 105.0));
            assert!(
                (slider.minimum()..=slider.maximum()).contains(&slider.value()),
                "{} at x={x}",
                slider.value()
            );
        }
    }

    #[test]
    fn a_drag_reports_changes_and_a_release_reports_the_committed_value() {
        let mut slider = slider();
        slider.update(&press(150.0, 105.0));
        let during = slider.update(&hold(180.0, 105.0));
        assert!(
            matches!(during, Some(WidgetEvent::ValueChanged(_))),
            "{during:?}"
        );

        let committed = slider.update(&release(180.0, 105.0));
        assert_eq!(committed, Some(WidgetEvent::ValueCommitted(slider.value())));
        assert!(!slider.dragging, "the drag outlived the release");
        assert_eq!(slider.update(&at(180.0, 105.0)), None, "it kept committing");
    }

    #[test]
    fn a_drag_that_does_not_move_reports_nothing_until_it_ends() {
        // Otherwise a held handle writes an event every frame at sixty frames a second.
        let mut slider = slider();
        slider.update(&press(150.0, 105.0));
        for _ in 0..10 {
            assert_eq!(slider.update(&hold(150.0, 105.0)), None);
        }
    }

    #[test]
    fn a_drag_continues_outside_the_track() {
        // The pointer leaves the 20-pixel-tall track constantly while dragging; losing the
        // handle when it does would make the slider unusable.
        let mut slider = slider();
        slider.update(&press(150.0, 105.0));
        slider.update(&hold(250.0, 900.0));
        assert!(slider.dragging);
        assert!(slider.value() > 2.0, "{}", slider.value());
    }

    #[test]
    fn a_pointer_that_never_pressed_the_track_does_not_move_the_handle() {
        let mut slider = slider();
        let before = slider.value();
        slider.update(&hold(280.0, 105.0));
        assert_eq!(slider.value(), before);
        assert!(!slider.dragging);
    }

    #[test]
    fn every_value_a_slider_produces_lies_on_its_step() {
        let mut slider = slider();
        slider.update(&press(150.0, 105.0));
        for x in (100..300).map(|step| step as f32) {
            slider.update(&hold(x, 105.0));
            let steps = (slider.value() - slider.minimum()) / slider.step();
            assert!(
                (steps - steps.round()).abs() < 1e-3,
                "{} is off-step",
                slider.value()
            );
        }
    }

    #[test]
    fn a_slider_only_moves_for_the_pointer() {
        // No keyboard adjustment: the keys it would have claimed belong to the game, which
        // is still being played behind the menu.
        let mut slider = slider();
        let before = slider.value();
        for input in [
            UiInput::new(),
            UiInput {
                cancel: true,
                ..UiInput::new()
            },
        ] {
            assert_eq!(slider.update(&input), None);
            assert_eq!(slider.value(), before);
        }
    }

    #[test]
    fn a_disabled_slider_cannot_be_dragged() {
        let mut slider = slider().disabled();
        let before = slider.value();
        slider.update(&press(299.0, 105.0));
        slider.update(&hold(299.0, 105.0));
        assert_eq!(slider.value(), before);
        assert!(!slider.dragging);
    }

    #[test]
    fn setting_a_value_clamps_snaps_and_refuses_nonsense() {
        let mut slider = slider();
        slider.set_value(-100.0);
        assert_eq!(slider.value(), slider.minimum());
        slider.set_value(1_000.0);
        assert_eq!(slider.value(), slider.maximum());

        slider.set_value(1.23);
        assert!((slider.value() - 1.25).abs() < 1e-6, "{}", slider.value());

        let before = slider.value();
        slider.set_value(f32::NAN);
        assert_eq!(slider.value(), before, "a NaN moved the handle");
        slider.set_value(f32::INFINITY);
        assert_eq!(slider.value(), before);
    }

    #[test]
    fn the_fraction_tracks_the_value_across_the_range() {
        let mut slider = slider();
        slider.set_value(slider.minimum());
        assert_eq!(slider.fraction(), 0.0);
        slider.set_value(slider.maximum());
        assert_eq!(slider.fraction(), 1.0);
        slider.set_value((slider.minimum() + slider.maximum()) * 0.5);
        assert!(
            (slider.fraction() - 0.5).abs() < 0.01,
            "{}",
            slider.fraction()
        );
    }

    #[test]
    fn a_degenerate_range_collapses_instead_of_reporting_nonsense() {
        let reversed = Slider::new("BAD", 5.0, 1.0, 0.1, 3.0);
        assert_eq!(reversed.minimum(), reversed.maximum());
        assert_eq!(reversed.value(), reversed.minimum());
        assert_eq!(reversed.fraction(), 0.0);

        let not_a_number = Slider::new("BAD", f32::NAN, 1.0, 0.1, 0.5);
        assert!(not_a_number.value().is_finite());

        // A zero-width track cannot select anything, and must not divide by it either.
        let mut zero_width = Slider::new("OK", 0.0, 1.0, 0.0, 0.5);
        zero_width.update(&press(0.0, 0.0));
        assert!(zero_width.value().is_finite());
    }

    #[test]
    fn a_continuous_slider_takes_any_value_the_pointer_selects() {
        let mut slider = Slider::new("VOLUME", 0.0, 1.0, 0.0, 0.5);
        slider.rect = UiRect::new(100.0, 100.0, 200.0, 20.0);
        slider.update(&press(100.0 + 200.0 * 0.37, 105.0));
        assert!((slider.value() - 0.37).abs() < 0.01, "{}", slider.value());
    }

    #[test]
    fn the_widget_wrapper_forwards_to_whichever_kind_it_holds() {
        let mut widgets = [
            Widget::from(button()),
            Widget::from(Checkbox::new("VSYNC")),
            Widget::from(slider()),
        ];
        for widget in &mut widgets {
            widget.set_rect(RECT);
            assert_eq!(widget.rect(), RECT);
            assert!(widget.is_enabled());
            widget.state_mut().hovered = true;
            assert!(widget.state().hovered);
        }

        widgets[0].update(&press(150.0, 120.0));
        assert_eq!(
            widgets[0].update(&release(150.0, 120.0)),
            Some(WidgetEvent::Activated)
        );
    }
}
