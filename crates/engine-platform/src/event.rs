//! Window events, the Win32 → engine input translation, and the queue that buffers
//! events between message pumps.
//!
//! Pure logic, no Win32 calls, fully unit-testable. The window procedure feeds this
//! queue; the application drains it once per frame.
//!
//! Virtual-key codes stop here. Above this module the engine sees only
//! [`engine_core::input::KeyCode`] and [`engine_core::input::InputEvent`].

use std::collections::VecDeque;

use engine_core::input::{InputEvent, KeyCode, MouseButton};

/// Number of events the queue can hold before it needs to grow.
///
/// A frame realistically produces a handful of events. Pre-reserving means the
/// per-frame path performs no allocation unless a burst exceeds this capacity.
const INITIAL_CAPACITY: usize = 64;

/// Maps a Windows virtual-key code to a [`KeyCode`].
///
/// Pure, so the mapping is unit-tested without a window. Unnamed keys become
/// [`KeyCode::Unknown`] rather than being dropped, so they can still be logged.
///
/// Left/right modifier pairs are only distinguishable after `MapVirtualKey` disambiguation,
/// which needs the message's scan code; the window procedure does that and calls this
/// function with the already-resolved `VK_LSHIFT`-style code.
#[must_use]
pub fn key_from_virtual_code(code: u16) -> KeyCode {
    const VK_BACK: u16 = 0x08;
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    const VK_SHIFT: u16 = 0x10;
    const VK_CONTROL: u16 = 0x11;
    const VK_MENU: u16 = 0x12;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_SPACE: u16 = 0x20;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_0: u16 = 0x30;
    const VK_9: u16 = 0x39;
    const VK_A: u16 = 0x41;
    const VK_Z: u16 = 0x5A;
    const VK_F1: u16 = 0x70;
    const VK_F12: u16 = 0x7B;
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    const VK_LCONTROL: u16 = 0xA2;
    const VK_RCONTROL: u16 = 0xA3;
    const VK_LMENU: u16 = 0xA4;
    const VK_RMENU: u16 = 0xA5;

    match code {
        VK_BACK => KeyCode::Backspace,
        VK_TAB => KeyCode::Tab,
        VK_RETURN => KeyCode::Enter,
        VK_ESCAPE => KeyCode::Escape,
        VK_SPACE => KeyCode::Space,
        VK_LEFT => KeyCode::ArrowLeft,
        VK_UP => KeyCode::ArrowUp,
        VK_RIGHT => KeyCode::ArrowRight,
        VK_DOWN => KeyCode::ArrowDown,
        // The unsided modifier codes arrive when the side cannot be determined; they are
        // reported as the left-hand key rather than dropped.
        VK_SHIFT | VK_LSHIFT => KeyCode::LeftShift,
        VK_RSHIFT => KeyCode::RightShift,
        VK_CONTROL | VK_LCONTROL => KeyCode::LeftControl,
        VK_RCONTROL => KeyCode::RightControl,
        VK_MENU | VK_LMENU => KeyCode::LeftAlt,
        VK_RMENU => KeyCode::RightAlt,
        VK_0..=VK_9 => KeyCode::digit_key((code - VK_0) as u8).unwrap_or(KeyCode::Unknown),
        VK_A..=VK_Z => KeyCode::ALL[KeyCode::A.index() + (code - VK_A) as usize],
        VK_F1..=VK_F12 => {
            KeyCode::function_key((code - VK_F1 + 1) as u8).unwrap_or(KeyCode::Unknown)
        }
        _ => KeyCode::Unknown,
    }
}

/// Something that happened to a window since the last pump.
///
/// Input arrives as [`WindowEvent::Input`] carrying an engine-level [`InputEvent`]; the
/// remaining variants are window lifecycle, which is not input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowEvent {
    /// Keyboard or mouse activity.
    Input(InputEvent),
    /// The client area changed size. Both dimensions are non-zero.
    Resized {
        /// New client width in pixels.
        width: u32,
        /// New client height in pixels.
        height: u32,
    },
    /// The window was minimized; its client area is zero-sized until restored.
    Minimized,
    /// The window came back from a minimized state.
    Restored,
    /// The user asked to close the window. The application decides whether to obey.
    CloseRequested,
}

