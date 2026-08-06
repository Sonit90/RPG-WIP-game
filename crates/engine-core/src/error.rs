//! The engine's error type.
//!
//! ## Shape
//!
//! One struct, one category enum, an optional source. Not a deep hierarchy: every layer
//! that added its own wrapper enum would force every caller to write another `match` arm,
//! and the information a caller actually acts on is "which subsystem failed, and what
//! was it doing".
//!
//! ```text
//! EngineError { category, context, source }
//!               ^         ^        ^ the original error, preserved for the chain
//!               ^         ^ what the engine was doing when it failed
//!               ^ which subsystem
//! ```
//!
//! ## Errors vs. assertions
//!
//! `EngineError` is for failures a correct program can hit: a device removed by a driver
//! reset, a file that is not there, a configuration the user typed wrong. A broken engine
//! invariant is not one of those — see [`crate::diagnostics`].
//!
//! ## Allocation
//!
//! Constructing an error allocates (the context string, the boxed source). Errors are not
//! a hot path; a frame that fails has already lost.

use std::error::Error;
use std::fmt;

/// Which subsystem failed, and broadly how.
///
/// The categories are the ones a caller can plausibly branch on. Anything finer belongs
/// in the context string, which is what a human reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// The OS, the window system, or an input device.
    Platform,
    /// The graphics API: device, swap chain, pipeline, shader compilation.
    Graphics,
    /// Reading or writing a file.
    File,
    /// A configuration value the engine cannot honour.
    Configuration,
    /// A subsystem failed to start.
    Initialization,
    /// A resource could not be created, found or loaded.
    Resource,
    /// An operation was attempted in a state that does not allow it.
    ///
    /// The boundary against an assertion: use this when *another* component drove the
    /// engine into a bad order of calls, and an assertion when this component's own
    /// invariant is broken.
    InvalidState,
    /// The hardware or driver lacks something the engine requires.
    UnsupportedFeature,
}

impl ErrorCategory {
    /// Short lowercase name, used in messages and log lines.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Graphics => "graphics",
            Self::File => "file",
            Self::Configuration => "configuration",
            Self::Initialization => "initialization",
            Self::Resource => "resource",
            Self::InvalidState => "invalid state",
            Self::UnsupportedFeature => "unsupported feature",
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A recoverable engine failure, with enough context to diagnose it.
#[derive(Debug)]
pub struct EngineError {
    category: ErrorCategory,
    context: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl EngineError {
    /// Creates an error in `category` describing what failed.
    ///
    /// The context should name the operation, not restate the category: "create the swap
    /// chain", not "graphics error".
    #[must_use]
    pub fn new(category: ErrorCategory, context: impl Into<String>) -> Self {
        Self {
            category,
            context: context.into(),
            source: None,
        }
    }

    /// Attaches the underlying error, preserving the chain.
    #[must_use]
    pub fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Adds an outer layer of context, keeping this error as the source.
    ///
    /// Turns "CreateWindowExW failed" into "start the platform layer: CreateWindowExW
    /// failed", which is what makes a failure legible three layers up.
    #[must_use]
    pub fn context(self, context: impl Into<String>) -> Self {
        let category = self.category;
        Self {
            category,
            context: context.into(),
            source: Some(Box::new(self)),
        }
    }

    /// Which subsystem failed.
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        self.category
    }

    /// What the engine was doing when it failed.
    #[must_use]
    pub fn context_message(&self) -> &str {
        &self.context
    }

    /// The whole chain rendered as one line, innermost cause last.
    ///
    /// `std`'s `Display` shows only the outermost message, which on its own is rarely
    /// enough to act on.
    #[must_use]
    pub fn chain(&self) -> String {
        let mut text = self.to_string();
        let mut source = self.source();
        while let Some(error) = source {
            text.push_str(": ");
            text.push_str(&error.to_string());
            source = error.source();
        }
        text
    }

    /// An OS, window-system or input-device failure.
    #[must_use]
    pub fn platform(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Platform, context)
    }

