//! Core utilities shared by every engine crate.
//!
//! `engine-core` sits at the bottom of the dependency graph. It must not depend on
//! platform, renderer or gameplay code, and it must not contain gameplay concepts.
//!
//! Scope so far: the engine lifecycle, logging and frame timing.

pub mod application;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod input;
pub mod jobs;
pub mod logging;
pub mod memory;
pub mod metrics;
pub mod profiling;
pub mod threading;
pub mod time;