impl WindowEvent {
    /// The engine input event this carries, if it is an input event.
    #[must_use]
    pub fn as_input(self) -> Option<InputEvent> {
        match self {
            Self::Input(event) => Some(event),
            _ => None,
        }
    }

    /// Convenience constructor for a key press.
    #[must_use]
    pub fn key_down(key: KeyCode, repeat: bool) -> Self {
        Self::Input(InputEvent::KeyDown { key, repeat })
    }

    /// Convenience constructor for a key release.
    #[must_use]
    pub fn key_up(key: KeyCode) -> Self {
        Self::Input(InputEvent::KeyUp { key })
    }
}

/// FIFO buffer of window events with resize coalescing.
#[derive(Debug)]
pub struct EventQueue {
    events: VecDeque<WindowEvent>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl EventQueue {
    /// Creates an empty queue with room for [`INITIAL_CAPACITY`] events.
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(INITIAL_CAPACITY),
        }
    }

    /// Appends an event.
    ///
    /// Consecutive `Resized` events collapse into the most recent one: dragging a
    /// window border emits hundreds of `WM_SIZE` messages, and only the final size
    /// matters to a renderer that recreates swap-chain buffers.
    ///
    /// Consecutive mouse moves collapse the same way. A high-polling-rate mouse produces
    /// several `WM_MOUSEMOVE` messages per frame and only the latest position matters;
    /// the accumulated delta is preserved because [`engine_core::input::InputState`]
    /// computes it from the previous position, not from the number of events.
    pub fn push(&mut self, event: WindowEvent) {
        let coalesces = matches!(
            (self.events.back(), &event),
            (
                Some(WindowEvent::Resized { .. }),
                WindowEvent::Resized { .. }
            ) | (
                Some(WindowEvent::Input(InputEvent::MouseMoved { .. })),
                WindowEvent::Input(InputEvent::MouseMoved { .. }),
            )
        );
        if coalesces {
            self.events.pop_back();
        }
        self.events.push_back(event);
    }

    /// Removes and returns the oldest event.
    pub fn pop(&mut self) -> Option<WindowEvent> {
        self.events.pop_front()
    }

    /// Returns `true` if no events are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Returns the number of buffered events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }
}

/// Extracts the client width and height packed into a `WM_SIZE` `LPARAM`.
///
/// Windows packs width in the low word and height in the high word.
#[must_use]
pub fn size_from_lparam(lparam: isize) -> (u32, u32) {
    let bits = lparam as usize as u64;
    let width = (bits & 0xffff) as u32;
    let height = ((bits >> 16) & 0xffff) as u32;
    (width, height)
}

/// Extracts the signed cursor position packed into a mouse message `LPARAM`.
///
/// The coordinates are signed 16-bit values: dragging above or left of the client area
/// while a button is captured produces negative values, and treating them as unsigned
/// would teleport the cursor to 65,000 pixels.
#[must_use]
pub fn cursor_from_lparam(lparam: isize) -> (i32, i32) {
    let bits = lparam as usize as u64;
    let x = i32::from((bits & 0xffff) as u16 as i16);
    let y = i32::from(((bits >> 16) & 0xffff) as u16 as i16);
    (x, y)
}

/// Converts a `WM_MOUSEWHEEL` `WPARAM` into wheel notches.
///
/// Windows reports multiples of `WHEEL_DELTA` (120) in the high word, signed.
#[must_use]
pub fn wheel_from_wparam(wparam: usize) -> f32 {
    const WHEEL_DELTA: f32 = 120.0;
    let raw = ((wparam >> 16) & 0xffff) as u16 as i16;
    f32::from(raw) / WHEEL_DELTA
}

