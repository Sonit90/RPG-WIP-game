//! macOS window creation and event pumping, on winit over AppKit.
//!
//! The Windows window is a hand-written Win32 class with a window procedure; on macOS the
//! platform layer is winit, used in its *pump* form. The engine owns the frame loop, so this
//! module never calls `run_app` (which would take the thread and never return): each
//! [`Window::pump_events`] hands winit a zero timeout, winit drains whatever AppKit has
//! queued into [`ApplicationHandler`] callbacks, and those callbacks translate the events
//! into the same [`WindowEvent`]/[`InputEvent`] values the Windows window procedure produces.
//! Nothing above this crate can tell which backend ran.
//!
//! ```text
//! Window::pump_events ──> pump_app_events(ZERO) ──> App::window_event / device_event
//!                                                          │
//!                                       EventQueue (same coalescing as Windows)
//! ```
//!
//! ## Threads
//!
//! AppKit only allows windows and event loops on the process's main thread, and winit enforces
//! it: [`Window::new`] must be called from `main`'s thread. The engine's loop already runs
//! there. (This is also why this backend has no windowed integration tests: the test harness
//! runs tests on worker threads.)
//!
//! ## Units
//!
//! Every size and coordinate is a physical pixel, like the per-monitor-DPI-aware Windows
//! window: winit reports physical sizes, so a Retina window comes out 2x its point size and
//! the renderer's drawable can match it one to one.

use std::time::{Duration, Instant};

use engine_core::input::{InputEvent, KeyCode, MouseButton};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton as WinitButton, MouseScrollDelta,
    WindowEvent as WinitEvent,
};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode as Key, PhysicalKey};
use winit::platform::macos::{
    EventLoopBuilderExtMacOS, MonitorHandleExtMacOS, WindowAttributesExtMacOS,
};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorGrabMode, Fullscreen, WindowAttributes, WindowId};

pub use crate::config::CursorClip;
use crate::config::{AppliedDisplay, WindowConfig, WindowMode};
use crate::display::{self, MonitorInfo};
use crate::error::PlatformError;
use crate::event::{EventQueue, WindowEvent};

/// How long [`Window::new`] waits for AppKit to finish launching and winit to deliver the
/// first `resumed`, which is where a window may be created.
///
/// In practice it takes a few pumps, a few milliseconds. The bound only stops a broken
/// session (no window server) from hanging start-up.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Pixels of trackpad scroll that count as one wheel notch.
///
/// Windows reports a wheel as whole notches; a Mac trackpad or Magic Mouse reports pixels.
/// AppKit's own line step for a precise scroll is about ten points, and a notch scrolls a
/// few lines, so this is the number of *physical* pixels that makes a trackpad swipe feel
/// like a similar number of notches to a mouse wheel. A tuning constant, not a measurement.
const PIXELS_PER_NOTCH: f64 = 40.0;

