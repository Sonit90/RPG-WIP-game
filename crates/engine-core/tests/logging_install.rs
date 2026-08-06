//! Integration test: the global logger installs exactly once per process.
//!
//! This lives outside the crate on purpose (section 4: prefer external integration
//! tests), and it is a single test because `log::set_logger` is process-global state.

use engine_core::logging;
use log::{Level, LevelFilter};

#[test]
fn logger_installs_once_and_filters_by_level() {
    assert!(logging::init_with(LevelFilter::Debug).is_ok());
    assert_eq!(log::max_level(), LevelFilter::Debug);

    // A second install must fail rather than silently replacing the logger.
    assert!(logging::init_with(LevelFilter::Trace).is_err());

    // At Debug, everything down to Debug passes and Trace does not.
    assert!(logging::enabled(Level::Error, "Test"));
    assert!(logging::enabled(Level::Warn, "Test"));
    assert!(logging::enabled(Level::Info, "Test"));
    assert!(logging::enabled(Level::Debug, "Test"));
    assert!(!logging::enabled(Level::Trace, "Test"));

    // Raising the filter silences everything below it. This is the check that matters for
    // the hot path: a disabled level must be answerable without formatting a message.
    log::set_max_level(LevelFilter::Warn);
    assert!(logging::enabled(Level::Error, "Test"));
    assert!(logging::enabled(Level::Warn, "Test"));
    assert!(!logging::enabled(Level::Info, "Test"));
    assert!(!logging::enabled(Level::Debug, "Test"));

    log::set_max_level(LevelFilter::Off);
    for level in [
        Level::Error,
        Level::Warn,
        Level::Info,
        Level::Debug,
        Level::Trace,
    ] {
        assert!(!logging::enabled(level, "Test"), "{level} survived Off");
    }

    log::set_max_level(LevelFilter::Info);
    log::info!(target: "Test", "logger integration test emitted this line");
    log::debug!(target: "Test", "this line must not appear: the filter is Info");
}
