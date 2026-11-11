//! `map_check`: validate community map files offline.
//!
//! ```text
//!  cargo run -p engine-realm --bin map_check -- my_map.json [more.json ...]
//!  cargo run -p engine-realm --bin map_check -- --json my_map.json
//! ```
//!
//! Prints each file's report and exits non-zero if any file has errors. This is the same
//! `engine_realm::mapcheck` pipeline the server runs on an upload; the file reading is the
//! only thing this binary adds.

use std::process::ExitCode;

use engine_realm::mapcheck::{Rules, check_str};

fn main() -> ExitCode {
    let mut json = false;
    let mut files = Vec::new();
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("usage: map_check [--json] <map.json>...");
                return ExitCode::SUCCESS;
            }
            _ => files.push(argument),
        }
    }
    if files.is_empty() {
        eprintln!("usage: map_check [--json] <map.json>...");
        return ExitCode::FAILURE;
    }

    let rules = Rules::default_community();
    let mut failed = false;
    for file in &files {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("{file}: cannot read: {error}");
                failed = true;
                continue;
            }
        };
        let report = check_str(&text, &rules);
        failed |= !report.is_valid();
        if json {
            match serde_json::to_string_pretty(&report) {
                Ok(out) => println!("{out}"),
                Err(error) => eprintln!("{file}: cannot serialise report: {error}"),
            }
        } else {
            println!(
                "{file}: {}\n{report}",
                if report.is_valid() { "OK" } else { "INVALID" }
            );
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