/// Everything the winit callbacks write into.
///
/// Owned by [`Window`] and lent to winit for the duration of each pump, which is how the
/// callbacks reach the event queue without any shared pointer (Win32 needs `GWLP_USERDATA`
/// for the same job).
struct App {
    events: EventQueue,
    /// Taken by `resumed` to create the window.
    attributes: Option<WindowAttributes>,
    /// The window, once `resumed` has created it.
    window: Option<winit::window::Window>,
    /// Why creating the window failed, if it did.
    create_error: Option<String>,
    /// Client size in physical pixels, the last non-zero size reported.
    client: (u32, u32),
    minimized: bool,
    cursor_visible: bool,
    mode: WindowMode,
    /// Where the cursor is held (mouse-look). Tracked so losing focus can release it and
    /// gaining focus take it again.
    cursor_clip: CursorClip,
    /// Whether Command or Control is down: typing is then a shortcut, not text.
    shortcut_held: bool,
    /// Set when winit asked the loop to exit (Cmd+Q, the app being terminated).
    exit_requested: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let Some(attributes) = self.attributes.take() else {
            return;
        };
        match event_loop.create_window(attributes) {
            Ok(window) => self.window = Some(window),
            Err(error) => self.create_error = Some(error.to_string()),
        }
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WinitEvent) {
        match event {
            WinitEvent::Resized(size) => self.on_resized(size),
            WinitEvent::CloseRequested => {
                // The application decides whether to honour the request, so the window is
                // not closed here.
                self.events.push(WindowEvent::CloseRequested);
            }
            WinitEvent::Focused(focused) => {
                if self.cursor_clip != CursorClip::Free {
                    // Never leave the pointer trapped in a window that is no longer in
                    // front; take the clip again when the application comes back.
                    let clip = if focused {
                        self.cursor_clip
                    } else {
                        CursorClip::Free
                    };
                    if let Some(window) = &self.window {
                        apply_cursor_clip(window, clip);
                    }
                }
                self.events
                    .push(WindowEvent::Input(InputEvent::FocusChanged { focused }));
            }
            WinitEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.shortcut_held = state.super_key() || state.control_key();
            }
            WinitEvent::KeyboardInput { event, .. } => self.on_key(&event),
            WinitEvent::CursorMoved { position, .. } => {
                self.events.push(WindowEvent::Input(InputEvent::MouseMoved {
                    x: position.x as i32,
                    y: position.y as i32,
                }));
            }
            WinitEvent::MouseInput { state, button, .. } => {
                let Some(button) = mouse_button(button) else {
                    return;
                };
                self.events.push(WindowEvent::Input(match state {
                    ElementState::Pressed => InputEvent::MouseButtonDown { button },
                    ElementState::Released => InputEvent::MouseButtonUp { button },
                }));
            }
            WinitEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => {
                        (y / PIXELS_PER_NOTCH) as f32
                    }
                };
                if notches != 0.0 {
                    self.events.push(WindowEvent::Input(InputEvent::MouseWheel {
                        delta: notches,
                    }));
                }
            }
            // A different backing scale (dragged to another monitor, display settings
            // changed) is a display configuration change; winit follows it with `Resized`.
            WinitEvent::ScaleFactorChanged { .. } => self.events.push(WindowEvent::DisplayChanged),
            WinitEvent::Occluded(occluded) => self.on_occluded(occluded),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        // Raw relative motion: it arrives whether or not the cursor can move, which is what
        // lets a locked cursor still turn the camera.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            let (dx, dy) = (dx.round() as i32, dy.round() as i32);
            if (dx, dy) != (0, 0) {
                self.events
                    .push(WindowEvent::Input(InputEvent::MouseDelta { dx, dy }));
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.exit_requested = true;
    }
}

impl App {
    /// Records a new client size and reports it, unless it is empty or unchanged.
    fn on_resized(&mut self, size: PhysicalSize<u32>) {
        let (width, height) = (size.width, size.height);
        if width > 0 && height > 0 && (width, height) != self.client {
            self.client = (width, height);
            if !self.minimized {
                self.events.push(WindowEvent::Resized { width, height });
            }
        }
    }

    /// Turns AppKit's occlusion notice into minimize/restore.
    ///
    /// AppKit says "occluded" for a window that is hidden, fully covered by another window,
    /// or on another Space, as well as one that is minimized into the Dock. Only the last is
    /// a minimize: a hidden window must still render its first frame (the application shows
    /// the window only once it has one), and a covered one is still on screen as far as the
    /// player is concerned.
    fn on_occluded(&mut self, occluded: bool) {
        let Some(window) = &self.window else {
            return;
        };
        if occluded && !self.minimized && window.is_minimized() == Some(true) {
            self.minimized = true;
            self.events.push(WindowEvent::Minimized);
        } else if !occluded && self.minimized {
            self.minimized = false;
            self.events.push(WindowEvent::Restored);
            // Windows reports the restored size as a resize, because its client area was
            // zero while minimized; the application relies on it to size its swap chain.
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                self.client = (size.width, size.height);
                self.events.push(WindowEvent::Resized {
                    width: size.width,
                    height: size.height,
                });
            }
        }
    }

    fn on_key(&mut self, event: &KeyEvent) {
        let key = match event.physical_key {
            PhysicalKey::Code(code) => key_from_winit(code),
            PhysicalKey::Unidentified(_) => KeyCode::Unknown,
        };
        match event.state {
            ElementState::Pressed => {
                self.events.push(WindowEvent::key_down(key, event.repeat));
                // Text, as Windows' WM_CHAR delivers it. Not while a shortcut modifier is
                // down: Cmd+V must paste, not type a "v".
                if !self.shortcut_held
                    && let Some(text) = &event.text
                {
                    for character in text.chars() {
                        // Control characters and AppKit's private-use function-key codes
                        // (arrows, F-keys) are keys, not text.
                        if character.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&character)
                        {
                            continue;
                        }
                        self.events
                            .push(WindowEvent::Input(InputEvent::Text { character }));
                    }
                }
            }
            ElementState::Released => self.events.push(WindowEvent::key_up(key)),
        }
    }
}

