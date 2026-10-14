//! The realm's rules, shared by every program that has to agree on them.
//!
//! ```text
//!                      engine-realm (this crate: rules, no I/O)
//!                     /             |                \
//!   realm-module (wasm32)     game client          persistence API
//!   authoritative state       prediction, UI       gear score, loadouts
//! ```
//!
//! The SpacetimeDB module is the authority: it owns the queue, the parties, the instances and
//! every runtime position. But *what* it decides — whether three tickets make a party, whether
//! a member may kick another, what one movement input does to a body — is written here, as
//! plain functions over plain data, so that:
//!
//! * the rules are unit-tested natively with `cargo test -p engine-realm`, without a server;
//! * the client predicts its own movement with the very function the server applies;
//! * the persistence API computes the gear score the matchmaker reads with the same code.
//!
//! Nothing here reads a clock or a random source. Time arrives as microseconds since the Unix
//! epoch (the server's `ctx.timestamp`) and randomness as a number the caller rolled, which is
//! what keeps every rule deterministic and testable.
//!
//! | Module | What it decides |
//! | --- | --- |
//! | [`api`] | the realm API's request and response bodies |
//! | [`role`] | Tank, Healer, DPS |
//! | [`catalog`] | which characters and dungeons exist |
//! | [`loadout`] | what a character wears, and its gear score |
//! | [`economy`] | resources, salvage and exchanges |
//! | [`key`] | dungeon keys: validity, progression, scaling |
//! | [`rewards`] | what a finished run pays |
//! | [`matchmaking`] | which queued tickets become a party |
//! | [`party`] | the party state machine, the ready check, who may do what |
//! | [`instance`] | the dungeon instance lifecycle, countdown and barrier |
//! | [`progression`] | levels and experience |
//! | [`movement`] | one input frame applied to one body, and the input budget |
//! | [`talents`] | stable talent keys, validated server-side |
//! | [`rate_limit`] | token buckets for every client-callable operation |
//! | [`error`] | the error categories a player can be shown |

pub mod api;
pub mod catalog;
pub mod combat;
pub mod economy;
pub mod error;
pub mod instance;
pub mod key;
pub mod loadout;
pub mod matchmaking;
pub mod movement;
pub mod ops;
pub mod party;
pub mod progression;
pub mod rate_limit;
pub mod rewards;
pub mod role;
pub mod talents;

pub use error::{ErrorCategory, RealmError};
pub use role::Role;

/// Microseconds since the Unix epoch: how the realm writes every instant.
///
/// SpacetimeDB's `Timestamp` is this number; keeping it a plain integer here means the rules
/// never depend on the server's types and a test can say "thirty seconds later" with `+`.
pub type Micros = i64;

/// One second in [`Micros`].
pub const SECOND: Micros = 1_000_000;
