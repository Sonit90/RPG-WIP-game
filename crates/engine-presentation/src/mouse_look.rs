//! When the mouse turns the camera: hold the right button.
//!
//! ```text
//! RMB down  ->  Began  ->  cursor hidden, movement rotates the camera
//! RMB up    ->  Ended  ->  cursor restored, movement does nothing
//! ```
//!
//! The World of Warcraft convention, and the one this project follows: the right button is
//! *held* to look around and the left button is left free for the world. There is no mode
//! to leave, so no key cancels one — Escape included.
//!
//! A state machine rather than a pair of event handlers in the application, because "held"
//! is the whole rule and it is the part that can go wrong: an implementation that toggled
//! on a press can be left rotating after a release the window never delivered. This one is
//! driven by the button's *current* state every frame, so it cannot disagree with the
//! hardware for longer than a frame — including after a focus loss, which releases every
//! button at once and ends the look with it.

use engine_core::input::{InputState, MouseButton};

/// The button held to turn the camera.
pub const LOOK_BUTTON: MouseButton = MouseButton::Right;

/// A change in whether the mouse is turning the camera.
///
/// Returned so the application can hide and restore the cursor exactly once per transition
/// rather than every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookChange {
    /// Rotation started this frame.
    Began,
    /// Rotation ended this frame.
    Ended,
}

/// Whether the mouse is currently turning the camera.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MouseLook {
    active: bool,
}

impl MouseLook {
    /// Not rotating.
    #[must_use]
    pub const fn new() -> Self {
        Self { active: false }
    }

    /// Whether mouse movement should rotate the camera right now.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Reads this frame's input and reports a transition, if there was one.
    ///
    /// Only [`LOOK_BUTTON`] is consulted. The left button is deliberately not: it belongs
    /// to the world, and clicking in it must never take the cursor away.
    pub fn update(&mut self, input: &InputState) -> Option<LookChange> {
        self.set(input.is_mouse_button_down(LOOK_BUTTON))
    }

    /// Ends rotation regardless of the button, reporting the transition if there was one.
    ///
    /// For the window losing focus: the release message goes to whichever window has focus
    /// now, so waiting for one would leave the cursor hidden on the desktop.
    pub fn cancel(&mut self) -> Option<LookChange> {
        self.set(false)
    }

    fn set(&mut self, active: bool) -> Option<LookChange> {
        if active == self.active {
            return None;
        }
        self.active = active;
        Some(if active {
            LookChange::Began
        } else {
            LookChange::Ended
        })
    }
}

/// Mouse movement while looking, with the engine's own cursor warps taken back out.
///
/// ```text
/// player moves the mouse   ->  counted
/// the engine warps it      ->  not counted
/// the button goes down     ->  the cursor is wherever it was; nothing is counted
/// ```
///
/// Only movement rotates the camera. Where the cursor happened to be when the button went
/// down is not movement, and neither is the re-centring the engine does to keep the cursor
/// off the screen edge — so pressing the button near a corner turns the camera by exactly
/// as much as pressing it in the middle: nothing.
///
/// Movement is accumulated from the *events*, not from a position compared against the
/// centre, because the two disagree for a frame or more: the warp's own move message is
/// delivered whenever the window gets around to it, and a comparison against the centre
/// reads that lag as a mouse movement and snaps the camera round. Here the warp is
/// cancelled by matching the message it produces, so it does not matter when it arrives.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RelativeMotion {
    /// The last cursor position seen, in client pixels.
    cursor: (i32, i32),
    /// Movement accumulated since it was last taken.
    delta: (i32, i32),
    /// Where a warp this code asked for is expected to land, until its message arrives.
    warp: Option<(i32, i32)>,
}