/// Maps a winit mouse button to the engine's, or `None` for one the engine has no name for.
fn mouse_button(button: WinitButton) -> Option<MouseButton> {
    match button {
        WinitButton::Left => Some(MouseButton::Left),
        WinitButton::Right => Some(MouseButton::Right),
        WinitButton::Middle => Some(MouseButton::Middle),
        WinitButton::Back => Some(MouseButton::Back),
        WinitButton::Forward => Some(MouseButton::Forward),
        WinitButton::Other(_) => None,
    }
}

/// Maps a winit physical key to a [`KeyCode`], with the same results as the Windows
/// virtual-key table ([`crate::key_from_virtual_code`]).
///
/// Physical rather than logical keys, so WASD stays where the fingers are on any keyboard
/// layout. The number pad's digits, `-` and `.` are the same keys as the main block's, as on
/// Windows. Command is reported as Control (see below); unnamed keys become
/// [`KeyCode::Unknown`] rather than being dropped.
///
/// Command is mapped to Control because the engine's text-field shortcuts (select all, copy,
/// paste) are Control combinations and a Mac user's fingers press Command. The cost is that
/// Command also triggers any Control binding.
#[must_use]
fn key_from_winit(code: Key) -> KeyCode {
    match code {
        Key::KeyA => KeyCode::A,
        Key::KeyB => KeyCode::B,
        Key::KeyC => KeyCode::C,
        Key::KeyD => KeyCode::D,
        Key::KeyE => KeyCode::E,
        Key::KeyF => KeyCode::F,
        Key::KeyG => KeyCode::G,
        Key::KeyH => KeyCode::H,
        Key::KeyI => KeyCode::I,
        Key::KeyJ => KeyCode::J,
        Key::KeyK => KeyCode::K,
        Key::KeyL => KeyCode::L,
        Key::KeyM => KeyCode::M,
        Key::KeyN => KeyCode::N,
        Key::KeyO => KeyCode::O,
        Key::KeyP => KeyCode::P,
        Key::KeyQ => KeyCode::Q,
        Key::KeyR => KeyCode::R,
        Key::KeyS => KeyCode::S,
        Key::KeyT => KeyCode::T,
        Key::KeyU => KeyCode::U,
        Key::KeyV => KeyCode::V,
        Key::KeyW => KeyCode::W,
        Key::KeyX => KeyCode::X,
        Key::KeyY => KeyCode::Y,
        Key::KeyZ => KeyCode::Z,
        Key::Digit0 | Key::Numpad0 => KeyCode::Digit0,
        Key::Digit1 | Key::Numpad1 => KeyCode::Digit1,
        Key::Digit2 | Key::Numpad2 => KeyCode::Digit2,
        Key::Digit3 | Key::Numpad3 => KeyCode::Digit3,
        Key::Digit4 | Key::Numpad4 => KeyCode::Digit4,
        Key::Digit5 | Key::Numpad5 => KeyCode::Digit5,
        Key::Digit6 | Key::Numpad6 => KeyCode::Digit6,
        Key::Digit7 | Key::Numpad7 => KeyCode::Digit7,
        Key::Digit8 | Key::Numpad8 => KeyCode::Digit8,
        Key::Digit9 | Key::Numpad9 => KeyCode::Digit9,
        Key::F1 => KeyCode::F1,
        Key::F2 => KeyCode::F2,
        Key::F3 => KeyCode::F3,
        Key::F4 => KeyCode::F4,
        Key::F5 => KeyCode::F5,
        Key::F6 => KeyCode::F6,
        Key::F7 => KeyCode::F7,
        Key::F8 => KeyCode::F8,
        Key::F9 => KeyCode::F9,
        Key::F10 => KeyCode::F10,
        Key::F11 => KeyCode::F11,
        Key::F12 => KeyCode::F12,
        Key::Space => KeyCode::Space,
        Key::Escape => KeyCode::Escape,
        // The Mac Return key and the keypad's Enter are both Enter, as VK_RETURN is on
        // Windows for both.
        Key::Enter | Key::NumpadEnter => KeyCode::Enter,
        Key::Tab => KeyCode::Tab,
        // The key a Mac labels "delete" is the one Windows calls Backspace.
        Key::Backspace => KeyCode::Backspace,
        Key::ArrowLeft => KeyCode::ArrowLeft,
        Key::ArrowRight => KeyCode::ArrowRight,
        Key::ArrowUp => KeyCode::ArrowUp,
        Key::ArrowDown => KeyCode::ArrowDown,
        Key::ShiftLeft => KeyCode::LeftShift,
        Key::ShiftRight => KeyCode::RightShift,
        Key::ControlLeft | Key::SuperLeft => KeyCode::LeftControl,
        Key::ControlRight | Key::SuperRight => KeyCode::RightControl,
        Key::AltLeft => KeyCode::LeftAlt,
        Key::AltRight => KeyCode::RightAlt,
        Key::Minus | Key::NumpadSubtract => KeyCode::Minus,
        Key::Period | Key::NumpadDecimal => KeyCode::Period,
        // Forward delete (fn+delete, or the extended keyboard's Del).
        Key::Delete => KeyCode::Delete,
        Key::Home => KeyCode::Home,
        _ => KeyCode::Unknown,
    }
}

