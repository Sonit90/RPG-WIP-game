//! Win32 window creation and message pumping.
//!
//! All `unsafe` code in this crate lives here. Everything the window procedure needs
//! is stored in a [`WindowState`] that is owned by [`Window`] and reached from the
//! procedure through `GWLP_USERDATA`.

use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, ClientToScreen, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
/// The largest window edge, in pixels, that [`Window::set_client_size`] may ask for.
///
/// 16384 is D3D12's own maximum texture dimension at feature level 11_0, so a window this
/// wide is the widest one whose swap chain could exist. It is a ceiling rather than a target:
/// nothing asks for it, and its only job is to be larger than any resolution a benchmark
/// names.
const MAX_WINDOW_EDGE: i32 = 16384;

use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE, GWL_STYLE,
    GWLP_USERDATA, GetClientRect, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, HWND_TOP,
    IDC_ARROW, IsIconic, IsWindowVisible, LoadCursorW, MINMAXINFO, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, RegisterClassExW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED, SM_CXSCREEN,
    SM_CYSCREEN, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, SW_SHOWNORMAL,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetCursorPos,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowCursor, ShowWindow, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATEAPP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_QUIT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SIZE, WM_CHAR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
    WM_XBUTTONUP, WNDCLASSEXW, WS_OVERLAPPED, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU,
    WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

use engine_core::input::{InputEvent, MouseButton};

use crate::config::{AppliedDisplay, WindowConfig, WindowMode};
use crate::display::{self, MonitorInfo, VideoMode};
use crate::error::PlatformError;
use crate::event::{
    EventQueue, WindowEvent, cursor_from_lparam, key_from_virtual_code, size_from_lparam,
    wheel_from_wparam, x_button_from_wparam,
};

/// Window class name. One class is enough for the whole process.
const CLASS_NAME: PCWSTR = w!("myGameWindowClass");

/// Result of registering the window class, computed once per process.
static CLASS_REGISTRATION: OnceLock<Result<(), windows::core::Error>> = OnceLock::new();

/// Mutable state shared between [`Window`] and its window procedure.
///
/// Boxed and kept alive by `Window` for exactly as long as the `HWND` exists.
#[derive(Debug)]
struct WindowState {
    events: EventQueue,
    client_width: u32,
    client_height: u32,
    minimized: bool,
    /// Whether the cursor is currently shown.
    ///
    /// Tracked because `ShowCursor` maintains a *counter*, not a flag: calling it twice to
    /// hide and once to show leaves the cursor hidden. Keeping the desired state here means
    /// the counter only ever moves between 0 and -1.
    cursor_visible: bool,
    /// Where the last pointer message said the cursor was, so a button message only reports
    /// a position when it is news.
    last_cursor: Option<(i32, i32)>,
    /// How the window covers the screen now.
    mode: WindowMode,
    /// The monitor mode this window switched to, while it is in exclusive fullscreen.
    exclusive: Option<Exclusive>,
    /// Whether a framed window may be resized by its edges.
    resizable: bool,
    /// The first half of a UTF-16 surrogate pair `WM_CHAR` delivered, waiting for the second.
    high_surrogate: Option<u16>,
}

/// An exclusive fullscreen switch this window made, and whether it is undone for now.
#[derive(Debug, Clone)]
struct Exclusive {
    /// Which monitor was switched.
    device: String,
    /// What it was switched to.
    mode: VideoMode,
    /// Whether the switch is undone while the game is not in front (alt-tab), to be made
    /// again when it comes back.
    suspended: bool,
}

/// A native Windows window.
///
/// Owns its `HWND` and destroys it on drop.
#[derive(Debug)]
pub struct Window {
    hwnd: HWND,
    /// Owning pointer to the state the window procedure writes into.
    /// Reclaimed in [`Window::drop`] after the `HWND` is destroyed.
    state: *mut WindowState,
    /// Whether showing this window should also focus it.
    ///
    /// Kept here rather than passed to [`Window::set_visible`] because it is a property of
    /// the window for its whole life: an application that asked not to steal focus at
    /// startup does not want to steal it when it shows the window later either.
    activate: bool,
    quit_received: bool,
}

