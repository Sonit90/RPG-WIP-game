//! `realm-bot`: headless realm clients.
//!
//! ```text
//! realm-bot scenario <slice|isolation|decline|reconnect|reconnect_party|security|results|combat|loop|chat|loot|modifiers|history|community|all>
//! realm-bot bot --email E --password P --character warrior --role tank [--dungeon D] [--walk] [--lead]
//! ```
//!
//! Endpoints come from the environment (`SPACETIMEDB_URL`, `SPACETIMEDB_DATABASE`,
//! `AUTH_API_URL`, `PERSISTENCE_API_URL`) or `--realm-config <file>`, defaulting to this
//! machine. `bot` runs one scripted player until Ctrl+C — three of them, or two and the game,
//! make a party for local multiplayer testing.

mod bot;
mod scenarios;

use std::process::ExitCode;
use std::time::Duration;

use engine_net::RealmEndpoints;
use engine_realm::Role;

use crate::bot::{Bot, BotConfig};

/// A scenario's entry point.
type Scenario = fn(&RealmEndpoints) -> scenarios::Outcome;

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> ExitCode {
    let _ = engine_core::logging::init_with(log::LevelFilter::Info);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = value(&args, "--realm-config").map(std::path::PathBuf::from);
    let endpoints = match RealmEndpoints::resolve(config.as_deref()) {
        Ok(endpoints) => endpoints,
        Err(error) => {
            eprintln!("realm-bot: {error}");
            return ExitCode::FAILURE;
        }
    };
    match args.first().map(String::as_str) {
        Some("scenario") => {
            let which = args.get(1).map_or("all", String::as_str);
            let list: Vec<(&str, Scenario)> = vec![
                ("slice", scenarios::slice),
                ("isolation", scenarios::isolation),
                ("decline", scenarios::decline),
                ("reconnect", scenarios::reconnect),
                ("reconnect_party", scenarios::reconnect_party),
                ("security", scenarios::security),
                ("results", scenarios::results),
                ("combat", scenarios::combat),
                ("loop", scenarios::game_loop),
                ("chat", scenarios::chat),
                ("loot", scenarios::loot),
                ("modifiers", scenarios::modifiers),
                ("history", scenarios::history),
                ("community", scenarios::community),
            ];
            let mut failed = false;
            for (name, run) in list {
                if which != "all" && which != name {
                    continue;
                }
                match run(&endpoints) {
                    Ok(()) => println!("PASS {name}"),
                    Err(error) => {
                        println!("FAIL {name}: {error}");
                        failed = true;
                    }
                }
            }
            // Let the logger thread flush.
            std::thread::sleep(Duration::from_millis(200));
            if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Some("bot") => {
            let role = value(&args, "--role")
                .and_then(|r| Role::from_key(&r))
                .unwrap_or(Role::Dps);
            let config = BotConfig {
                name: value(&args, "--name").unwrap_or_else(|| role.key().into()),
                email: value(&args, "--email")
                    .unwrap_or_else(|| format!("{}@bots.mygame.test", role.key())),
                password: value(&args, "--password").unwrap_or_else(|| "bot-password-1".into()),
                definition: value(&args, "--character").unwrap_or_else(|| "firemage".into()),
                role,
                dungeon: value(&args, "--dungeon").unwrap_or_else(|| "graveyard_crypt".into()),
                accept: !args.iter().any(|a| a == "--decline"),
                insert_key: args.iter().any(|a| a == "--lead"),
                start_if_leader: args.iter().any(|a| a == "--lead"),
                walk: args.iter().any(|a| a == "--walk"),
                hunt: args.iter().any(|a| a == "--hunt"),
                queue: true,
            };
            let mut bot = match Bot::start(config, &endpoints) {
                Ok(bot) => bot,
                Err(error) => {
                    eprintln!("realm-bot: {}", error.message);
                    return ExitCode::FAILURE;
                }
            };
            loop {
                bot.tick();
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        _ => {
            eprintln!(
                "usage: realm-bot scenario <slice|isolation|decline|reconnect|reconnect_party|security|results|combat|loop|chat|loot|modifiers|history|community|all>"
            );
            eprintln!(
                "       realm-bot bot --email E --password P --character warrior --role tank [--walk] [--lead]"
            );
            ExitCode::FAILURE
        }
    }
}