/// Applies `clip` to the cursor through winit's grab modes.
///
/// - [`CursorClip::Free`] releases the grab.
/// - [`CursorClip::Pinned`] is `Locked`: the cursor stops moving and motion arrives as raw
///   deltas, which is precisely the Windows behaviour of a one-pixel clip rectangle plus raw
///   input. The pinned point is not applied: a locked cursor stays where it already is,
///   which is where the application pinned it (the button-down point).
/// - [`CursorClip::Client`] asks for `Confined`; macOS has no such mode in winit, so it
///   falls back to `Locked`. `Client` is only used by the application when raw mouse input
///   is unavailable, which never happens here, so the fallback is a safety net rather than
///   a path anyone takes.
fn apply_cursor_clip(window: &winit::window::Window, clip: CursorClip) {
    let result = match clip {
        CursorClip::Free => window.set_cursor_grab(CursorGrabMode::None),
        CursorClip::Pinned(..) => window.set_cursor_grab(CursorGrabMode::Locked),
        CursorClip::Client => window
            .set_cursor_grab(CursorGrabMode::Confined)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked)),
    };
    if let Err(error) = result {
        log::warn!("cursor grab {clip:?} failed: {error}");
    }
}

/// A native macOS window.
///
/// Owns its winit window and the event loop that feeds it; both go away on drop.
pub struct Window {
    // Declared before the event loop so the window is destroyed first.
    app: App,
    event_loop: EventLoop<()>,
    /// Whether showing this window should also focus it.
    ///
    /// Kept here rather than passed to [`Window::set_visible`] because it is a property of
    /// the window for its whole life, as on Windows.
    activate: bool,
    quit_received: bool,
}

impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window")
            .field("client", &self.app.client)
            .field("mode", &self.app.mode)
            .field("minimized", &self.app.minimized)
            .finish_non_exhaustive()
    }
}

