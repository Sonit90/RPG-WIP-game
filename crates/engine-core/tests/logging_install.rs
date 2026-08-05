//! Integration test: the global logger installs exactly once per process.
//!
//! This lives outside the crate on purpose (section 4: prefer external integration
//! tests), and it is a single test because `log::set_logger` is process-global state.

use log::LevelFilter;

#[test]
fn logger_installs_once_and_sets_max_level() {
    assert!(engine_core::logging::init_with(LevelFilter::Debug).is_ok());
    assert_eq!(log::max_level(), LevelFilter::Debug);

    // A second install must fail rather than silently replacing the logger.
    assert!(engine_core::logging::init_with(LevelFilter::Trace).is_err());

    log::info!("logger integration test emitted this line");
}
