//! `asset_check`: validate assets against `docs/style/ASSET_LIMITATIONS.md`.
//!
//! ```text
//!  cargo run --bin asset_check -- assets/
//!  cargo run --bin asset_check -- --library --summary
//!  cargo run --bin asset_check -- --category rock assets/environment/rocks
//! ```
//!
//! Everything this file does is turn a command line into an [`engine_assetcheck::Outcome`]
//! and that outcome into an exit code. The work is in the library beside it, so it can be
//! tested without a subprocess — see its documentation.

use std::process::ExitCode;

use engine_assetcheck::{Outcome, USAGE, parse, run};

fn main() -> ExitCode {
    // The engine's own logger, so a run's lines look like the game's. Reports go to stdout;
    // the log carries whatever the importer has to say on the way there.
    let _ = engine_core::logging::init();

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("asset_check: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let mut out = String::new();
    let outcome: Outcome = run(&options, &mut out);
    print!("{out}");

    if outcome.failing(options.strict_warnings) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
