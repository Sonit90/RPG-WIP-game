//! `asset_gen`: write the procedural starter library into an assets directory.
//!
//! ```text
//!  cargo run --bin asset_gen
//!  cargo run --bin asset_gen -- --list
//!  cargo run --bin asset_gen -- --root some/other/assets --force
//! ```
//!
//! Everything this file does is turn a command line into an [`engine_assetgen::Outcome`] and
//! that outcome into an exit code. The work is in the library beside it, so it can be tested
//! without a subprocess — see its documentation.

use std::process::ExitCode;

use engine_assetgen::{Outcome, USAGE, parse, run};

fn main() -> ExitCode {
    let _ = engine_core::logging::init();

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&arguments) {
        Ok(options) => options,
        Err(message) if message == "help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("asset_gen: {message}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let mut out = String::new();
    let outcome: Outcome = run(&options, &mut out);
    print!("{out}");

    if outcome.failing() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
