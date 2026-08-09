//! A screen: a list of widgets, a focus, and the events they produced.
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
//! Focus, hover and pressed live here as one rule each:
//!
//! - **hover** is whichever widget the pointer is over; the widgets work it out themselves
//! - **pressed** is whichever widget the pointer went down on
//! - **focus** is exactly one widget, moved by [`Nav::Prev`]/[`Nav::Next`], and also moved
//!   by the pointer so that clicking something and then pressing an arrow key continues
//!   from where the player was looking

use engine_render_data::ui::UiRect;

use super::input::{Nav, UiInput};
use super::widget::{Widget, WidgetEvent};

/// A list of tagged widgets with a focus.
///
/// Allocates when it is built and never again: the widget vector and the event vector are
/// both reused, so a menu open for a minute allocates nothing after its first frame.
#[derive(Debug, Clone, Default)]
pub struct UiScreen<T> {
    widgets: Vec<(T, Widget)>,
    /// Index of the focused widget, if any.
    focus: Option<usize>,
    /// This frame's events. Cleared at the top of every update, never freed.
    events: Vec<(T, WidgetEvent)>,
}

impl<T: Copy + PartialEq> UiScreen<T> {
    /// An empty screen.
    #[must_use]
    pub fn new() -> Self {
        Self {
            widgets: Vec::new(),
            focus: None,
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

    /// Which widget has the focus, if any.
    #[must_use]
    pub fn focused(&self) -> Option<T> {
        self.focus.map(|index| self.widgets[index].0)
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

    /// Puts the focus on `tag`, if it names an enabled widget.
    pub fn focus(&mut self, tag: T) {
        let index = self
            .widgets
            .iter()
            .position(|(candidate, widget)| *candidate == tag && widget.is_enabled());
        if index.is_some() {
            self.set_focus(index);
        }
    }

    /// Puts the focus on the first enabled widget, or nowhere if there is none.
    ///
    /// Called when a screen opens, so a player who never touches the mouse has somewhere to
    /// start.
    pub fn focus_first(&mut self) {
        let index = self.widgets.iter().position(|(_, w)| w.is_enabled());
        self.set_focus(index);
    }

    /// Drops the focus and forgets every hover and press.
    ///
    /// Called when a screen closes: a button left hovered would light up the moment the
    /// screen came back, before the player had moved the mouse.
    pub fn reset(&mut self) {
        self.set_focus(None);
        for (_, widget) in &mut self.widgets {
            widget.state_mut().clear_interaction();
        }
    }

    /// Applies one frame of input and returns what happened, in layout order.
    ///
    /// The focus moves *before* the widgets update, so a `Tab` that lands on a button and an
    /// `Enter` in the same frame cannot both apply to it — one intent, one effect.
    pub fn update(&mut self, input: &UiInput) -> &[(T, WidgetEvent)] {
        self.events.clear();

        match input.nav {
            Some(Nav::Next) => self.move_focus(1),
            Some(Nav::Prev) => self.move_focus(-1),
            _ => {}
        }

        // A press moves the focus to whatever was pressed, so keyboard and mouse agree
        // about where the player is.
        if input.pointer_pressed {
            let hit = self.widgets.iter().position(|(_, widget)| {
                widget.is_enabled() && widget.rect().contains(input.pointer.0, input.pointer.1)
            });
            if hit.is_some() {
                self.set_focus(hit);
            }
        }

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

    /// Moves the focus `delta` enabled widgets along, wrapping at both ends.
    ///
    /// Wrapping rather than stopping: a menu is a short ring, and a player holding `Tab`
    /// expects to come back round rather than to get stuck on the last item.
    fn move_focus(&mut self, delta: isize) {
        let count = self.widgets.len();
        if count == 0 {
            return;
        }
        let start = match self.focus {
            Some(index) => index as isize,
            // Coming from nowhere, `Next` should land on the first widget rather than the
            // second, so start one before it.
            None => {
                if delta > 0 {
                    -1
                } else {
                    0
                }
            }
        };
        for offset in 1..=count as isize {
            let index = (start + delta * offset).rem_euclid(count as isize) as usize;
            if self.widgets[index].1.is_enabled() {
                self.set_focus(Some(index));
                return;
            }
        }
    }

    /// Sets the focus flag on exactly one widget.
    fn set_focus(&mut self, index: Option<usize>) {
        self.focus = index;
        for (position, (_, widget)) in self.widgets.iter_mut().enumerate() {
            widget.state_mut().focused = Some(position) == index;
        }
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

    fn nav(nav: Nav) -> UiInput {
        UiInput {
            nav: Some(nav),
            ..UiInput::new()
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
    fn navigation_moves_the_focus_and_wraps_around() {
        let mut screen = screen();
        assert_eq!(screen.focused(), None);

        screen.update(&nav(Nav::Next));
        assert_eq!(
            screen.focused(),
            Some(Item::Play),
            "next started at the second"
        );
        screen.update(&nav(Nav::Next));
        assert_eq!(screen.focused(), Some(Item::Volume));

        for _ in 0..2 {
            screen.update(&nav(Nav::Next));
        }
        assert_eq!(screen.focused(), Some(Item::Quit));
        screen.update(&nav(Nav::Next));
        assert_eq!(screen.focused(), Some(Item::Play), "the focus did not wrap");

        screen.update(&nav(Nav::Prev));
        assert_eq!(screen.focused(), Some(Item::Quit), "backwards did not wrap");
    }

    #[test]
    fn exactly_one_widget_carries_the_focus_flag() {
        let mut screen = screen();
        screen.update(&nav(Nav::Next));
        screen.update(&nav(Nav::Next));
        let focused: Vec<Item> = screen
            .widgets()
            .filter(|(_, widget)| widget.state().focused)
            .map(|(tag, _)| tag)
            .collect();
        assert_eq!(focused, [Item::Volume]);
    }

    #[test]
    fn the_focus_skips_disabled_widgets() {
        let mut screen = UiScreen::new();
        screen.push(Item::Play, Button::new("PLAY").disabled());
        screen.push(Item::Quit, Button::new("QUIT"));

        screen.update(&nav(Nav::Next));
        assert_eq!(screen.focused(), Some(Item::Quit));
        screen.update(&nav(Nav::Next));
        assert_eq!(
            screen.focused(),
            Some(Item::Quit),
            "a disabled widget took focus"
        );

        screen.focus(Item::Play);
        assert_eq!(
            screen.focused(),
            Some(Item::Quit),
            "focus() ignored disabled"
        );
    }

    #[test]
    fn a_screen_with_nothing_enabled_simply_has_no_focus() {
        let mut screen: UiScreen<Item> = UiScreen::new();
        screen.push(Item::Play, Button::new("PLAY").disabled());
        screen.focus_first();
        assert_eq!(screen.focused(), None);
        screen.update(&nav(Nav::Next));
        assert_eq!(screen.focused(), None);
    }

    #[test]
    fn clicking_a_widget_moves_the_focus_to_it() {
        // So that clicking a slider and then pressing an arrow key adjusts *that* slider.
        let mut screen = screen();
        screen.focus_first();
        assert_eq!(screen.focused(), Some(Item::Play));

        screen.update(&press(150.0, 160.0));
        assert_eq!(screen.focused(), Some(Item::Volume));

        // Clicking the background leaves the focus where it was rather than dropping it.
        screen.update(&press(5.0, 5.0));
        assert_eq!(screen.focused(), Some(Item::Volume));
    }

    #[test]
    fn keyboard_activation_applies_to_the_focused_widget_only() {
        let mut screen = screen();
        screen.focus(Item::Quit);
        let events = screen.update(&nav(Nav::Activate));
        assert_eq!(events, [(Item::Quit, WidgetEvent::Activated)]);
    }

    #[test]
    fn arrow_keys_adjust_the_focused_slider_and_nothing_else() {
        let mut screen = screen();
        screen.focus(Item::Volume);
        let events = screen.update(&nav(Nav::Increase)).to_vec();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, Item::Volume);
        assert!(matches!(events[0].1, WidgetEvent::ValueCommitted(_)));

        screen.focus(Item::Play);
        assert!(screen.update(&nav(Nav::Increase)).is_empty());
    }

    #[test]
    fn resetting_clears_the_focus_and_every_hover() {
        let mut screen = screen();
        screen.update(&press(150.0, 110.0));
        assert!(screen.hovered().is_some() && screen.pressed().is_some());

        screen.reset();
        assert_eq!(screen.focused(), None);
        assert_eq!(screen.hovered(), None);
        assert_eq!(screen.pressed(), None);
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
        screen.focus_first();
        for input in [
            at(1.0, 1.0),
            press(1.0, 1.0),
            release(1.0, 1.0),
            nav(Nav::Next),
        ] {
            assert!(screen.update(&input).is_empty());
        }
        assert!(screen.is_empty());
        assert_eq!(screen.len(), 0);
        assert_eq!(screen.widget(Item::Play), None);
    }
}