/// Which side button an `XBUTTON` message refers to.
///
/// Windows packs the button index into the high word of `WPARAM`.
#[must_use]
pub fn x_button_from_wparam(wparam: usize) -> MouseButton {
    const XBUTTON1: u16 = 0x0001;
    match ((wparam >> 16) & 0xffff) as u16 {
        XBUTTON1 => MouseButton::Back,
        _ => MouseButton::Forward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_in_fifo_order() {
        let mut queue = EventQueue::new();
        queue.push(WindowEvent::Input(InputEvent::FocusChanged {
            focused: true,
        }));
        queue.push(WindowEvent::CloseRequested);
        assert_eq!(
            queue.pop(),
            Some(WindowEvent::Input(InputEvent::FocusChanged {
                focused: true
            }))
        );
        assert_eq!(queue.pop(), Some(WindowEvent::CloseRequested));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn coalesces_consecutive_resizes() {
        let mut queue = EventQueue::new();
        for height in [100, 200, 300] {
            queue.push(WindowEvent::Resized {
                width: 1280,
                height,
            });
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.pop(),
            Some(WindowEvent::Resized {
                width: 1280,
                height: 300
            })
        );
    }

    #[test]
    fn does_not_coalesce_resizes_separated_by_another_event() {
        let mut queue = EventQueue::new();
        queue.push(WindowEvent::Resized {
            width: 800,
            height: 600,
        });
        queue.push(WindowEvent::Minimized);
        queue.push(WindowEvent::Resized {
            width: 1024,
            height: 768,
        });
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn coalesces_consecutive_mouse_moves_to_the_latest_position() {
        let mut queue = EventQueue::new();
        for x in [10, 20, 30] {
            queue.push(WindowEvent::Input(InputEvent::MouseMoved { x, y: 5 }));
        }
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.pop(),
            Some(WindowEvent::Input(InputEvent::MouseMoved { x: 30, y: 5 }))
        );
    }

    #[test]
    fn does_not_coalesce_mouse_moves_across_a_click() {
        let mut queue = EventQueue::new();
        queue.push(WindowEvent::Input(InputEvent::MouseMoved { x: 1, y: 1 }));
        queue.push(WindowEvent::Input(InputEvent::MouseButtonDown {
            button: MouseButton::Left,
        }));
        queue.push(WindowEvent::Input(InputEvent::MouseMoved { x: 2, y: 2 }));
        assert_eq!(
            queue.len(),
            3,
            "a click must stay ordered against the moves around it"
        );
    }

    #[test]
    fn maps_letters_digits_and_function_keys() {
        assert_eq!(key_from_virtual_code(0x41), KeyCode::A);
        assert_eq!(key_from_virtual_code(0x57), KeyCode::W);
        assert_eq!(key_from_virtual_code(0x5A), KeyCode::Z);
        assert_eq!(key_from_virtual_code(0x30), KeyCode::Digit0);
        assert_eq!(key_from_virtual_code(0x39), KeyCode::Digit9);
        assert_eq!(key_from_virtual_code(0x70), KeyCode::F1);
        assert_eq!(key_from_virtual_code(0x74), KeyCode::F5);
        assert_eq!(key_from_virtual_code(0x7B), KeyCode::F12);
    }

    #[test]
    fn maps_the_named_control_keys() {
        assert_eq!(key_from_virtual_code(0x1B), KeyCode::Escape);
        assert_eq!(key_from_virtual_code(0x20), KeyCode::Space);
        assert_eq!(key_from_virtual_code(0x0D), KeyCode::Enter);
        assert_eq!(key_from_virtual_code(0x09), KeyCode::Tab);
        assert_eq!(key_from_virtual_code(0x08), KeyCode::Backspace);
        assert_eq!(key_from_virtual_code(0x25), KeyCode::ArrowLeft);
        assert_eq!(key_from_virtual_code(0x26), KeyCode::ArrowUp);
        assert_eq!(key_from_virtual_code(0x27), KeyCode::ArrowRight);
        assert_eq!(key_from_virtual_code(0x28), KeyCode::ArrowDown);
    }

    #[test]
    fn maps_modifiers_to_a_side_and_defaults_to_the_left() {
        assert_eq!(key_from_virtual_code(0xA0), KeyCode::LeftShift);
        assert_eq!(key_from_virtual_code(0xA1), KeyCode::RightShift);
        assert_eq!(key_from_virtual_code(0xA2), KeyCode::LeftControl);
        assert_eq!(key_from_virtual_code(0xA3), KeyCode::RightControl);
        assert_eq!(key_from_virtual_code(0xA4), KeyCode::LeftAlt);
        assert_eq!(key_from_virtual_code(0xA5), KeyCode::RightAlt);
        assert_eq!(key_from_virtual_code(0x10), KeyCode::LeftShift);
        assert_eq!(key_from_virtual_code(0x11), KeyCode::LeftControl);
        assert_eq!(key_from_virtual_code(0x12), KeyCode::LeftAlt);
    }

    #[test]
    fn keeps_unnamed_keys_as_unknown() {
        assert_eq!(key_from_virtual_code(0x7C), KeyCode::Unknown, "F13");
        assert_eq!(key_from_virtual_code(0x00), KeyCode::Unknown);
        assert_eq!(key_from_virtual_code(0xFF), KeyCode::Unknown);
    }

    #[test]
    fn unpacks_wm_size_lparam() {
        // width = 1920 (0x0780), height = 1080 (0x0438)
        let lparam = ((1080_isize) << 16) | 1920;
        assert_eq!(size_from_lparam(lparam), (1920, 1080));
        assert_eq!(size_from_lparam(0), (0, 0));
    }

    #[test]
    fn unpacks_maximum_word_values_without_sign_extension() {
        let lparam = ((0xffff_isize) << 16) | 0xffff;
        assert_eq!(size_from_lparam(lparam), (65535, 65535));
    }

    #[test]
    fn unpacks_cursor_positions_including_negative_ones() {
        let lparam = ((300_isize) << 16) | 200;
        assert_eq!(cursor_from_lparam(lparam), (200, 300));

        // A drag above and left of the client area reports negative coordinates.
        let negative = ((0xfffb_isize) << 16) | 0xfff6;
        assert_eq!(cursor_from_lparam(negative), (-10, -5));
    }

    #[test]
    fn converts_wheel_deltas_to_notches() {
        assert!((wheel_from_wparam(120 << 16) - 1.0).abs() < 1e-6);
        assert!((wheel_from_wparam(240 << 16) - 2.0).abs() < 1e-6);
        // -120 in the high word is one notch towards the user.
        assert!((wheel_from_wparam(0xff88 << 16) + 1.0).abs() < 1e-6);
        assert_eq!(wheel_from_wparam(0), 0.0);
    }

    #[test]
    fn distinguishes_the_two_side_buttons() {
        assert_eq!(x_button_from_wparam(1 << 16), MouseButton::Back);
        assert_eq!(x_button_from_wparam(2 << 16), MouseButton::Forward);
    }

    #[test]
    fn input_events_are_queued_like_any_other() {
        let mut queue = EventQueue::new();
        queue.push(WindowEvent::key_down(KeyCode::F5, false));
        queue.push(WindowEvent::CloseRequested);
        assert_eq!(
            queue.pop().and_then(WindowEvent::as_input),
            Some(InputEvent::KeyDown {
                key: KeyCode::F5,
                repeat: false
            })
        );
        assert_eq!(queue.pop(), Some(WindowEvent::CloseRequested));
    }

    #[test]
    fn non_input_events_carry_no_input() {
        assert_eq!(WindowEvent::CloseRequested.as_input(), None);
        assert_eq!(WindowEvent::Minimized.as_input(), None);
    }
}