impl Window {
    /// Creates and (unless hidden) shows a window with the requested client size.
    ///
    /// Must be called on the main thread, which is where AppKit allows windows.
    ///
    /// winit creates a window only once the application has finished launching, which it
    /// signals from inside the first pump; this function therefore pumps until the window
    /// exists, so the caller gets a finished window exactly as on Windows.
    ///
    /// A `fullscreen` window asks for borderless fullscreen on the current monitor. macOS
    /// performs that as an animated move to its own Space, so the window opens at its
    /// requested size and the fullscreen size arrives as a [`WindowEvent::Resized`] a few
    /// frames later; [`Window::client_size`] is the size at creation until then.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] if `config` is not usable, or
    /// [`PlatformError::Native`] if the event loop or the window cannot be created.
    pub fn new(config: &WindowConfig) -> Result<Self, PlatformError> {
        config.validate()?;

        let mut builder = EventLoop::builder();
        // Whether starting the game may take the keyboard away from the program it was
        // started from. The same intent as `WindowConfig::activate` on Windows.
        builder.with_activate_ignoring_other_apps(config.activate);
        let event_loop = builder.build().map_err(|error| PlatformError::Native {
            call: "create event loop",
            message: error.to_string(),
        })?;

        let mut attributes = WindowAttributes::default()
            .with_title(config.title.clone())
            .with_inner_size(PhysicalSize::new(config.width, config.height))
            .with_resizable(config.resizable)
            .with_visible(config.visible)
            .with_active(config.activate)
            // Hide the menu bar and the Dock in fullscreen: this is a game.
            .with_borderless_game(true);
        if config.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }

        let mut window = Self {
            app: App {
                events: EventQueue::new(),
                attributes: Some(attributes),
                window: None,
                create_error: None,
                client: (config.width, config.height),
                minimized: false,
                cursor_visible: true,
                mode: if config.fullscreen {
                    WindowMode::Borderless
                } else {
                    WindowMode::Windowed
                },
                cursor_clip: CursorClip::Free,
                shortcut_held: false,
                exit_requested: false,
            },
            event_loop,
            activate: config.activate,
            quit_received: false,
        };

        let started = Instant::now();
        while window.app.window.is_none() && window.app.create_error.is_none() {
            if started.elapsed() > LAUNCH_TIMEOUT {
                return Err(PlatformError::Native {
                    call: "create window",
                    message: "the application did not finish launching".to_owned(),
                });
            }
            window.pump_once();
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(message) = window.app.create_error.take() {
            return Err(PlatformError::Native {
                call: "create window",
                message,
            });
        }
        if let Some(created) = &window.app.window {
            let size = created.inner_size();
            if size.width > 0 && size.height > 0 {
                window.app.client = (size.width, size.height);
            }
        }
        // Events from launch (focus, the initial resize) describe a window that did not
        // exist yet as far as the caller is concerned; the size above already includes them.
        window.app.events = EventQueue::new();

        log::info!(
            "window created: {}x{} client, resizable={}, visible={}, activate={}, fullscreen={}",
            window.app.client.0,
            window.app.client.1,
            config.resizable && !config.fullscreen,
            config.visible,
            config.activate,
            config.fullscreen
        );

        Ok(window)
    }

