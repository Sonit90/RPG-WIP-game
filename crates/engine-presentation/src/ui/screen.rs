//! A screen: a list of widgets and the events they produced.
//!
//! ```text
//! UiInput -> UiScreen<T> -> [(T, WidgetEvent)]
//! ```
//!
//! The screen is generic over the tag its owner uses to name widgets, so a menu gets its
//! events back as `(PauseItem::Quit, WidgetEvent::Activated)` rather than as index 1. That
//! is what keeps a screen's layout order and its meaning independent: reordering the buttons
//! cannot silently change what they do.
//!
//! Hover and pressed live here as one rule each:
//!
//! - **hover** is whichever widget the pointer is over; the widgets work it out themselves
//! - **pressed** is whichever widget the pointer went down on
//!
//! There is no focus, because there is no keyboard navigation to need one.

use engine_render_data::ui::UiRect;

use super::input::UiInput;
use super::widget::{Widget, WidgetEvent};

/// A list of tagged widgets.
///
/// Allocates when it is built and never again: the widget vector and the event vector are
/// both reused, so a menu open for a minute allocates nothing after its first frame.
#[derive(Debug, Clone, Default)]
pub struct UiScreen<T> {
    widgets: Vec<(T, Widget)>,
    /// This frame's events. Cleared at the top of every update, never freed.
    events: Vec<(T, WidgetEvent)>,
}

impl<T: Copy + PartialEq> UiScreen<T> {
    /// An empty screen.
    #[must_use]
    pub fn new() -> Self {
        Self {
            widgets: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Adds a widget under `tag`, returning its index.
    ///
    /// The index is the layout order and nothing else; every lookup afterwards is by tag.
    pub fn push(&mut self, tag: T, widget: impl Into<Widget>) -> usize {
        self.widgets.push((tag, widget.into()));
        self.events.reserve(1);
        self.widgets.len() - 1
    }

    /// How many widgets the screen holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.widgets.len()
    }

    /// Whether the screen holds no widgets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.widgets.is_empty()
    }

    /// The widgets, in layout order.
    pub fn widgets(&self) -> impl Iterator<Item = (T, &Widget)> {
        self.widgets.iter().map(|(tag, widget)| (*tag, widget))
    }

    /// The widget tagged `tag`, if there is one.
    #[must_use]
    pub fn widget(&self, tag: T) -> Option<&Widget> {
        self.widgets
            .iter()
            .find(|(candidate, _)| *candidate == tag)
            .map(|(_, widget)| widget)
    }

    /// The widget tagged `tag`, mutably.
    pub fn widget_mut(&mut self, tag: T) -> Option<&mut Widget> {
        self.widgets
            .iter_mut()
            .find(|(candidate, _)| *candidate == tag)
            .map(|(_, widget)| widget)
    }

    /// Positions the widget tagged `tag`.
    pub fn set_rect(&mut self, tag: T, rect: UiRect) {
        if let Some(widget) = self.widget_mut(tag) {
            widget.set_rect(rect);
        }
    }

    /// Which widget the pointer is over, if any.
    #[must_use]
    pub fn hovered(&self) -> Option<T> {
        self.widgets
            .iter()
            .find(|(_, widget)| widget.state().hovered)
            .map(|(tag, _)| *tag)
    }

    /// Which widget the pointer is holding down, if any.
    #[must_use]
    pub fn pressed(&self) -> Option<T> {
        self.widgets
            .iter()
            .find(|(_, widget)| widget.state().pressed)
            .map(|(tag, _)| *tag)
    }

    /// Forgets every hover and press.
    ///
    /// Called when a screen closes: a button left hovered would light up the moment the
    /// screen came back, before the player had moved the mouse.
    pub fn reset(&mut self) {
        for (_, widget) in &mut self.widgets {
            widget.state_mut().clear_interaction();
        }
    }

    /// Applies one frame of input and returns what happened, in layout order.
    pub fn update(&mut self, input: &UiInput) -> &[(T, WidgetEvent)] {
        self.events.clear();
        for index in 0..self.widgets.len() {
            let (tag, widget) = &mut self.widgets[index];
            if let Some(event) = widget.update(input) {
                self.events.push((*tag, event));
            }
        }
        &self.events
    }

