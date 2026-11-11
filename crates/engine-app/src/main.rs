//! Game executable entry point.
//!
//! Wiring only: parse arguments, build the application, hand it to the engine.
//! The lifecycle and the frame loop belong to `engine-core`.

mod alloc_count;
mod app;
mod area_indicator;
mod args;
mod audio_device;
mod benchmark;
mod map_bake;
mod online;
mod online_audio;
mod online_community;
mod online_records;
mod run_profile;
#[cfg(feature = "standalone")]
mod standalone;
mod startup;

use std::process::ExitCode;

use engine_core::application::{Engine, EngineConfig};

use crate::app::App;

fn main() -> ExitCode {
    // First statement in the process: everything after it is measured against this instant.
    startup::begin();
    if let Err(error) = engine_core::logging::init() {
        eprintln!("fatal: failed to install logger: {error}");
        return ExitCode::FAILURE;
    }
    // The shipped talent numbers (assets/catalog/talents.json), online or not.
    if let Err(error) = engine_realm::talents::install_builtin() {
        eprintln!("fatal: {error}");
        return ExitCode::FAILURE;
    }

    #[cfg(feature = "standalone")]
    standalone::install();
    #[cfg(feature = "standalone")]
    let command_line = standalone::default_arguments(std::env::args().skip(1).collect());
    #[cfg(not(feature = "standalone"))]
    let command_line = std::env::args().skip(1);

    let mut args = match args::parse(command_line) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("error: {error}\n\n{}", args::USAGE);
            return ExitCode::FAILURE;
        }
    };

    if args.help {
        println!("{}", args::USAGE);
        return ExitCode::SUCCESS;
    }

    log::info!("{} {}", env!("CARGO_BIN_NAME"), env!("CARGO_PKG_VERSION"));
    // This text exists in the binary only when the developer tools are compiled in (the string
    // is reachable, so the linker keeps it). `tools/package.ps1` searches the shipping binary
    // for exactly `mygame-dev-tools-marker` and fails the package if it is found, so "the real
    // build has no dev tools" is checked rather than assumed.
    #[cfg(feature = "dev-tools")]
    log::info!("developer tools enabled (mygame-dev-tools-marker): not for release builds");

    // Online, the world is a realm dungeon: the level is the dungeon's, whatever scene the
    // command line's default would have chosen. Its monsters come from the server. The crypt
    // is only the level the client starts with: once an instance names its dungeon the client
    // loads that dungeon's scene from `dungeons.json` (see `App::install_dungeon_level`).
    if args.network_mode == args::NetworkMode::Online && args.scene_file.is_none() {
        args.scene = args::Scene::Graveyard;
    }

    // Configuration is validated before anything is created, so a bad value is a message
    // here rather than a failed Win32 call three layers down.
    let settings = args.to_settings();
    if let Err(error) = settings.validate() {
        eprintln!("error: {}\n\n{}", error.chain(), args::USAGE);
        return ExitCode::FAILURE;
    }

    startup::stage("process entry");

    let config = EngineConfig::default().with_frame_budget(settings.frame_budget);
    let result = App::new(&settings, &args).and_then(|app| Engine::new(app, config).run());

    // Records are written by a background thread, so every exit path drains before
    // returning: a process that exits with the queue full prints nothing about why.
    match result {
        Ok(()) => {
            log::info!("clean shutdown");
            engine_core::logging::drain();
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The chain, not just the outermost message: "graphics: initialize the
            // renderer" alone says nothing about which call returned which HRESULT.
            let chain = error.chain();
            log::error!("fatal: {chain}");
            engine_core::logging::drain();
            eprintln!("fatal: {chain}");
            ExitCode::FAILURE
        }
    }
}