    /// One non-blocking pump of the winit loop.
    fn pump_once(&mut self) {
        match self
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut self.app)
        {
            PumpStatus::Continue => {}
            PumpStatus::Exit(_) => self.quit_received = true,
        }
        if self.app.exit_requested {
            self.quit_received = true;
        }
    }

    /// Drains the OS event queue into the internal event queue.
    ///
    /// Non-blocking: returns as soon as no more events are pending, so the caller controls
    /// the frame pace.
    pub fn pump_events(&mut self) {
        self.pump_once();
    }

    /// Removes and returns the oldest buffered event.
    pub fn poll_event(&mut self) -> Option<WindowEvent> {
        self.app.events.pop()
    }

    /// Returns `true` once the application was asked to quit (Command+Q, the Dock's Quit)
    /// and winit's loop exited, as seen by [`Window::pump_events`].
    #[must_use]
    pub fn quit_received(&self) -> bool {
        self.quit_received
    }

    /// Current client-area size in physical pixels. `(0, 0)` while minimized.
    #[must_use]
    pub fn client_size(&self) -> (u32, u32) {
        if self.app.minimized {
            (0, 0)
        } else {
            self.app.client
        }
    }

    /// Whether the window is currently minimized into the Dock.
    #[must_use]
    pub fn is_minimized(&self) -> bool {
        self.app.minimized
    }

    /// The window's `NSView` pointer, as an integer.
    ///
    /// The renderer takes this opaque value so it never depends on platform types. On macOS
    /// it attaches a `CAMetalLayer` to this view and presents into it; the Windows
    /// equivalent is the `HWND` a swap chain is built on. The view lives as long as this
    /// [`Window`]. `0` is returned only if AppKit gives no handle, which a live window never
    /// does.
    #[must_use]
    pub fn raw_handle(&self) -> isize {
        let Some(window) = &self.app.window else {
            return 0;
        };
        match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr() as isize,
            _ => 0,
        }
    }

    fn native(&self) -> &winit::window::Window {
        // `new` does not return until the window exists and nothing removes it afterwards.
        // The fallible accessor keeps that invariant out of the type's public surface.
        #[allow(clippy::expect_used, reason = "invariant established by Window::new")]
        self.app
            .window
            .as_ref()
            .expect("the window exists from Window::new onwards")
    }

    /// Replaces the title-bar text.
    ///
    /// # Errors
    ///
    /// Never fails on macOS; the signature matches the Windows window, where setting the
    /// title can.
    pub fn set_title(&self, title: &str) -> Result<(), PlatformError> {
        self.native().set_title(title);
        Ok(())
    }

    /// Shows or hides the mouse cursor.
    ///
    /// Idempotent. A hidden cursor stays hidden over this window only; anything that hides
    /// it must show it again before the window loses focus.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.app.cursor_visible == visible {
            return;
        }
        self.app.cursor_visible = visible;
        self.native().set_cursor_visible(visible);
    }

    /// Restricts where the cursor may go, or releases it with [`CursorClip::Free`].
    ///
    /// The grab is released while the window is not in front so the pointer is never
    /// trapped. See [`CursorClip`] and the module's grab mapping for how each clip maps to
    /// winit's `Locked`/`Confined` modes.
    pub fn set_cursor_clip(&mut self, clip: CursorClip) {
        self.app.cursor_clip = clip;
        apply_cursor_clip(self.native(), clip);
    }

    /// Whether raw mouse motion is delivered as [`InputEvent::MouseDelta`].
    ///
    /// Always true on macOS: winit delivers raw motion as device events whether or not the
    /// cursor can move, so mouse-look can pin the cursor.
    #[must_use]
    pub fn raw_mouse(&self) -> bool {
        true
    }

    /// Whether the cursor is currently shown.
    #[must_use]
    pub fn cursor_visible(&self) -> bool {
        self.app.cursor_visible
    }

    /// Warps the cursor to the centre of the client area.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Native`] if the cursor cannot be moved.
    pub fn center_cursor(&self) -> Result<(), PlatformError> {
        let (x, y) = self.client_center();
        self.set_cursor_position(x, y)
    }

    /// Warps the cursor to a position in the client area, in client pixels.
    ///
    /// winit re-associates the mouse with the cursor when it warps, which would undo a
    /// `Locked` grab; the grab is therefore applied again afterwards if one is in force.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Native`] if the cursor cannot be moved.
    pub fn set_cursor_position(&self, x: i32, y: i32) -> Result<(), PlatformError> {
        let window = self.native();
        window
            .set_cursor_position(PhysicalPosition::new(f64::from(x), f64::from(y)))
            .map_err(|error| PlatformError::Native {
                call: "set cursor position",
                message: error.to_string(),
            })?;
        if self.app.cursor_clip != CursorClip::Free {
            apply_cursor_clip(window, self.app.cursor_clip);
        }
        Ok(())
    }

    /// The client-area centre in client pixels.
    ///
    /// The reference point a mouse-look camera measures its per-frame offset against.
    #[must_use]
    pub fn client_center(&self) -> (i32, i32) {
        let (width, height) = self.client_size();
        ((width / 2) as i32, (height / 2) as i32)
    }

    /// Shows or hides the window.
    ///
    /// Showing respects the configuration's `activate` where AppKit allows: AppKit has no
    /// "show without making key" for an ordinary window, so a window created not to steal
    /// focus may still become key when shown. That is a development convenience only.
    pub fn set_visible(&self, visible: bool) {
        self.native().set_visible(visible);
        if visible && self.activate {
            self.native().focus_window();
        }
    }

    /// Whether showing this window also gives it the keyboard focus.
    #[must_use]
    pub fn activates(&self) -> bool {
        self.activate
    }

    /// Resizes the client area to `width` x `height`, in physical pixels.
    ///
    /// The window keeps its position and focus. As on Windows the resize is a request: the
    /// next [`Window::pump_events`] reports it as a [`WindowEvent::Resized`] (AppKit may
    /// clamp it, for example to the screen when the window is on one), and the application's
    /// own resize path recreates whatever depends on the size.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] for a zero dimension.
    pub fn set_client_size(&mut self, width: u32, height: u32) -> Result<(), PlatformError> {
        if width == 0 || height == 0 {
            return Err(PlatformError::InvalidConfig {
                reason: format!("a window cannot have a {width}x{height} client area"),
            });
        }
        // winit 0.30 applies the size synchronously on macOS (`setContentSize`) but returns
        // `None` and sends no `Resized` for it, so read the size back and report it now.
        // `on_resized` ignores an unchanged size, so a later `Resized` is not reported twice.
        let size = self
            .native()
            .request_inner_size(PhysicalSize::new(width, height))
            .unwrap_or_else(|| self.native().inner_size());
        self.app.on_resized(size);
        Ok(())
    }

    /// How the window covers the screen now.
    #[must_use]
    pub fn window_mode(&self) -> WindowMode {
        self.app.mode
    }

    /// The monitor the window is on.
    #[must_use]
    pub fn monitor(&self) -> Option<MonitorInfo> {
        self.native()
            .current_monitor()
            .and_then(|monitor| display::monitor_for_display_id(monitor.native_id()))
            .or_else(display::primary_monitor)
    }

    /// Puts the window in `mode`, at a client size of `width` x `height` where the mode
    /// takes one, on the monitor it is on now.
    ///
    /// - **Windowed**: a framed window with that client size, centred on its monitor's work
    ///   area.
    /// - **Borderless** and **Fullscreen**: macOS's borderless fullscreen on that monitor
    ///   (its own Space, menu bar and Dock hidden); the size is not used.
    ///
    /// There is no exclusive video-mode switch on macOS, so a `Fullscreen` request is
    /// answered with `Borderless` and the returned [`AppliedDisplay`] says so, the same
    /// honest fallback the Windows window reports when a monitor refuses a mode. Switching
    /// the display's mode would capture the display, break Mission Control and the Dock, and
    /// flicker every other application's windows; at the desktop's native mode a borderless
    /// window is also what the compositor can flip directly, so nothing is lost.
    ///
    /// Entering and leaving fullscreen is animated by the system and finishes after this
    /// returns; [`WindowEvent::Resized`] reports the size when it does. The returned
    /// [`AppliedDisplay`] is therefore the monitor's size for borderless, not a measurement
    /// of the window yet.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] if the size is zero.
    pub fn apply_display(
        &mut self,
        mode: WindowMode,
        width: u32,
        height: u32,
    ) -> Result<AppliedDisplay, PlatformError> {
        if width == 0 || height == 0 {
            return Err(PlatformError::InvalidConfig {
                reason: format!("a {width}x{height} display is not a display"),
            });
        }
        let monitor = self.monitor().ok_or_else(|| PlatformError::InvalidConfig {
            reason: "the window is on no monitor".to_owned(),
        })?;
        let applied = match mode {
            WindowMode::Windowed => self.become_windowed(&monitor, width, height),
            WindowMode::Borderless => self.become_borderless(&monitor),
            WindowMode::Fullscreen => {
                log::warn!(
                    "display: exclusive fullscreen is not used on macOS; borderless instead"
                );
                self.become_borderless(&monitor)
            }
        };
        log::info!(
            "display: {:?} at {}x{} on {}",
            applied.mode,
            applied.width,
            applied.height,
            monitor.device
        );
        Ok(applied)
    }

    /// Fits the window to its monitor again after the display configuration changed under
    /// it: a borderless window to the monitor's new size. A framed window needs nothing:
    /// AppKit keeps windows on a display that still exists.
    pub fn refit_after_display_change(&mut self) {
        if self.window_mode() == WindowMode::Borderless
            && let Some(monitor) = self.monitor()
        {
            self.become_borderless(&monitor);
        }
    }

    fn become_windowed(
        &mut self,
        monitor: &MonitorInfo,
        width: u32,
        height: u32,
    ) -> AppliedDisplay {
        let window = self.native();
        window.set_fullscreen(None);
        let frame = {
            // The frame around the client area, from the window as it is. Zero straight
            // after leaving fullscreen, when there is no title bar to measure yet; the
            // window is then placed a title bar too high, never off screen.
            let outer = window.outer_size();
            let inner = window.inner_size();
            (
                outer.width.saturating_sub(inner.width) as i32,
                outer.height.saturating_sub(inner.height) as i32,
            )
        };
        let _ = window.request_inner_size(PhysicalSize::new(width, height));
        // Centred on the work area, and never starting above or left of it, so the title bar
        // is on screen even when the window is larger than the monitor.
        let work = monitor.work_area;
        let outer_width = width as i32 + frame.0;
        let outer_height = height as i32 + frame.1;
        let x = work.x + ((work.width as i32 - outer_width) / 2).max(0);
        let y = work.y + ((work.height as i32 - outer_height) / 2).max(0);
        window.set_outer_position(PhysicalPosition::new(x, y));
        self.app.mode = WindowMode::Windowed;
        AppliedDisplay {
            mode: WindowMode::Windowed,
            width,
            height,
        }
    }

    fn become_borderless(&mut self, monitor: &MonitorInfo) -> AppliedDisplay {
        let window = self.native();
        let handle = window.current_monitor();
        window.set_fullscreen(Some(Fullscreen::Borderless(handle)));
        self.app.mode = WindowMode::Borderless;
        AppliedDisplay {
            mode: WindowMode::Borderless,
            width: monitor.bounds.width,
            height: monitor.bounds.height,
        }
    }

    /// The window's outer rectangle on the virtual desktop, in physical pixels: left, top,
    /// width, height.
    #[must_use]
    pub fn outer_rect(&self) -> (i32, i32, u32, u32) {
        let window = self.native();
        let position = window.outer_position().unwrap_or_default();
        let size = window.outer_size();
        (position.x, position.y, size.width, size.height)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // The cursor and its grab are process-wide: a window that hid or locked them and
        // then went away would leave the desktop without a usable pointer.
        if let Some(window) = &self.app.window {
            window.set_cursor_visible(true);
            apply_cursor_clip(window, CursorClip::Free);
        }
    }
}

