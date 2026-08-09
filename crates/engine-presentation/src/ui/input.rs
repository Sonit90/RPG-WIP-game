//! What a widget is allowed to know about the player's hands.
//!
//! ```text
//! InputState (mouse, keys)  ->  UiInput  ->  widgets
//! ```
//!
//! One small struct, filled once a frame. A widget never sees a `KeyCode`, never asks
//! whether the left mouse button exists, and never learns that a keyboard is involved: it
//! sees a pointer with three edges and an optional navigation intent.
//!
//! That is the seam a gamepad would arrive through. Adding one means filling the same struct
//! from a different device — a stick becomes [`Nav::Next`] and [`Nav::Prev`], a face button
//! becomes [`Nav::Activate`] — and not one widget changes.

use engine_core::input::{InputState, KeyCode, MouseButton};

/// A navigation intent, independent of the device that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// Move the focus to the previous widget.
    Prev,
    /// Move the focus to the next widget.
    Next,
    /// Decrease the focused widget's value.
    Decrease,
    /// Increase the focused widget's value.
    Increase,
    /// Activate the focused widget.
    Activate,
    /// Back out: close this screen, or the menu.
    Cancel,
}

/// One frame of input, as the UI sees it.
///
/// `pressed` and `released` are edges, not levels: a widget that used only the level could
/// not tell a click from the tail of a drag that started somewhere else, which is how a
/// menu ends up activating a button the player never pressed on.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct UiInput {
    /// Pointer position in client pixels, from the top-left.
    pub pointer: (f32, f32),
    /// Whether the primary pointer button is held right now.
    pub pointer_down: bool,
    /// Whether it went down this frame.
    pub pointer_pressed: bool,
    /// Whether it was released this frame.
    pub pointer_released: bool,
    /// A navigation intent from this frame, if there was one.
    pub nav: Option<Nav>,
}

impl UiInput {
    /// Nothing held, no intent, pointer at the origin.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pointer: (0.0, 0.0),
            pointer_down: false,
            pointer_pressed: false,
            pointer_released: false,
            nav: None,
        }
    }

    /// Reads the engine's input state as UI input.
    ///
    /// The mapping is the only place a key code appears in the UI:
    ///
    /// | device | intent |
    /// | --- | --- |
    /// | left mouse button | pointer |
    /// | tab / down / up | [`Nav::Next`], [`Nav::Prev`] |
    /// | left / right | [`Nav::Decrease`], [`Nav::Increase`] |
    /// | enter / space | [`Nav::Activate`] |
    /// | escape | [`Nav::Cancel`] |
    ///
    /// Escape is *not* consumed here; the caller decides what backing out means, because it
    /// also opens the menu in the first place.
    #[must_use]
    pub fn from_input(input: &InputState) -> Self {
        let (x, y) = input.mouse_position();
        Self {
            pointer: (x as f32, y as f32),
            pointer_down: input.is_mouse_button_down(MouseButton::Left),
            pointer_pressed: input.was_mouse_button_pressed(MouseButton::Left),
            pointer_released: input.was_mouse_button_released(MouseButton::Left),
            nav: nav_from_input(input),
        }
    }

    /// The same input with the pointer somewhere else. For tests and for a screen that
    /// wants to hit-test against a translated rectangle.
    #[must_use]
    pub fn with_pointer(mut self, x: f32, y: f32) -> Self {
        self.pointer = (x, y);
        self
    }
}

/// This frame's navigation intent, if any. The first match wins.
fn nav_from_input(input: &InputState) -> Option<Nav> {
    // Presses, not holds: a held arrow key must not run a slider to its maximum in three
    // frames. Auto-repeat is the keyboard's job and `InputState` already suppresses it.
    const BINDINGS: [(KeyCode, Nav); 8] = [
        (KeyCode::Tab, Nav::Next),
        (KeyCode::ArrowDown, Nav::Next),
        (KeyCode::ArrowUp, Nav::Prev),
        (KeyCode::ArrowLeft, Nav::Decrease),
        (KeyCode::ArrowRight, Nav::Increase),
        (KeyCode::Enter, Nav::Activate),
        (KeyCode::Space, Nav::Activate),
        (KeyCode::Escape, Nav::Cancel),
    ];
    BINDINGS
        .iter()
        .find(|(key, _)| input.was_key_pressed(*key))
        .map(|(_, nav)| *nav)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::input::InputEvent;

    fn frame(state: &mut InputState, events: &[InputEvent]) {
        state.begin_frame();
        for event in events {
            state.apply(*event);
        }
    }

    fn key(key: KeyCode) -> InputEvent {
        InputEvent::KeyDown { key, repeat: false }
    }

    #[test]
    fn the_pointer_follows_the_mouse_and_its_edges_last_one_frame() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[
                InputEvent::MouseMoved { x: 120, y: 48 },
                InputEvent::MouseButtonDown {
                    button: MouseButton::Left,
                },
            ],
        );

        let input = UiInput::from_input(&state);
        assert_eq!(input.pointer, (120.0, 48.0));
        assert!(input.pointer_down);
        assert!(input.pointer_pressed);
        assert!(!input.pointer_released);

        frame(&mut state, &[]);
        let held = UiInput::from_input(&state);
        assert!(held.pointer_down, "still held");
        assert!(!held.pointer_pressed, "the press was last frame");

        frame(
            &mut state,
            &[InputEvent::MouseButtonUp {
                button: MouseButton::Left,
            }],
        );
        let released = UiInput::from_input(&state);
        assert!(released.pointer_released);
        assert!(!released.pointer_down);
    }

    #[test]
    fn keys_become_device_independent_intents() {
        for (pressed, expected) in [
            (KeyCode::Tab, Nav::Next),
            (KeyCode::ArrowDown, Nav::Next),
            (KeyCode::ArrowUp, Nav::Prev),
            (KeyCode::ArrowLeft, Nav::Decrease),
            (KeyCode::ArrowRight, Nav::Increase),
            (KeyCode::Enter, Nav::Activate),
            (KeyCode::Space, Nav::Activate),
            (KeyCode::Escape, Nav::Cancel),
        ] {
            let mut state = InputState::new();
            frame(&mut state, &[key(pressed)]);
            assert_eq!(
                UiInput::from_input(&state).nav,
                Some(expected),
                "{pressed:?} did not become {expected:?}"
            );
        }
    }

    #[test]
    fn a_held_key_produces_one_intent_rather_than_one_a_frame() {
        // Otherwise a leaned-on arrow key runs a slider end to end in a few frames.
        let mut state = InputState::new();
        frame(&mut state, &[key(KeyCode::ArrowRight)]);
        assert_eq!(UiInput::from_input(&state).nav, Some(Nav::Increase));

        frame(&mut state, &[]);
        assert_eq!(UiInput::from_input(&state).nav, None, "the key repeated");
    }

    #[test]
    fn an_untouched_frame_carries_no_intent_and_no_edges() {
        let state = InputState::new();
        let input = UiInput::from_input(&state);
        assert_eq!(input.nav, None);
        assert!(!input.pointer_down && !input.pointer_pressed && !input.pointer_released);
        assert_eq!(input, UiInput::new().with_pointer(0.0, 0.0));
    }
}
