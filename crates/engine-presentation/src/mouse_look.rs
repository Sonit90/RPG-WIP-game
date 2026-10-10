//! When the mouse turns the camera: hold a button.
//!
//! ```text
//! LMB down  ->  Began  ->  cursor hidden, movement orbits the camera
//! RMB down  ->  Began  ->  cursor hidden, movement orbits the camera *and* steers
//! both up   ->  Ended  ->  cursor restored, movement does nothing
//! ```
//!
//! The World of Warcraft convention, and the one this project follows: **both** buttons are
//! *held* to look, and they differ in what else they do. The left orbits the camera around a
//! character that keeps facing where it was; the right orbits and turns the character to
//! match, so the camera is steering. There is no mode to leave, so no key cancels one —
//! Escape included.
//!
//! Steering is not decided here. `engine_game::InputMap` reads the right button directly for
//! the `face_camera` flag on a movement command, because which button steers is a binding
//! and bindings live there. This module's only job is "is the mouse turning the camera".
//!
//! # Why the left button needs permission and the right one does not
//!
//! The left button also belongs to the interface: it clicks a menu, and it confirms where a
//! ground-targeted spell lands. Starting an orbit on a press meant for one of those would
//! take the cursor away mid-click. So [`MouseLook::update`] is told each frame whether the
//! left button is free, and the caller answers with "no menu is open and nothing is being
//! aimed". The right button does nothing in the interface and needs no such gate.
//!
//! A state machine rather than a pair of event handlers in the application, because "held"
//! is the whole rule and it is the part that can go wrong: an implementation that toggled
//! on a press can be left rotating after a release the window never delivered. This one is
//! driven by the buttons' *current* state every frame, so it cannot disagree with the
//! hardware for longer than a frame — including after a focus loss, which releases every
//! button at once and ends the look with it.

use engine_core::input::{InputState, MouseButton};

/// The button held to turn the camera *and* steer the character with it.
pub const LOOK_BUTTON: MouseButton = MouseButton::Right;

/// The button held to orbit the camera around a character that keeps facing where it was.
pub const ORBIT_BUTTON: MouseButton = MouseButton::Left;

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

/// What a held button is doing to the camera.
///
/// Reported so the developer overlay and a future binding screen can say which of the two
/// is running. Nothing in the frame path branches on it: steering is `face_camera` on a
/// movement command, and that is read from the button itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookMode {
    /// Left button: the camera moves around the character, the character does not turn.
    Orbit,
    /// Right button: the camera moves and the character turns to face where it looks.
    Steer,
}

/// Whether the mouse is currently turning the camera, and how.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MouseLook {
    active: Option<LookMode>,
}

impl MouseLook {
    /// Not rotating.
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    /// Whether mouse movement should rotate the camera right now.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Which of the two is running, if either.
    #[must_use]
    pub fn mode(&self) -> Option<LookMode> {
        self.active
    }

    /// Reads this frame's input and reports a transition, if there was one.
    ///
    /// `orbit_allowed` is the caller's answer to "is the left button free this frame" — no
    /// menu open, nothing being aimed. It gates [`ORBIT_BUTTON`] only; [`LOOK_BUTTON`] is
    /// always honoured, because it does nothing in the interface.
    ///
    /// **The right button wins.** A player who starts orbiting and then presses the right
    /// button as well is asking to steer, and the look continues without a transition —
    /// which is what stops the cursor flickering back and forth in the middle of a turn.
    pub fn update(&mut self, input: &InputState, orbit_allowed: bool) -> Option<LookChange> {
        let mode = if input.is_mouse_button_down(LOOK_BUTTON) {
            Some(LookMode::Steer)
        } else if orbit_allowed && input.is_mouse_button_down(ORBIT_BUTTON) {
            Some(LookMode::Orbit)
        } else {
            None
        };
        self.set(mode)
    }

    /// Ends rotation regardless of the button, reporting the transition if there was one.
    ///
    /// For the window losing focus: the release message goes to whichever window has focus
    /// now, so waiting for one would leave the cursor hidden on the desktop.
    pub fn cancel(&mut self) -> Option<LookChange> {
        self.set(None)
    }

