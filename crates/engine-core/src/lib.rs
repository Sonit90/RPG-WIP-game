//! Core utilities shared by every engine crate.
//!
//! `engine-core` sits at the bottom of the dependency graph. It must not depend on
//! platform, renderer or gameplay code, and it must not contain gameplay concepts.
//!
//! Phase 0 scope: logging and frame timing.

pub mod logging;
pub mod time;
