//! Window events and the queue that buffers them between message pumps.
//!
//! Pure logic, no Win32 calls, fully unit-testable. The window procedure feeds this
//! queue; the application drains it once per frame.

use std::collections::VecDeque;

/// Number of events the queue can hold before it needs to grow.
///
/// A frame realistically produces a handful of events. Pre-reserving means the
/// per-frame path performs no allocation unless a burst exceeds this capacity.
const INITIAL_CAPACITY: usize = 32;

/// Something that happened to a window since the last pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
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
    /// Keyboard focus was gained (`true`) or lost (`false`).
    FocusChanged(bool),
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
    pub fn push(&mut self, event: WindowEvent) {
        if let WindowEvent::Resized { .. } = event
            && matches!(self.events.back(), Some(WindowEvent::Resized { .. }))
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_in_fifo_order() {
        let mut queue = EventQueue::new();
        queue.push(WindowEvent::FocusChanged(true));
        queue.push(WindowEvent::CloseRequested);
        assert_eq!(queue.pop(), Some(WindowEvent::FocusChanged(true)));
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
}
