//! Persistent game data and the bridge to the realm.
//!
//! ```text
//!  routes (api crate) ──► store    ── Postgres `game` schema (characters, settings, results)
//!                     └─► bridge   ── HTTP ──► SpacetimeDB realm (svc_enqueue, results)
//! ```
//!
//! This crate owns everything long-lived about a player that is not their credentials:
//! characters (definition, level, experience, loadout, talents), dungeon completions, applied
//! results and account settings. The realm holds only snapshots of it for the length of a run,
//! and hands back results through an outbox this crate collects and applies exactly once.

pub mod bridge;
pub mod characters;
pub mod history;
pub mod store;

use std::sync::OnceLock;

use engine_realm::catalog::Catalog;

/// The catalog compiled into this service.
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| match Catalog::builtin() {
        Ok(catalog) => catalog,
        // The catalog is compiled in and tested; a broken one cannot ship.
        Err(e) => panic!("{e}"),
    })
}

/// The leaderboard rules compiled into this service.
pub fn leaderboards() -> &'static engine_realm::leaderboard::LeaderboardConfig {
    static CONFIG: OnceLock<engine_realm::leaderboard::LeaderboardConfig> = OnceLock::new();
    CONFIG.get_or_init(|| match engine_realm::leaderboard::LeaderboardConfig::builtin() {
        Ok(config) => config,
        // Compiled in and tested, like the catalog.
        Err(e) => panic!("{e}"),
    })
}
