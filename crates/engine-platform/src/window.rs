//! Win32 window creation and message pumping.
//!
//! All `unsafe` code in this crate lives here. Everything the window procedure needs
//! is stored in a [`WindowState`] that is owned by [`Window`] and reached from the
//! procedure through `GWLP_USERDATA`.

use std::sync::OnceLock;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CREATESTRUCTW, CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CW_USEDEFAULT,
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetClientRect,
    GetSystemMetrics, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, RegisterClassExW, SIZE_MAXIMIZED, SIZE_MINIMIZED, SIZE_RESTORED, SM_CXSCREEN,
    SM_CYSCREEN, SW_HIDE, SW_SHOW, SW_SHOWNORMAL, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WM_ACTIVATEAPP, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCCREATE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SIZE, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_OVERLAPPED, WS_OVERLAPPEDWINDOW,
    WS_POPUP, WS_SYSMENU,
};
use windows::core::{HSTRING, PCWSTR, w};

use engine_core::input::{InputEvent, MouseButton};

use crate::config::WindowConfig;
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
            client_width: client.0,
            client_height: client.1,
            minimized: false,
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
            quit_received: false,
        };

        let command = if config.visible {
            SW_SHOWNORMAL
        } else {
            SW_HIDE
        };
        // SAFETY: `hwnd` is a live window owned by `window`. ShowWindow's BOOL result
        // reports the previous visibility, not success, so it is intentionally ignored.
        let _ = unsafe { ShowWindow(hwnd, command) };

        log::info!(
            "window created: {}x{} client, resizable={}, visible={}, fullscreen={}",
            client.0,
            client.1,
            config.resizable && !config.fullscreen,
            config.visible,
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

    /// Shows or hides the window.
    pub fn set_visible(&self, visible: bool) {
        let command = if visible { SW_SHOW } else { SW_HIDE };
        // SAFETY: `hwnd` is live; the BOOL result is the previous visibility state.
        let _ = unsafe { ShowWindow(self.hwnd, command) };
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
            hbrBackground: HBRUSH::default(),
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
            state
                .events
                .push(WindowEvent::Input(InputEvent::FocusChanged {
                    focused: wparam.0 != 0,
                }));
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