impl Window {
    /// Creates and (unless hidden) shows a window with the requested client size.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] if `config` is not usable, or
    /// [`PlatformError::Win32`] if a Windows API call fails.
    pub fn new(config: &WindowConfig) -> Result<Self, PlatformError> {
        config.validate()?;

        let instance = module_handle()?;
        ensure_class_registered(instance)?;

        // Borderless fullscreen is a popup window the size of the primary monitor: the
        // client area is then exactly the outer size, with no border to subtract.
        let (style, position, outer, client) = if config.fullscreen {
            let (width, height) = primary_monitor_size();
            log::info!("borderless fullscreen: primary monitor is {width}x{height}");
            (
                WS_POPUP,
                (0, 0),
                (width as i32, height as i32),
                (width, height),
            )
        } else {
            let style = if config.resizable {
                WS_OVERLAPPEDWINDOW
            } else {
                WS_OVERLAPPED | WS_SYSMENU
            };
            let outer = outer_size(
                config.width,
                config.height,
                style,
                WINDOW_EX_STYLE::default(),
            )?;
            (
                style,
                (CW_USEDEFAULT, CW_USEDEFAULT),
                outer,
                (config.width, config.height),
            )
        };
        let ex_style = WINDOW_EX_STYLE::default();

        // The state is created before the window because WM_NCCREATE, which stores the
        // pointer in GWLP_USERDATA, is delivered from inside CreateWindowExW.
        let state = Box::into_raw(Box::new(WindowState {
            events: EventQueue::new(),
            last_cursor: None,
            high_surrogate: None,
            client_width: client.0,
            client_height: client.1,
            minimized: false,
            cursor_visible: true,
            mode: if config.fullscreen {
                WindowMode::Borderless
            } else {
                WindowMode::Windowed
            },
            exclusive: None,
            resizable: config.resizable,
        }));

        let title = HSTRING::from(config.title.as_str());

        // SAFETY: CreateWindowExW is an FFI call. The class is registered above, the
        // title outlives the call, and `state` is a valid unique pointer that the window
        // procedure only stores; ownership stays with this function until `Window` takes
        // it over below. On failure the box is reclaimed before returning.
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                CLASS_NAME,
                PCWSTR(title.as_ptr()),
                style,
                position.0,
                position.1,
                outer.0,
                outer.1,
                None,
                None,
                Some(instance),
                Some(state.cast()),
            )
        };

        let hwnd = match hwnd {
            Ok(hwnd) => hwnd,
            Err(error) => {
                // SAFETY: the window was never created, so no procedure can observe the
                // pointer any more; this call reclaims the box created just above.
                drop(unsafe { Box::from_raw(state) });
                return Err(PlatformError::win32("CreateWindowExW", error));
            }
        };

        let window = Self {
            hwnd,
            state,
            activate: config.activate,
            quit_received: false,
        };

        let command = match (config.visible, config.activate) {
            (false, _) => SW_HIDE,
            (true, true) => SW_SHOWNORMAL,
            // Shown, but the keyboard stays wherever it was. `SW_SHOWNOACTIVATE` rather
            // than the `WS_EX_NOACTIVATE` style: the style would stop the window taking
            // focus when it is *clicked* too, which would make the game unplayable rather
            // than unobtrusive.
            (true, false) => SW_SHOWNOACTIVATE,
        };
        // SAFETY: `hwnd` is a live window owned by `window`. ShowWindow's BOOL result
        // reports the previous visibility, not success, so it is intentionally ignored.
        let _ = unsafe { ShowWindow(hwnd, command) };

        log::info!(
            "window created: {}x{} client, resizable={}, visible={}, activate={}, fullscreen={}",
            client.0,
            client.1,
            config.resizable && !config.fullscreen,
            config.visible,
            config.activate,
            config.fullscreen
        );

        Ok(window)
    }

    /// Drains the OS message queue into the internal event queue.
    ///
    /// Non-blocking: returns as soon as no more messages are pending, so the caller
    /// controls the frame pace. Performs no heap allocation in the common case.
    pub fn pump_events(&mut self) {
        let mut msg = MSG::default();
        loop {
            // SAFETY: `msg` is a valid, owned message struct. Passing `None` for the
            // window filter also delivers thread messages such as WM_QUIT.
            let has_message = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool();
            if !has_message {
                break;
            }
            if msg.message == WM_QUIT {
                self.quit_received = true;
                break;
            }
            // SAFETY: `msg` was filled by PeekMessageW immediately above. Dispatch may
            // re-enter this crate's window procedure, which only touches `self.state`.
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Removes and returns the oldest buffered event.
    pub fn poll_event(&mut self) -> Option<WindowEvent> {
        self.state_mut().events.pop()
    }

    /// Returns `true` once a `WM_QUIT` has been seen by [`Window::pump_events`].
    #[must_use]
    pub fn quit_received(&self) -> bool {
        self.quit_received
    }

    /// Current client-area size in pixels. `(0, 0)` while minimized.
    #[must_use]
    pub fn client_size(&self) -> (u32, u32) {
        let state = self.state_ref();
        (state.client_width, state.client_height)
    }

    /// Whether the window is currently minimized.
    #[must_use]
    pub fn is_minimized(&self) -> bool {
        self.state_ref().minimized
    }

    /// Raw `HWND` as an integer.
    ///
    /// The renderer takes this opaque value so it never depends on platform types.
    #[must_use]
    pub fn raw_handle(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Replaces the title-bar text.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Win32`] if `SetWindowTextW` fails.
    pub fn set_title(&self, title: &str) -> Result<(), PlatformError> {
        let title = HSTRING::from(title);
        // SAFETY: `hwnd` is live and `title` outlives the call.
        unsafe { SetWindowTextW(self.hwnd, PCWSTR(title.as_ptr())) }
            .map_err(|error| PlatformError::win32("SetWindowTextW", error))
    }

    /// Shows or hides the mouse cursor.
    ///
    /// Idempotent, which `ShowCursor` on its own is not: it keeps a counter and hides the
    /// cursor while that counter is negative, so two hides need two shows. Calling this
    /// repeatedly with the same value does nothing.
    ///
    /// A hidden cursor is a *process-wide* state on Windows. Anything that hides it must
    /// show it again before the window loses focus or the user is left without a pointer.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.state_ref().cursor_visible == visible {
            return;
        }
        self.state_mut().cursor_visible = visible;
        // SAFETY: FFI call that only adjusts this process's cursor display counter. The
        // returned counter is not needed: the desired state is tracked above.
        let _ = unsafe { ShowCursor(visible) };
    }

    /// Whether the cursor is currently shown.
    #[must_use]
    pub fn cursor_visible(&self) -> bool {
        self.state_ref().cursor_visible
    }

    /// Warps the cursor to the centre of the client area.
    ///
    /// This is what a mouse-look camera does every frame: it reads the offset from the
    /// centre as the frame's rotation, then re-centres so the cursor can never reach an
    /// edge of the screen and stop producing movement.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Win32`] if the client rectangle cannot be read or the
    /// cursor cannot be moved.
    pub fn center_cursor(&self) -> Result<(), PlatformError> {
        let mut rect = RECT::default();
        // SAFETY: `hwnd` is live and `rect` is a valid out-parameter.
        unsafe { GetClientRect(self.hwnd, &mut rect) }
            .map_err(|error| PlatformError::win32("GetClientRect", error))?;

        self.set_cursor_position((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
    }

    /// Warps the cursor to a position in the client area, in client pixels.
    ///
    /// The counterpart of [`Window::center_cursor`]: mouse-look warps the cursor to the
    /// centre while it is held, and puts it back where the player pressed the button when
    /// they let go, so the pointer does not drift towards the middle of the screen every
    /// time the camera is turned.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::Win32`] if the cursor cannot be moved.
    pub fn set_cursor_position(&self, x: i32, y: i32) -> Result<(), PlatformError> {
        let mut point = POINT { x, y };
        // SAFETY: `hwnd` is live and `point` is a valid in/out parameter. The BOOL result
        // only fails for an invalid window, which the next call reports anyway.
        let _ = unsafe { ClientToScreen(self.hwnd, &mut point) };

        // SAFETY: FFI call moving the system cursor to a screen coordinate.
        unsafe { SetCursorPos(point.x, point.y) }
            .map_err(|error| PlatformError::win32("SetCursorPos", error))
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
    /// Showing respects the configuration's `activate`: a window created not to steal focus
    /// does not steal it when it is shown later either.
    pub fn set_visible(&self, visible: bool) {
        let command = match (visible, self.activate) {
            (false, _) => SW_HIDE,
            (true, true) => SW_SHOW,
            (true, false) => SW_SHOWNOACTIVATE,
        };
        // SAFETY: `hwnd` is live; the BOOL result is the previous visibility state.
        let _ = unsafe { ShowWindow(self.hwnd, command) };
    }

    /// Whether showing this window also gives it the keyboard focus.
    #[must_use]
    pub fn activates(&self) -> bool {
        self.activate
    }

    /// Resizes the client area to `width` x `height`, in pixels.
    ///
    /// The window keeps its position, its z-order and its focus; only the size changes. The
    /// outer size is computed from the client size through the same `AdjustWindowRectEx`
    /// that sized the window at creation, so the *client* area — which is what the swap
    /// chain matches — really becomes the requested size rather than that size minus a
    /// border.
    ///
    /// This exists for the benchmark: measuring four resolutions used to mean four processes,
    /// four scene loads and four sets of driver warm-up, and the numbers from separate
    /// processes are not quite comparable. A window that can be resized at runtime turns that
    /// into one process that resizes three times.
    ///
    /// The resize is not applied here. Windows delivers `WM_SIZE`, the next
    /// [`Window::pump_events`] turns it into a [`crate::WindowEvent::Resized`], and the
    /// application's own resize path recreates whatever depends on the size — which is the
    /// same path a player dragging the window edge takes, and the reason this is a request
    /// rather than a mutation.
    ///
    /// A hidden window is resized exactly like a visible one: `WM_SIZE` does not depend on
    /// visibility, which is what lets `--hidden --benchmark` mean anything.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] for a zero dimension, or
    /// [`PlatformError::Win32`] if `SetWindowPos` fails.
    pub fn set_client_size(&self, width: u32, height: u32) -> Result<(), PlatformError> {
        if width == 0 || height == 0 {
            return Err(PlatformError::InvalidConfig {
                reason: format!("a window cannot have a {width}x{height} client area"),
            });
        }

        // SAFETY: `hwnd` is live. GWL_STYLE and GWL_EXSTYLE read the window's own styles,
        // which is what the client-to-outer conversion has to be computed against — a
        // borderless fullscreen popup and an ordinary framed window need different margins.
        // SAFETY: FFI reads of a live window's own style words. GWL_STYLE and GWL_EXSTYLE are
        // what the client-to-outer conversion has to be computed against — a borderless
        // fullscreen popup and an ordinary framed window need different margins — and the
        // low 32 bits are the whole of each style value.
        let style = WINDOW_STYLE(unsafe { GetWindowLongPtrW(self.hwnd, GWL_STYLE) } as u32);
        // SAFETY: as above.
        let ex_style = WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) } as u32);
        let (outer_width, outer_height) = outer_size(width, height, style, ex_style)?;

        // SAFETY: FFI call on a live window with no borrowed pointers. SWP_NOMOVE keeps the
        // position, so the zeroed x and y are ignored; SWP_NOACTIVATE keeps a `--background`
        // run from stealing focus the moment the benchmark changes resolution.
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                outer_width,
                outer_height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(|error| PlatformError::win32("SetWindowPos", error))
    }

    /// How the window covers the screen now.
    #[must_use]
    pub fn window_mode(&self) -> WindowMode {
        self.state_ref().mode
    }

    /// The monitor the window is on — the one holding most of it.
    #[must_use]
    pub fn monitor(&self) -> Option<MonitorInfo> {
        display::monitor_of_window(self.hwnd)
    }

    /// Puts the window in `mode`, at a client size of `width` x `height` where the mode
    /// takes one, on the monitor it is on now.
    ///
    /// - **Windowed**: a framed window with that client size, centred on its monitor's work
    ///   area. Any exclusive switch is undone first.
    /// - **Borderless**: a popup covering the monitor at the desktop's own mode; the size is
    ///   not used. Any exclusive switch is undone first.
    /// - **Fullscreen**: the monitor switched to the mode nearest `width` x `height` it
    ///   offers, and a popup covering it. If the monitor refuses, the window falls back to
    ///   borderless rather than to nothing — the returned [`AppliedDisplay`] says so.
    ///
    /// Nothing renderer-side happens here. Windows delivers `WM_SIZE`, the next
    /// [`Window::pump_events`] reports it as [`crate::WindowEvent::Resized`], and the
    /// application resizes its swap chain on the same path a dragged edge takes.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::InvalidConfig`] if the window's monitor cannot be found or
    /// the size is zero, or [`PlatformError::Win32`] if positioning the window fails.
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
        // Leaving exclusive fullscreen — or changing its mode — starts from the desktop's
        // own mode, so the monitor read below describes the desktop and not the old switch.
        if let Some(exclusive) = self.state_mut().exclusive.take() {
            display::restore_mode(&exclusive.device);
        }
        let monitor = self.monitor().ok_or_else(|| PlatformError::InvalidConfig {
            reason: "the window is on no monitor".to_owned(),
        })?;
        let applied = match mode {
            WindowMode::Windowed => self.become_windowed(&monitor, width, height)?,
            WindowMode::Borderless => self.become_borderless(&monitor)?,
            WindowMode::Fullscreen => {
                let wanted = monitor
                    .mode_for(width, height)
                    .or_else(|| nearest_mode(&monitor.modes, width, height));
                match wanted.map(|wanted| (wanted, display::change_mode(&monitor.device, wanted))) {
                    Some((wanted, Ok(()))) => {
                        self.state_mut().exclusive = Some(Exclusive {
                            device: monitor.device.clone(),
                            mode: wanted,
                            suspended: false,
                        });
                        // The monitor's rectangle is the new mode's now.
                        let switched = self.monitor().unwrap_or_else(|| monitor.clone());
                        let mut applied = self.become_borderless(&switched)?;
                        applied.mode = WindowMode::Fullscreen;
                        self.state_mut().mode = WindowMode::Fullscreen;
                        applied
                    }
                    Some((_, Err(error))) => {
                        log::warn!(
                            "display: exclusive fullscreen refused ({error}); borderless instead"
                        );
                        self.become_borderless(&monitor)?
                    }
                    None => {
                        log::warn!(
                            "display: {} lists no modes; borderless instead",
                            monitor.device
                        );
                        self.become_borderless(&monitor)?
                    }
                }
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
    /// it: a borderless window to the monitor's new size, and a framed window that has been
    /// left with no monitor under it back onto one.
    pub fn refit_after_display_change(&mut self) {
        match self.window_mode() {
            WindowMode::Borderless => {
                if let Some(monitor) = self.monitor()
                    && let Err(error) = self.become_borderless(&monitor)
                {
                    log::warn!("display: could not refit the borderless window: {error}");
                }
            }
            WindowMode::Windowed => {
                // SAFETY: FFI call on a live window; DEFAULTTONULL answers whether any
                // monitor holds part of it.
                let on_screen = !unsafe {
                    windows::Win32::Graphics::Gdi::MonitorFromWindow(
                        self.hwnd,
                        windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONULL,
                    )
                }
                .is_invalid();
                if !on_screen && let Some(monitor) = display::primary_monitor() {
                    let (width, height) = self.client_size();
                    let _ = self.become_windowed(&monitor, width.max(640), height.max(360));
                }
            }
            // Exclusive: the switch is this window's own, and alt-tab handles the rest.
            WindowMode::Fullscreen => {}
        }
    }

    fn become_windowed(
        &mut self,
        monitor: &MonitorInfo,
        width: u32,
        height: u32,
    ) -> Result<AppliedDisplay, PlatformError> {
        let style = if self.state_ref().resizable {
            WS_OVERLAPPEDWINDOW
        } else {
            WS_OVERLAPPED | WS_SYSMENU
        };
        let (outer_width, outer_height) =
            outer_size(width, height, style, WINDOW_EX_STYLE::default())?;
        // Centred on the work area, and never starting above or left of it, so the title bar
        // is on screen even when the window is larger than the monitor.
        let work = monitor.work_area;
        let x = work.x + ((work.width as i32 - outer_width) / 2).max(0);
        let y = work.y + ((work.height as i32 - outer_height) / 2).max(0);
        self.restyle(style, (x, y, outer_width, outer_height))?;
        self.state_mut().mode = WindowMode::Windowed;
        Ok(AppliedDisplay {
            mode: WindowMode::Windowed,
            width,
            height,
        })
    }

    fn become_borderless(
        &mut self,
        monitor: &MonitorInfo,
    ) -> Result<AppliedDisplay, PlatformError> {
        let bounds = monitor.bounds;
        self.restyle(
            WS_POPUP,
            (
                bounds.x,
                bounds.y,
                bounds.width as i32,
                bounds.height as i32,
            ),
        )?;
        self.state_mut().mode = WindowMode::Borderless;
        Ok(AppliedDisplay {
            mode: WindowMode::Borderless,
            width: bounds.width,
            height: bounds.height,
        })
    }

    /// Replaces the window's frame style and puts its outer rectangle at `rect`.
    fn restyle(
        &self,
        style: WINDOW_STYLE,
        rect: (i32, i32, i32, i32),
    ) -> Result<(), PlatformError> {
        // SAFETY: FFI read of a live window's own style; the visibility bit is carried over so
        // a hidden window stays hidden and a shown one stays shown.
        let current = unsafe { GetWindowLongPtrW(self.hwnd, GWL_STYLE) } as u32;
        let style = style.0 | (current & WS_VISIBLE.0);
        // SAFETY: FFI write of the window's own style word. SWP_FRAMECHANGED below makes
        // Windows recompute the frame for it.
        unsafe { SetWindowLongPtrW(self.hwnd, GWL_STYLE, style as isize) };
        // SAFETY: FFI call on a live window with no borrowed pointers.
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                SWP_FRAMECHANGED | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(|error| PlatformError::win32("SetWindowPos", error))
    }

    /// The window's outer rectangle on the virtual desktop: left, top, width, height.
    #[must_use]
    pub fn outer_rect(&self) -> (i32, i32, u32, u32) {
        let mut rect = RECT::default();
        // SAFETY: `hwnd` is live and `rect` is a valid out-parameter. A failure leaves it
        // zeroed, which reads as an empty rectangle.
        let _ = unsafe { GetWindowRect(self.hwnd, &mut rect) };
        (
            rect.left,
            rect.top,
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    }

    fn state_ref(&self) -> &WindowState {
        // SAFETY: `state` was created by Box::into_raw in `new`, is never null, and is
        // only reclaimed in `drop`, so it is valid for the lifetime of `self`. The window
        // procedure runs only inside `pump_events`, which requires `&mut self`, so no
        // aliasing mutation can be in flight here.
        unsafe { &*self.state }
    }

    fn state_mut(&mut self) -> &mut WindowState {
        // SAFETY: see `state_ref`. `&mut self` guarantees exclusive access.
        unsafe { &mut *self.state }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // The cursor's display counter is process-wide: a window that hid it and then went
        // away would leave the desktop without a pointer.
        self.set_cursor_visible(true);
        // And the monitor's mode: CDS_FULLSCREEN would be undone at exit anyway, but a window
        // dropped before the process ends — a test, a recreated window — must not leave it.
        if let Some(exclusive) = self.state_mut().exclusive.take() {
            display::restore_mode(&exclusive.device);
        }
        // SAFETY: `hwnd` is live and owned by this value. DestroyWindow synchronously
        // delivers WM_DESTROY/WM_NCDESTROY, so the procedure can still reach the state
        // during this call; the box is only reclaimed afterwards.
        if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
            log::warn!("DestroyWindow failed: {error}");
        }
        // SAFETY: the window no longer exists, so the procedure can never observe the
        // pointer again. This reclaims the box allocated in `new`.
        drop(unsafe { Box::from_raw(self.state) });
    }
}

/// Returns the module handle used as the window class instance.
fn module_handle() -> Result<HINSTANCE, PlatformError> {
    // SAFETY: passing `None` asks for the handle of the current executable, which is
    // always valid and needs no release.
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| PlatformError::win32("GetModuleHandleW", error))?;
    Ok(HINSTANCE(module.0))
}

/// Registers the window class the first time it is needed.
fn ensure_class_registered(instance: HINSTANCE) -> Result<(), PlatformError> {
    let result = CLASS_REGISTRATION.get_or_init(|| {
        // SAFETY: LoadCursorW with a system cursor identifier does not fail in a way we
        // must recover from; a null cursor simply means the class supplies none.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();

        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: cursor,
            // Black, not the null brush it was.
            //
            // `WM_ERASEBKGND` below tells GDI not to erase, because the renderer owns every
            // pixel — but that is only true once the renderer has presented a frame. Between
            // `CreateWindowExW` and the first `Present` the client area is whatever the
            // compositor last had there, which on Windows is white, and loading a scene takes
            // long enough for a player to see it. A black brush makes the window's own
            // non-client paint black instead; the application defers showing the window until
            // it has a frame, and the two together are why there is no white flash.
            hbrBackground: HBRUSH(black_brush()),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };

        // SAFETY: `class` is fully initialized and lives for the duration of the call;
        // `CLASS_NAME` is a static wide string that outlives the process.
        let atom = unsafe { RegisterClassExW(&class) };
        if atom == 0 {
            Err(windows::core::Error::from_thread())
        } else {
            Ok(())
        }
    });

    result
        .clone()
        .map_err(|error| PlatformError::win32("RegisterClassExW", error))
}

/// The stock black brush, as the raw handle `WNDCLASSEXW` wants.
///
/// A stock object: it is owned by the system, never has to be freed, and returning it from
/// several calls is the documented behaviour rather than a leak.
fn black_brush() -> *mut core::ffi::c_void {
    // SAFETY: FFI call with a valid stock-object index. The returned handle is owned by the
    // system and outlives the process.
    unsafe { GetStockObject(BLACK_BRUSH) }.0
}

/// Size of the primary monitor in pixels.
///
/// `GetSystemMetrics` is per-process DPI aware once `set_process_dpi_aware` has run, which
/// the application does before creating any window — otherwise this reports the scaled
/// size and a "fullscreen" window covers part of the screen.
fn primary_monitor_size() -> (u32, u32) {
    // SAFETY: FFI calls that read a global system value and cannot fail; an unknown
    // metric returns 0, which the fallback below covers.
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if width > 0 && height > 0 {
        (width as u32, height as u32)
    } else {
        log::warn!("GetSystemMetrics reported no primary monitor; falling back to 1920x1080");
        (1920, 1080)
    }
}

/// The mode among `modes` whose size is nearest `width` x `height`.
fn nearest_mode(modes: &[VideoMode], width: u32, height: u32) -> Option<VideoMode> {
    let wanted = (f64::from(width) * f64::from(height)).max(1.0);
    modes.iter().copied().min_by(|a, b| {
        let score = |mode: &VideoMode| {
            (f64::from(mode.width) * f64::from(mode.height) / wanted)
                .ln()
                .abs()
        };
        score(a)
            .partial_cmp(&score(b))
            .unwrap_or(core::cmp::Ordering::Equal)
    })
}

/// Converts a desired client size into the outer window size for a given style.
fn outer_size(
    width: u32,
    height: u32,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    ex_style: WINDOW_EX_STYLE,
) -> Result<(i32, i32), PlatformError> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    // SAFETY: `rect` is a valid, initialized rectangle owned by this function; the call
    // only writes back into it.
    unsafe { AdjustWindowRectEx(&mut rect, style, false, ex_style) }
        .map_err(|error| PlatformError::win32("AdjustWindowRectEx", error))?;
    Ok((rect.right - rect.left, rect.bottom - rect.top))
}

