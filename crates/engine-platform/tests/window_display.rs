//! Display-mode switching on a real `HWND`: windowed, borderless, and the way back.
//!
//! The windows are hidden, so nothing appears and no focus is taken; Windows still sizes,
//! restyles and reports them exactly as it would a visible one. The exclusive fullscreen
//! test changes the real monitor's mode and is therefore `#[ignore]`d — run it by hand with
//! `cargo test -p engine-platform --test window_display -- --ignored` on a desktop session.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use engine_platform::window::query_client_size;
use engine_platform::{Window, WindowConfig, WindowEvent, WindowMode};

fn hidden(width: u32, height: u32) -> Window {
    Window::new(&WindowConfig::new("display test", width, height).with_visible(false))
        .expect("window creation must succeed")
}

/// The last resize the window reported, after pumping its messages.
fn last_resize(window: &mut Window) -> Option<(u32, u32)> {
    window.pump_events();
    let mut last = None;
    while let Some(event) = window.poll_event() {
        if let WindowEvent::Resized { width, height } = event {
            last = Some((width, height));
        }
    }
    last
}

#[test]
fn the_monitor_the_window_is_on_is_described() {
    let window = hidden(640, 360);
    let monitor = window.monitor().expect("a desktop session has a monitor");
    assert!(monitor.bounds.width >= 640 && monitor.bounds.height >= 360);
    assert!(monitor.work_area.width <= monitor.bounds.width);
    assert!(monitor.desktop.width > 0 && monitor.desktop.height > 0);
}

#[test]
fn windowed_sets_the_client_size_and_reports_the_resize() {
    let mut window = hidden(640, 360);
    let applied = window
        .apply_display(WindowMode::Windowed, 800, 600)
        .expect("windowed always applies");
    assert_eq!(applied.mode, WindowMode::Windowed);
    assert_eq!((applied.width, applied.height), (800, 600));
    assert_eq!(query_client_size(&window).expect("client rect"), (800, 600));
    assert_eq!(last_resize(&mut window), Some((800, 600)));
    assert_eq!(window.window_mode(), WindowMode::Windowed);
    // Centred on the work area: the title bar is on the monitor.
    let monitor = window.monitor().expect("a monitor");
    let (x, y, _, _) = window.outer_rect();
    assert!(
        x >= monitor.work_area.x && y >= monitor.work_area.y,
        "{x},{y}"
    );
}

#[test]
fn borderless_covers_the_monitor_and_windowed_comes_back() {
    let mut window = hidden(640, 360);
    let monitor = window.monitor().expect("a monitor");
    let applied = window
        .apply_display(WindowMode::Borderless, 1280, 720)
        .expect("borderless always applies");
    assert_eq!(applied.mode, WindowMode::Borderless);
    // The desktop's size, not the one asked for.
    assert_eq!(
        (applied.width, applied.height),
        (monitor.bounds.width, monitor.bounds.height)
    );
    assert_eq!(
        query_client_size(&window).expect("client rect"),
        (monitor.bounds.width, monitor.bounds.height),
        "no border is left inside the monitor"
    );
    let (x, y, width, height) = window.outer_rect();
    assert_eq!((x, y), (monitor.bounds.x, monitor.bounds.y));
    assert_eq!(
        (width, height),
        (monitor.bounds.width, monitor.bounds.height)
    );
    assert_eq!(window.window_mode(), WindowMode::Borderless);

    let back = window
        .apply_display(WindowMode::Windowed, 1024, 576)
        .expect("windowed always applies");
    assert_eq!((back.width, back.height), (1024, 576));
    assert_eq!(
        query_client_size(&window).expect("client rect"),
        (1024, 576)
    );
    assert_eq!(last_resize(&mut window), Some((1024, 576)));
    assert!(
        window.outer_rect().2 > 1024,
        "the frame is back around the client area"
    );
}

#[test]
fn a_zero_size_is_refused_and_changes_nothing() {
    let mut window = hidden(640, 360);
    assert!(window.apply_display(WindowMode::Windowed, 0, 600).is_err());
    assert_eq!(query_client_size(&window).expect("client rect"), (640, 360));
}

#[test]
#[ignore = "switches the real monitor's display mode; run by hand"]
fn exclusive_fullscreen_switches_the_monitor_and_dropping_the_window_restores_it() {
    let mut window = hidden(640, 360);
    let monitor = window.monitor().expect("a monitor");
    let target = monitor
        .resolutions()
        .into_iter()
        .find(|size| *size != (monitor.desktop.width, monitor.desktop.height))
        .unwrap_or((monitor.desktop.width, monitor.desktop.height));
    let applied = window
        .apply_display(WindowMode::Fullscreen, target.0, target.1)
        .expect("fullscreen applies or falls back");
    if applied.mode == WindowMode::Fullscreen {
        assert_eq!((applied.width, applied.height), target);
        let switched = window.monitor().expect("a monitor");
        assert_eq!((switched.bounds.width, switched.bounds.height), target);
    }
    drop(window);
    let after = engine_platform::primary_monitor().expect("a monitor");
    let restored = (after.bounds.width, after.bounds.height);
    assert_eq!(restored, (monitor.bounds.width, monitor.bounds.height));
}