/// Queries the live client size straight from winit.
///
/// Used by tests and diagnostics to confirm the cached size matches reality.
///
/// # Errors
///
/// Never fails on macOS; the signature matches the Windows version, where the query can.
pub fn query_client_size(window: &Window) -> Result<(u32, u32), PlatformError> {
    let size = window.native().inner_size();
    Ok((size.width, size.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys_map_like_windows() {
        assert_eq!(key_from_winit(Key::KeyA), KeyCode::A);
        assert_eq!(key_from_winit(Key::KeyW), KeyCode::W);
        assert_eq!(key_from_winit(Key::KeyZ), KeyCode::Z);
        assert_eq!(key_from_winit(Key::Digit0), KeyCode::Digit0);
        assert_eq!(key_from_winit(Key::Digit9), KeyCode::Digit9);
        assert_eq!(key_from_winit(Key::F1), KeyCode::F1);
        assert_eq!(key_from_winit(Key::F12), KeyCode::F12);
    }

    #[test]
    fn the_number_pad_is_the_main_block() {
        assert_eq!(key_from_winit(Key::Numpad7), key_from_winit(Key::Digit7));
        assert_eq!(key_from_winit(Key::NumpadDecimal), KeyCode::Period);
        assert_eq!(key_from_winit(Key::NumpadSubtract), KeyCode::Minus);
    }

    #[test]
    fn command_is_control_and_unnamed_keys_are_unknown() {
        assert_eq!(key_from_winit(Key::SuperLeft), KeyCode::LeftControl);
        assert_eq!(key_from_winit(Key::SuperRight), KeyCode::RightControl);
        assert_eq!(key_from_winit(Key::F13), KeyCode::Unknown);
        assert_eq!(key_from_winit(Key::CapsLock), KeyCode::Unknown);
    }

    #[test]
    fn only_named_mouse_buttons_map() {
        assert_eq!(mouse_button(WinitButton::Back), Some(MouseButton::Back));
        assert_eq!(
            mouse_button(WinitButton::Forward),
            Some(MouseButton::Forward)
        );
        assert_eq!(mouse_button(WinitButton::Other(7)), None);
    }
}
