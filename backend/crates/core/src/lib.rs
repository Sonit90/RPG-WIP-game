//! Pure domain types for generic auth system.
//!
//! This crate is intentionally free of HTTP, DB, and IO concerns so it can be
//! reused from other crates without pulling in a large dependency graph.

pub mod error;
pub mod models;

pub use error::{AppError, AppResult};