/// Window procedure. Translates Win32 messages into [`WindowEvent`]s.
///
/// # Safety
///
/// Called by Windows with a valid `hwnd`. Reads the [`WindowState`] pointer that
/// `WM_NCCREATE` stored in `GWLP_USERDATA`; that pointer stays valid until the owning
/// [`Window`] is dropped, which happens only after `DestroyWindow` returns.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE, Windows guarantees `lparam` points to a CREATESTRUCTW
        // that is valid for the duration of this message.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        // SAFETY: storing the state pointer passed to CreateWindowExW; subsequent
        // messages read it back through GWLP_USERDATA.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        // SAFETY: default processing must run for WM_NCCREATE or creation fails.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    // SAFETY: reads back the pointer stored above. It is null for the few messages that
    // precede WM_NCCREATE, which is why the null case falls through to default handling.
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if state.is_null() {
        // SAFETY: FFI call with the parameters Windows supplied.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    // SAFETY: the pointer is non-null and, per the contract above, points to a live
    // WindowState. Windows delivers messages on the thread that owns the window, and the
    // only other accessor requires `&mut Window`, so this reference is exclusive.
    let state = unsafe { &mut *state };

    match message {
        // How large the window is allowed to become.
        //
        // Windows defaults a resizable window's maximum *tracking* size to the monitor's work
        // area, and enforces it on `SetWindowPos` as well as on a drag. A benchmark asking a
        // window on a 1080p monitor for a 3840x2160 client area therefore got 1080p back,
        // silently, and waited forever for a size that could never arrive.
        //
        // Raising the limit lets the window be larger than the screen. The part that hangs
        // off the edge is not visible, and it does not need to be: the swap chain matches the
        // client area, so the GPU renders every one of those pixels and the measurement is
        // honest. A player never reaches this path — dragging is still bounded by the monitor
        // — and no other engine behaviour depends on a maximum size existing.
        WM_GETMINMAXINFO => {
            // SAFETY: for WM_GETMINMAXINFO, `lparam` is a pointer to a MINMAXINFO owned by
            // the caller and valid for the duration of the message.
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMaxTrackSize = POINT {
                x: MAX_WINDOW_EDGE,
                y: MAX_WINDOW_EDGE,
            };
            LRESULT(0)
        }
        WM_SIZE => {
            let (width, height) = size_from_lparam(lparam.0);
            match wparam.0 as u32 {
                SIZE_MINIMIZED => {
                    if !state.minimized {
                        state.minimized = true;
                        state.events.push(WindowEvent::Minimized);
                    }
                    state.client_width = 0;
                    state.client_height = 0;
                }
                SIZE_RESTORED | SIZE_MAXIMIZED => {
                    if state.minimized {
                        state.minimized = false;
                        state.events.push(WindowEvent::Restored);
                    }
                    if (width, height) != (state.client_width, state.client_height)
                        && width > 0
                        && height > 0
                    {
                        state.client_width = width;
                        state.client_height = height;
                        state.events.push(WindowEvent::Resized { width, height });
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            // Bit 30 of lparam is the previous key state: set means this is auto-repeat.
            // The event still reports the key as held, but marks the repeat so a toggle
            // fires once per physical press rather than sixty times a second.
            const PREVIOUS_STATE_DOWN: isize = 1 << 30;
            let repeat = lparam.0 & PREVIOUS_STATE_DOWN != 0;
            let key = key_from_virtual_code(wparam.0 as u16);
            state.events.push(WindowEvent::key_down(key, repeat));

            // System keys still need default processing, or Alt+F4 and the system menu
            // stop working.
            if message == WM_SYSKEYDOWN {
                // SAFETY: FFI call with the parameters Windows supplied.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let key = key_from_virtual_code(wparam.0 as u16);
            state.events.push(WindowEvent::key_up(key));
            if message == WM_SYSKEYUP {
                // SAFETY: FFI call with the parameters Windows supplied.
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = cursor_from_lparam(lparam.0);
            state.last_cursor = Some((x, y));
            state
                .events
                .push(WindowEvent::Input(InputEvent::MouseMoved { x, y }));
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
            let button = match message {
                WM_LBUTTONDOWN => MouseButton::Left,
                WM_RBUTTONDOWN => MouseButton::Right,
                WM_MBUTTONDOWN => MouseButton::Middle,
                _ => x_button_from_wparam(wparam.0),
            };
            // A button message carries where the pointer is. Reported first, so a click that
            // arrives with no move before it — the first click after the window takes focus,
            // a tablet tap — lands where it was made rather than where the pointer last was.
            let (x, y) = cursor_from_lparam(lparam.0);
            if state.last_cursor != Some((x, y)) {
                state.last_cursor = Some((x, y));
                state
                    .events
                    .push(WindowEvent::Input(InputEvent::MouseMoved { x, y }));
            }
            state
                .events
                .push(WindowEvent::Input(InputEvent::MouseButtonDown { button }));
            // The X-button messages are documented to return TRUE when handled.
            LRESULT(i32::from(message == WM_XBUTTONDOWN) as isize)
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
            let button = match message {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                WM_MBUTTONUP => MouseButton::Middle,
                _ => x_button_from_wparam(wparam.0),
            };
            let (x, y) = cursor_from_lparam(lparam.0);
            if state.last_cursor != Some((x, y)) {
                state.last_cursor = Some((x, y));
                state
                    .events
                    .push(WindowEvent::Input(InputEvent::MouseMoved { x, y }));
            }
            state
                .events
                .push(WindowEvent::Input(InputEvent::MouseButtonUp { button }));
            LRESULT(i32::from(message == WM_XBUTTONUP) as isize)
        }
        WM_MOUSEWHEEL => {
            let delta = wheel_from_wparam(wparam.0);
            state
                .events
                .push(WindowEvent::Input(InputEvent::MouseWheel { delta }));
            LRESULT(0)
        }
        WM_CLOSE => {
            // The application decides whether to honour the request, so the window is
            // not destroyed here.
            state.events.push(WindowEvent::CloseRequested);
            LRESULT(0)
        }
        WM_ACTIVATEAPP => {
            let focused = wparam.0 != 0;
            state
                .events
                .push(WindowEvent::Input(InputEvent::FocusChanged { focused }));
            // Exclusive fullscreen gives the monitor back while the game is not in front:
            // alt-tab lands on the desktop at its own resolution, and the game minimizes
            // rather than sitting in front of it at the wrong size. Coming back makes the
            // switch again.
            if let Some(exclusive) = state.exclusive.as_mut() {
                // SAFETY: FFI queries on the live window this message is for.
                let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
                if !focused && visible && !exclusive.suspended {
                    exclusive.suspended = true;
                    display::restore_mode(&exclusive.device);
                    // SAFETY: as above; the BOOL is the previous visibility.
                    let _ = unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
                } else if focused && exclusive.suspended {
                    exclusive.suspended = false;
                    if let Err(error) = display::change_mode(&exclusive.device, exclusive.mode) {
                        log::warn!("display: could not switch back after alt-tab: {error}");
                    }
                    // SAFETY: as above.
                    if unsafe { IsIconic(hwnd) }.as_bool() {
                        // SAFETY: as above.
                        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
                    }
                    if let Some(monitor) = display::monitor_of_window(hwnd) {
                        let b = monitor.bounds;
                        // SAFETY: FFI call on the live window; a failure leaves it where it
                        // was, which the next display change refits.
                        let _ = unsafe {
                            SetWindowPos(
                                hwnd,
                                Some(HWND_TOP),
                                b.x,
                                b.y,
                                b.width as i32,
                                b.height as i32,
                                SWP_NOOWNERZORDER,
                            )
                        };
                    }
                }
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            state.events.push(WindowEvent::DisplayChanged);
            // SAFETY: FFI call with the parameters Windows supplied.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        // Alt+Enter is the game's (it toggles fullscreen); without this Windows treats it as
        // an unhandled menu mnemonic and plays the error sound.
        WM_SYSCHAR if wparam.0 == 0x0D => LRESULT(0),
        WM_CHAR => {
            // UTF-16 code units: a character outside the basic plane arrives as a surrogate
            // pair in two messages, and is only a character once both halves are here.
            let unit = (wparam.0 & 0xFFFF) as u16;
            let character = if (0xD800..0xDC00).contains(&unit) {
                state.high_surrogate = Some(unit);
                None
            } else if (0xDC00..0xE000).contains(&unit) {
                state
                    .high_surrogate
                    .take()
                    .and_then(|high| char::decode_utf16([high, unit]).next())
                    .and_then(Result::ok)
            } else {
                state.high_surrogate = None;
                char::from_u32(u32::from(unit))
            };
            if let Some(character) = character {
                state
                    .events
                    .push(WindowEvent::Input(InputEvent::Text { character }));
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            // The renderer owns every pixel of the client area; letting GDI erase it
            // first only causes flicker.
            LRESULT(1)
        }
        WM_DESTROY => {
            // SAFETY: FFI call that only posts a message to this thread's queue.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: FFI call with the parameters Windows supplied.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Queries the live client size straight from Windows.
///
/// Used by tests and diagnostics to confirm the cached size matches reality.
///
/// # Errors
///
/// Returns [`PlatformError::Win32`] if `GetClientRect` fails.
pub fn query_client_size(window: &Window) -> Result<(u32, u32), PlatformError> {
    let mut rect = RECT::default();
    // SAFETY: `window` owns a live HWND and `rect` is a valid out-parameter.
    unsafe { GetClientRect(window.hwnd, &mut rect) }
        .map_err(|error| PlatformError::win32("GetClientRect", error))?;
    Ok((
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_size_is_at_least_the_client_size() {
        let Ok((width, height)) =
            outer_size(1280, 720, WS_OVERLAPPEDWINDOW, WINDOW_EX_STYLE::default())
        else {
            panic!("AdjustWindowRectEx must succeed for a standard style");
        };
        assert!(width >= 1280, "outer width {width}");
        assert!(
            height > 720,
            "outer height {height} must include the title bar"
        );
    }
}