    /// Applies a new mode, reporting only a change in *whether* the camera is turning.
    ///
    /// Switching between the two modes mid-turn is not a transition: the cursor is already
    /// hidden and already warped, and reporting one would restore it and take it away again
    /// in the same frame.
    fn set(&mut self, mode: Option<LookMode>) -> Option<LookChange> {
        let was_active = self.active.is_some();
        let is_active = mode.is_some();
        self.active = mode;
        if was_active == is_active {
            return None;
        }
        Some(if is_active {
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
///
/// # Raw input first, positions as the fallback
///
/// Matching warp messages is fragile: the message can be coalesced away by the event queue or
/// replaced by one Windows synthesises from the cursor's current state. A warp that is never
/// matched makes the next move count the whole warp distance (the camera snaps) and, because
/// it stays pending, stops the cursor being recentred, so it runs into the window edge and
/// the camera stops turning. So the primary source is raw relative device motion
/// ([`RelativeMotion::raw`]), which no cursor warp or screen edge can disturb. Once any raw
/// motion has arrived, cursor positions only maintain the reference point. The position path
/// remains for devices that produce no raw relative input (remote desktop, some tablets).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RelativeMotion {
    /// The last cursor position seen, in client pixels.
    cursor: (i32, i32),
    /// Movement accumulated since it was last taken.
    delta: (i32, i32),
    /// Where a warp this code asked for is expected to land, until its message arrives.
    warp: Option<(i32, i32)>,
    /// Whether raw relative motion has ever arrived. Sticky across holds: a device that
    /// reported raw motion once will keep doing so.
    raw_seen: bool,
}

impl RelativeMotion {
    /// No movement, cursor at the origin.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cursor: (0, 0),
            delta: (0, 0),
            warp: None,
            raw_seen: false,
        }
    }

    /// Adds raw relative device motion, in device counts.
    ///
    /// Marks raw input as the source of truth from now on; see the type docs.
    pub fn raw(&mut self, dx: i32, dy: i32) {
        self.delta.0 = self.delta.0.saturating_add(dx);
        self.delta.1 = self.delta.1.saturating_add(dy);
        self.raw_seen = true;
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
    ///
    /// Once raw input has been seen this only moves the reference point: the position is
    /// not trusted to carry motion, because a warp whose message was lost looks like a jump.
    pub fn moved(&mut self, position: (i32, i32)) {
        if self.warp == Some(position) {
            self.warp = None;
        } else if !self.raw_seen {
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
    /// the caller warps only when this is false. With raw input the warp is cosmetic and
    /// never needs matching, so this is always false then.
    #[must_use]
    pub fn warp_pending(&self) -> bool {
        !self.raw_seen && self.warp.is_some()
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
    fn holding_the_left_button_orbits_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Left, true)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Began));
        assert_eq!(look.mode(), Some(LookMode::Orbit));

        // Held across frames: still orbiting, and no second transition to act on.
        for _ in 0..5 {
            frame(&mut input, &[]);
            assert_eq!(look.update(&input, true), None);
            assert!(look.is_active());
        }

        frame(&mut input, &[button(MouseButton::Left, false)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Ended));
        assert_eq!(look.mode(), None);
    }

    #[test]
    fn the_left_button_does_not_orbit_when_the_interface_wants_it() {
        // Clicking a menu button or confirming where a spell lands must not take the cursor
        // away. The right button is unaffected, because it does nothing in the interface.
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Left, true)]);
        assert_eq!(look.update(&input, false), None);
        assert!(
            !look.is_active(),
            "a click on the interface started an orbit"
        );

        frame(&mut input, &[button(MouseButton::Right, true)]);
        assert_eq!(look.update(&input, false), Some(LookChange::Began));
        assert_eq!(look.mode(), Some(LookMode::Steer));
    }

    #[test]
    fn an_orbit_that_is_no_longer_permitted_ends_rather_than_continuing() {
        // The player holds the left button and something opens a menu. The look has to stop
        // on the same frame the cursor is needed again.
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Left, true)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Began));

        frame(&mut input, &[]);
        assert_eq!(look.update(&input, false), Some(LookChange::Ended));
        assert!(!look.is_active());
    }

    #[test]
    fn adding_the_right_button_mid_orbit_starts_steering_without_a_transition() {
        // A transition here would restore the cursor and hide it again in the same frame,
        // which is a visible flicker in the middle of a turn.
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Left, true)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Began));
        assert_eq!(look.mode(), Some(LookMode::Orbit));

        frame(&mut input, &[button(MouseButton::Right, true)]);
        assert_eq!(
            look.update(&input, true),
            None,
            "the cursor must not flicker"
        );
        assert_eq!(look.mode(), Some(LookMode::Steer));

        // And releasing the right button hands the turn back to the left one, still without
        // a transition, because the left button never went up.
        frame(&mut input, &[button(MouseButton::Right, false)]);
        assert_eq!(look.update(&input, true), None);
        assert_eq!(look.mode(), Some(LookMode::Orbit));

        frame(&mut input, &[button(MouseButton::Left, false)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Ended));
    }

    #[test]
    fn the_right_button_wins_while_both_are_held() {
        // Steering is the more specific request, and the player pressed it deliberately.
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(
            &mut input,
            &[
                button(MouseButton::Right, true),
                button(MouseButton::Left, true),
            ],
        );
        assert_eq!(look.update(&input, true), Some(LookChange::Began));
        assert_eq!(look.mode(), Some(LookMode::Steer));
    }

    #[test]
    fn holding_the_right_button_turns_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Began));
        assert!(look.is_active());

        // Held across frames: still rotating, and no second transition to act on.
        for _ in 0..5 {
            frame(&mut input, &[]);
            assert_eq!(look.update(&input, true), None);
            assert!(look.is_active());
        }
    }

    #[test]
    fn releasing_the_right_button_stops_the_camera() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input, true);

        frame(&mut input, &[button(MouseButton::Right, false)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Ended));
        assert!(!look.is_active());

        frame(&mut input, &[]);
        assert_eq!(look.update(&input, true), None);
        assert!(!look.is_active(), "rotation resumed on its own");
    }

    #[test]
    fn repeated_presses_never_leave_the_camera_stuck() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        for round in 0..20 {
            frame(&mut input, &[button(MouseButton::Right, true)]);
            look.update(&input, true);
            assert!(look.is_active(), "round {round} did not start");

            frame(&mut input, &[button(MouseButton::Right, false)]);
            look.update(&input, true);
            assert!(!look.is_active(), "round {round} left the camera rotating");
        }
    }

    #[test]
    fn escape_is_not_needed_to_stop_rotating() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input, true);

        // Escape while rotating changes nothing: the button is still held.
        frame(
            &mut input,
            &[InputEvent::KeyDown {
                key: KeyCode::Escape,
                repeat: false,
            }],
        );
        assert_eq!(look.update(&input, true), None);
        assert!(look.is_active(), "escape cancelled the rotation");

        // The button alone ends it.
        frame(&mut input, &[button(MouseButton::Right, false)]);
        assert_eq!(look.update(&input, true), Some(LookChange::Ended));
    }

    #[test]
    fn losing_focus_ends_the_rotation() {
        let mut input = InputState::new();
        let mut look = MouseLook::new();

        frame(&mut input, &[button(MouseButton::Right, true)]);
        look.update(&input, true);

        frame(&mut input, &[InputEvent::FocusChanged { focused: false }]);
        assert_eq!(
            look.update(&input, true),
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
    fn raw_deltas_accumulate_and_take_resets() {
        let mut motion = RelativeMotion::new();
        motion.start_at((0, 0));
        motion.raw(3, -1);
        motion.raw(4, 5);
        assert_eq!(motion.take(), (7, 4));
        assert_eq!(motion.take(), (0, 0));
    }

    #[test]
    fn once_raw_input_is_seen_positions_add_nothing() {
        let mut motion = RelativeMotion::new();
        motion.start_at((640, 360));
        motion.raw(2, 2);
        motion.note_warp((640, 360));
        assert!(!motion.warp_pending(), "recentring is unconditional");
        // A warp whose message was lost: the next move is a huge jump.
        motion.moved((100, 900));
        motion.moved((110, 905));
        assert_eq!(motion.take(), (2, 2), "position jumps leaked into rotation");
        assert_eq!(motion.cursor(), (110, 905));

        // Sticky across holds.
        motion.start_at((50, 50));
        motion.moved((500, 500));
        assert_eq!(motion.take(), (0, 0));
    }

    #[test]
    fn without_raw_input_the_position_path_still_works() {
        let mut motion = RelativeMotion::new();
        motion.start_at((0, 0));
        motion.moved((5, 6));
        assert_eq!(motion.take(), (5, 6));
        motion.note_warp((1, 1));
        assert!(motion.warp_pending());
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
        look.update(&input, true);

        assert_eq!(look.cancel(), Some(LookChange::Ended));
        assert_eq!(look.cancel(), None);
        assert!(!look.is_active());
    }
}