    /// What the last update produced.
    #[must_use]
    pub fn events(&self) -> &[(T, WidgetEvent)] {
        &self.events
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::ui::widget::{Button, Checkbox, Slider};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Item {
        Play,
        Volume,
        Fullscreen,
        Quit,
    }

    fn rect(y: f32) -> UiRect {
        UiRect::new(100.0, y, 200.0, 40.0)
    }

    fn screen() -> UiScreen<Item> {
        let mut screen = UiScreen::new();
        screen.push(Item::Play, Button::new("PLAY"));
        screen.push(Item::Volume, Slider::new("VOLUME", 0.0, 1.0, 0.1, 0.5));
        screen.push(Item::Fullscreen, Checkbox::new("FULLSCREEN"));
        screen.push(Item::Quit, Button::new("QUIT"));
        for (index, item) in [Item::Play, Item::Volume, Item::Fullscreen, Item::Quit]
            .into_iter()
            .enumerate()
        {
            screen.set_rect(item, rect(100.0 + index as f32 * 50.0));
        }
        screen
    }

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

    fn release(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_released: true,
            ..at(x, y)
        }
    }

    #[test]
    fn a_screen_reports_which_widget_is_hovered_and_which_is_pressed() {
        let mut screen = screen();
        screen.update(&at(150.0, 110.0));
        assert_eq!(screen.hovered(), Some(Item::Play));
        assert_eq!(screen.pressed(), None);

        screen.update(&press(150.0, 210.0));
        assert_eq!(screen.hovered(), Some(Item::Fullscreen));
        assert_eq!(screen.pressed(), Some(Item::Fullscreen));

        screen.update(&at(0.0, 0.0));
        assert_eq!(screen.hovered(), None, "the pointer left everything");
    }

    #[test]
    fn only_one_widget_is_hovered_at_a_time() {
        let mut screen = screen();
        screen.update(&at(150.0, 110.0));
        let hovered = screen.widgets().filter(|(_, w)| w.state().hovered).count();
        assert_eq!(hovered, 1);
    }

    #[test]
    fn events_come_back_tagged_rather_than_indexed() {
        let mut screen = screen();
        screen.update(&press(150.0, 260.0));
        let events = screen.update(&release(150.0, 260.0));
        assert_eq!(events, [(Item::Quit, WidgetEvent::Activated)]);
    }

    #[test]
    fn resetting_clears_every_hover_and_press() {
        let mut screen = screen();
        screen.update(&press(150.0, 110.0));
        assert!(screen.hovered().is_some() && screen.pressed().is_some());

        screen.reset();
        assert_eq!(screen.hovered(), None);
        assert_eq!(screen.pressed(), None);
    }

    #[test]
    fn a_disabled_widget_takes_no_part_in_anything() {
        let mut screen: UiScreen<Item> = UiScreen::new();
        screen.push(Item::Play, Button::new("PLAY").disabled());
        screen.push(Item::Quit, Button::new("QUIT"));
        screen.set_rect(Item::Play, rect(100.0));
        screen.set_rect(Item::Quit, rect(150.0));

        screen.update(&press(150.0, 110.0));
        assert_eq!(screen.hovered(), None, "a disabled widget lit up");
        assert!(screen.update(&release(150.0, 110.0)).is_empty());

        screen.update(&press(150.0, 160.0));
        assert_eq!(
            screen.update(&release(150.0, 160.0)),
            [(Item::Quit, WidgetEvent::Activated)]
        );
    }

    #[test]
    fn a_screen_reuses_its_event_buffer_rather_than_allocating_per_frame() {
        let mut screen = screen();
        screen.update(&press(150.0, 110.0));
        screen.update(&release(150.0, 110.0));
        let capacity = screen.events.capacity();

        for _ in 0..1_000 {
            screen.update(&press(150.0, 110.0));
            screen.update(&release(150.0, 110.0));
        }
        assert_eq!(screen.events.capacity(), capacity, "the screen reallocated");
        assert_eq!(screen.widgets.capacity(), screen.widgets.capacity());
    }

    #[test]
    fn an_empty_screen_handles_every_input_without_panicking() {
        let mut screen: UiScreen<Item> = UiScreen::new();
        for input in [at(1.0, 1.0), press(1.0, 1.0), release(1.0, 1.0)] {
            assert!(screen.update(&input).is_empty());
        }
        assert!(screen.is_empty());
        assert_eq!(screen.len(), 0);
        assert_eq!(screen.widget(Item::Play), None);
    }
}
