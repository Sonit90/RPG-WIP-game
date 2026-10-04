//! The dungeon-result outbox.
//!
//! ```text
//! instance::finish ─► pending_result rows ─► svc_take_results (bridge reads)
//!                                                   │ persisted with idempotency key
//!                                                   ▼
//!                                         svc_ack_results (rows removed)
//! ```
//!
//! The outbox lives in the same transaction as the run's end, so a result exists exactly when
//! the run ended. Delivery is at-least-once; the persistence API's `result_id` key makes it
//! exactly-once in effect. See `service.rs` for the two bridge entry points.
