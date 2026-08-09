//! What a widget is allowed to know about the player's hands.
//!
//! ```text
//! InputState (mouse, escape)  ->  UiInput  ->  widgets
//! ```
//!
//! One small struct, filled once a frame. A widget never sees a `KeyCode`, never asks whether
//! the left mouse button exists, and never learns that a keyboard is involved: it sees a
//! pointer with three edges.
//!
//! ## The menu is a pointer interface
//!
//! There is no keyboard navigation: no focus to move, no key that activates whatever the
//! focus happens to be sitting on. The game keeps running while the menu is open, so every
//! key the menu could have claimed is a key the player is still using to play — space jumps,
//! tab cycles targets — and a menu that answered them too would be taking input out of the
//! player's hands mid-fight. The mouse is unambiguous; it is the whole interface.
//!
//! Escape is the exception, and it is not navigation: it is the key that opens the menu, so
//! it is also the key that closes it.

use engine_core::input::{InputState, KeyCode, MouseButton};

/// One frame of input, as the UI sees it.
///
/// `pressed` and `released` are edges, not levels: a widget that used only the level could
/// not tell a click from the tail of a drag that started somewhere else, which is how a menu
/// ends up activating a button the player never pressed on.
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
    /// Whether the player asked to back out this frame: escape.
    pub cancel: bool,
}

impl UiInput {
    /// Nothing held, pointer at the origin.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pointer: (0.0, 0.0),
            pointer_down: false,
            pointer_pressed: false,
            pointer_released: false,
            cancel: false,
        }
    }

    /// Reads the engine's input state as UI input.
    ///
    /// The only two devices the UI knows about: the left mouse button, and escape. Escape is
    /// read as a *press* rather than as a hold, so holding it does not toggle the menu sixty
    /// times a second.
    #[must_use]
    pub fn from_input(input: &InputState) -> Self {
        let (x, y) = input.mouse_position();
        Self {
            pointer: (x as f32, y as f32),
            pointer_down: input.is_mouse_button_down(MouseButton::Left),
            pointer_pressed: input.was_mouse_button_pressed(MouseButton::Left),
            pointer_released: input.was_mouse_button_released(MouseButton::Left),
            cancel: input.was_key_pressed(KeyCode::Escape),
        }
    }

    /// The same input with the pointer somewhere else. For tests and for a screen that wants
    /// to hit-test against a translated rectangle.
    #[must_use]
    pub fn with_pointer(mut self, x: f32, y: f32) -> Self {
        self.pointer = (x, y);
        self
    }
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
    fn escape_is_the_only_key_the_interface_reads() {
        let mut state = InputState::new();
        frame(&mut state, &[key(KeyCode::Escape)]);
        assert!(UiInput::from_input(&state).cancel);

        // Everything else belongs to the game, which is still being played behind the menu.
        for other in [
            KeyCode::Space,
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
            KeyCode::W,
            KeyCode::Digit1,
        ] {
            let mut state = InputState::new();
            frame(&mut state, &[key(other)]);
            let input = UiInput::from_input(&state);
            assert!(!input.cancel, "{other:?} reached the interface");
            assert_eq!(input, UiInput::new(), "{other:?} changed the UI's input");
        }
    }

    #[test]
    fn a_held_escape_backs_out_once_rather_than_once_a_frame() {
        let mut state = InputState::new();
        frame(&mut state, &[key(KeyCode::Escape)]);
        assert!(UiInput::from_input(&state).cancel);

        frame(&mut state, &[]);
        assert!(!UiInput::from_input(&state).cancel, "escape repeated");
    }

    #[test]
    fn an_untouched_frame_carries_nothing() {
        let state = InputState::new();
        let input = UiInput::from_input(&state);
        assert!(!input.cancel);
        assert!(!input.pointer_down && !input.pointer_pressed && !input.pointer_released);
        assert_eq!(input, UiInput::new());
    }
}