    /// A graphics-API failure.
    #[must_use]
    pub fn graphics(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Graphics, context)
    }

    /// A file-system failure.
    #[must_use]
    pub fn file(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::File, context)
    }

    /// A configuration value the engine cannot honour.
    #[must_use]
    pub fn configuration(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Configuration, context)
    }

    /// A subsystem that failed to start.
    #[must_use]
    pub fn initialization(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Initialization, context)
    }

    /// A resource that could not be created, found or loaded.
    #[must_use]
    pub fn resource(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Resource, context)
    }

    /// An operation attempted in a state that does not allow it.
    #[must_use]
    pub fn invalid_state(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::InvalidState, context)
    }

    /// A capability the hardware or driver does not provide.
    #[must_use]
    pub fn unsupported(context: impl Into<String>) -> Self {
        Self::new(ErrorCategory::UnsupportedFeature, context)
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category, self.context)
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // The double deref goes Box<dyn Error + Send + Sync> -> dyn Error + Send + Sync,
        // which then coerces to the plain trait object the API asks for.
        self.source
            .as_ref()
            .map(|source| &**source as &(dyn Error + 'static))
    }
}

impl From<std::io::Error> for EngineError {
    fn from(error: std::io::Error) -> Self {
        Self::file(error.to_string()).with_source(error)
    }
}

/// Result type used throughout the engine.
pub type EngineResult<T> = Result<T, EngineError>;

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Underlying(&'static str);

    impl fmt::Display for Underlying {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl Error for Underlying {}

    #[test]
    fn an_error_names_its_category_and_context() {
        let error = EngineError::graphics("create the swap chain");
        assert_eq!(error.category(), ErrorCategory::Graphics);
        assert_eq!(error.context_message(), "create the swap chain");
        assert_eq!(error.to_string(), "graphics: create the swap chain");
        assert!(error.source().is_none());
    }

    #[test]
    fn every_category_has_a_distinct_name() {
        let categories = [
            ErrorCategory::Platform,
            ErrorCategory::Graphics,
            ErrorCategory::File,
            ErrorCategory::Configuration,
            ErrorCategory::Initialization,
            ErrorCategory::Resource,
            ErrorCategory::InvalidState,
            ErrorCategory::UnsupportedFeature,
        ];
        let mut names: Vec<&str> = categories.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two categories share a name");
    }

    #[test]
    fn a_source_is_preserved_and_reachable() {
        let error =
            EngineError::platform("create the window").with_source(Underlying("E_INVALIDARG"));
        let source = error.source().expect("a source was attached");
        assert_eq!(source.to_string(), "E_INVALIDARG");
    }

    #[test]
    fn context_wraps_without_losing_the_cause() {
        let error = EngineError::graphics("D3D12CreateDevice failed")
            .with_source(Underlying("E_NOINTERFACE"))
            .context("initialize the renderer");

        assert_eq!(error.to_string(), "graphics: initialize the renderer");
        let chain = error.chain();
        assert!(chain.contains("initialize the renderer"), "{chain}");
        assert!(chain.contains("D3D12CreateDevice failed"), "{chain}");
        assert!(chain.contains("E_NOINTERFACE"), "{chain}");
    }

    #[test]
    fn context_keeps_the_original_category() {
        let error = EngineError::unsupported("shader model 6").context("load the pipeline");
        assert_eq!(error.category(), ErrorCategory::UnsupportedFeature);
    }

    #[test]
    fn the_chain_of_a_bare_error_is_just_its_message() {
        assert_eq!(
            EngineError::file("open config.toml").chain(),
            "file: open config.toml"
        );
    }

    #[test]
    fn an_io_error_becomes_a_file_error_with_its_cause() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "settings.toml not found");
        let error = EngineError::from(io);
        assert_eq!(error.category(), ErrorCategory::File);
        assert!(error.chain().contains("settings.toml not found"));
    }

    #[test]
    fn errors_are_usable_across_threads() {
        // Send + Sync matters: a job that fails on a worker has to return its error.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EngineError>();
    }
}