impl RelativeMotion {
    /// No movement, cursor at the origin.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cursor: (0, 0),
            delta: (0, 0),
            warp: None,
        }
    }

    /// Starts tracking at `position`, discarding whatever came before.
    ///
    /// Called when the look button goes down: the cursor's position at that moment is the
    /// origin movement is measured from, so the press itself rotates nothing.
    pub fn start_at(&mut self, position: (i32, i32)) {
        self.cursor = position;
        self.delta = (0, 0);
        self.warp = None;
    }

    /// Applies one cursor movement, in client pixels.
    ///
    /// A movement that lands exactly where a warp was asked to put the cursor *is* that
    /// warp: it moves the reference point without counting as movement.
    pub fn moved(&mut self, position: (i32, i32)) {
        if self.warp == Some(position) {
            self.warp = None;
        } else {
            self.delta.0 += position.0 - self.cursor.0;
            self.delta.1 += position.1 - self.cursor.1;
        }
        self.cursor = position;
    }

    /// Records that the cursor has been warped to `target` and that its message is owed.
    pub fn note_warp(&mut self, target: (i32, i32)) {
        self.warp = Some(target);
    }

    /// Whether a warp is still waiting for its message.
    ///
    /// A second warp while one is outstanding would leave the first unmatched forever, so
    /// the caller warps only when this is false.
    #[must_use]
    pub fn warp_pending(&self) -> bool {
        self.warp.is_some()
    }

    /// The last cursor position seen.
    #[must_use]
    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    /// Takes the movement accumulated so far and resets it to nothing.
    pub fn take(&mut self) -> (i32, i32) {
        std::mem::take(&mut self.delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::input::{InputEvent, KeyCode};

    /// Applies events the way the engine does: transitions first, then the frame's events.
    fn frame(input: &mut InputState, events: &[InputEvent]) {
        input.begin_frame();
        for event in events {
            input.apply(*event);
        }
    }

    fn button(button: MouseButton, down: bool) -> InputEvent {
        if down {
            InputEvent::MouseButtonDown { button }
        } else {
            InputEvent::MouseButtonUp { button }
        }
    }

    #[test]
    fn the_left_button_does_not_turn_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Left, true)]);
        assert_eq!(look.update(&input), None);
        assert!(!look.is_active(), "the left button started a rotation");

        frame(&mut input, &[]);
        assert_eq!(look.update(&input), None);
        assert!(!look.is_active(), "holding it started one later");
    }

    #[test]
    fn holding_the_right_button_turns_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        assert_eq!(look.update(&input), Some(LookChange::Began));
        assert!(look.is_active());

        // Held across frames: still rotating, and no second transition to act on.
        for _ in 0..5 {
            frame(&mut input, &[]);
            assert_eq!(look.update(&input), None);
            assert!(look.is_active());
        }
    }

    #[test]
    fn releasing_the_right_button_stops_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input);

        frame(&mut input, &[button(MouseButton::Right, false)]);
        assert_eq!(look.update(&input), Some(LookChange::Ended));
        assert!(!look.is_active());

        frame(&mut input, &[]);
        assert_eq!(look.update(&input), None);
        assert!(!look.is_active(), "rotation resumed on its own");
    }

    #[test]
    fn repeated_presses_never_leave_the_camera_stuck() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        for round in 0..20 {
            frame(&mut input, &[button(MouseButton::Right, true)]);
            look.update(&input);
            assert!(look.is_active(), "round {round} did not start");

            frame(&mut input, &[button(MouseButton::Right, false)]);
            look.update(&input);
            assert!(!look.is_active(), "round {round} left the camera rotating");
        }
    }

    #[test]
    fn escape_is_not_needed_to_stop_rotating() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input);

        // Escape while rotating changes nothing: the button is still held.
        frame(
            &mut input,
            &[InputEvent::KeyDown {
                key: KeyCode::Escape,
                repeat: false,
            }],
        );
        assert_eq!(look.update(&input), None);
        assert!(look.is_active(), "escape cancelled the rotation");

        // The button alone ends it.
        frame(&mut input, &[button(MouseButton::Right, false)]);
        assert_eq!(look.update(&input), Some(LookChange::Ended));
    }

    #[test]
    fn losing_focus_ends_the_rotation() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input);

        frame(&mut input, &[InputEvent::FocusChanged { focused: false }]);
        assert_eq!(
            look.update(&input),
            Some(LookChange::Ended),
            "the button was cleared but the camera kept rotating"
        );
    }

    #[test]
    fn where_the_button_was_pressed_does_not_rotate_the_camera() {
        // The whole point: pressing in a corner and pressing in the middle must both
        // rotate by nothing until the mouse actually moves.
        let mut corner = RelativeMotion::new();
        corner.start_at((3, 700));
        let mut middle = RelativeMotion::new();
        middle.start_at((640, 360));

        assert_eq!(corner.take(), (0, 0));
        assert_eq!(middle.take(), (0, 0));

        // And the same movement from either turns the camera by the same amount.
        corner.moved((23, 690));
        middle.moved((660, 350));
        assert_eq!(corner.take(), (20, -10));
        assert_eq!(middle.take(), (20, -10));
    }

    #[test]
    fn only_movement_is_counted_not_the_engines_own_warp() {
        let mut motion = RelativeMotion::new();
        motion.start_at((100, 100));

        motion.note_warp((640, 360));
        assert!(motion.warp_pending());
        motion.moved((640, 360));
        assert!(!motion.warp_pending(), "the warp was never matched");
        assert_eq!(motion.take(), (0, 0), "the warp rotated the camera");

        // Movement after the warp is measured from where the warp put the cursor.
        motion.moved((650, 360));
        assert_eq!(motion.take(), (10, 0));
    }

    #[test]
    fn a_warp_that_arrives_late_still_costs_nothing() {
        // The warp's message can be a frame or more behind. Movement that happens in the
        // meantime must survive, and the warp must still be cancelled when it lands.
        let mut motion = RelativeMotion::new();
        motion.start_at((640, 360));

        motion.moved((660, 360));
        assert_eq!(motion.take(), (20, 0));

        motion.note_warp((640, 360));
        // A frame passes with the warp still in flight.
        assert_eq!(motion.take(), (0, 0));

        motion.moved((640, 360));
        motion.moved((645, 358));
        assert_eq!(motion.take(), (5, -2), "the warp leaked into the rotation");
    }

    #[test]
    fn movement_accumulates_between_takes() {
        let mut motion = RelativeMotion::new();
        motion.start_at((0, 0));
        for step in 1..=4 {
            motion.moved((step * 3, step * -2));
        }
        assert_eq!(motion.take(), (12, -8));
        assert_eq!(motion.take(), (0, 0), "movement was counted twice");
        assert_eq!(motion.cursor(), (12, -8));
    }

    #[test]
    fn starting_again_forgets_the_previous_hold() {
        // Release, move the mouse across the screen, press again: none of that movement is
        // a rotation, and neither is the new press position.
        let mut motion = RelativeMotion::new();
        motion.start_at((640, 360));
        motion.moved((700, 300));
        assert_ne!(motion.take(), (0, 0));

        motion.moved((10, 10));
        motion.start_at((10, 10));
        assert_eq!(motion.take(), (0, 0));
        assert!(!motion.warp_pending());
    }

    #[test]
    fn cancelling_reports_the_transition_exactly_once() {
        let mut look = MouseLook::new();
        assert_eq!(look.cancel(), None, "nothing to cancel");

        let mut input = InputState::new();
        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input);

        assert_eq!(look.cancel(), Some(LookChange::Ended));
        assert_eq!(look.cancel(), None);
        assert!(!look.is_active());
    }
}
