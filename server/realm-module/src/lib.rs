//! The realm: the authoritative, realtime half of the online game.
//!
//! ```text
//!   game client ──WSS (OIDC token)──► reducers / views ──┐
//!                                                        ├──► this module (tables, rules)
//!   realm API (bridge) ──HTTP (service token)──► svc_* ──┘
//! ```
//!
//! What lives here: the queue, parties, ready checks, leaders, dungeon instances, every
//! runtime position, the barrier and the countdown, and the outbox of dungeon results. What
//! does not: accounts, characters, gear, settings — the persistence API owns those, and it is
//! the only caller allowed to put a character's snapshot into the queue (`svc_enqueue`) or to
//! collect results (`svc_take_results`). The module never makes an outbound request and holds
//! no secret: the bridge is recognised by its identity, which SpacetimeDB derives from a token
//! the bridge's own issuer signed.
//!
//! Every table is private. Clients read only through `my_*` views, each scoped to the caller
//! (`ViewContext::sender`), so a player in instance A receives nothing from instance B and
//! cannot subscribe their way into another party.
//!
//! The rules themselves — matchmaking, the party state machine, the ready check, the instance
//! lifecycle, the movement step — are `engine-realm` functions, tested natively. This crate is
//! the transactional shell around them.

mod admin;
mod combat;
mod connection;
mod instance;
mod level;
mod matchmaking;
mod movement;
mod party;
mod results;
mod schema;
mod service;
mod util;
mod views;

