//! The one failure mode pure rendering data has.
//!
//! A separate type rather than reusing `engine_renderer::RendererError`: that enum carries
//! a `windows::core::Error` in its `Api` variant, and depending on it would drag `windows`
//! into this crate — which is exactly what this crate exists not to do.
//!
//! `engine-renderer` converts it into `RendererError::InvalidConfig`, so a caller inside the
//! renderer keeps writing `?` and sees no difference.

use std::fmt;

use engine_core::error::{EngineError, ErrorCategory};

/// A value that cannot describe real rendering data.
///
/// One variant, because there is one kind of mistake to make here: a number that produces a
/// degenerate matrix. Every one of these fails silently otherwise — a zero aspect ratio
/// gives infinities the GPU turns into an empty screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderDataError {
    /// A caller-supplied value is out of range or degenerate.
    InvalidConfig {
        /// Human-readable explanation, naming the value and its bounds.
        reason: String,
    },
}

impl RenderDataError {
    /// Builds an invalid-configuration error.
    #[must_use]
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidConfig {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for RenderDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { reason } => write!(f, "invalid render data: {reason}"),
        }
    }
}

impl std::error::Error for RenderDataError {}

impl From<RenderDataError> for EngineError {
    /// Lifts a render-data failure into the engine's error type.
    ///
    /// `Configuration`, not `Graphics`: nothing has touched a device yet, and the fix is a
    /// value the caller passed rather than a driver or an adapter.
    fn from(error: RenderDataError) -> Self {
        Self::new(ErrorCategory::Configuration, error.to_string()).with_source(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_what_was_wrong() {
        let error = RenderDataError::invalid("aspect ratio must be positive, got 0");
        assert!(error.to_string().contains("aspect ratio"), "{error}");
    }

    #[test]
    fn the_engine_conversion_calls_it_a_configuration_problem() {
        // Not Graphics: no device has been touched, and the fix is a value the caller
        // passed rather than a driver.
        let engine = EngineError::from(RenderDataError::invalid("near plane must be positive"));
        assert_eq!(engine.category(), ErrorCategory::Configuration);
        assert!(engine.chain().contains("near plane"), "{}", engine.chain());
    }
}
