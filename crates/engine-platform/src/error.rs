//! Platform layer error type, and its conversion into [`EngineError`].

use std::fmt;

use engine_core::error::{EngineError, ErrorCategory};

/// Failure modes of the platform layer.
#[derive(Debug)]
pub enum PlatformError {
    /// The [`crate::WindowConfig`] could not produce a window.
    InvalidConfig {
        /// Human-readable explanation of what is wrong.
        reason: String,
    },
    /// A Win32 call failed. `call` names the API so the message is actionable.
    #[cfg(windows)]
    Win32 {
        /// Name of the failing Win32 function, e.g. `CreateWindowExW`.
        call: &'static str,
        /// The underlying `HRESULT` / last-error value reported by Windows.
        source: windows::core::Error,
    },
    /// A native macOS call failed (window or event-loop creation, cursor grab). `call` names
    /// the operation so the message is actionable; winit reports no numeric code, only text.
    #[cfg(target_os = "macos")]
    Native {
        /// What was being attempted, e.g. `create window`.
        call: &'static str,
        /// The error text the underlying library gave.
        message: String,
    },
}

impl PlatformError {
    /// Wraps a `windows` crate error with the name of the call that produced it.
    #[cfg(windows)]
    pub(crate) fn win32(call: &'static str, source: windows::core::Error) -> Self {
        Self::Win32 { call, source }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { reason } => write!(f, "invalid window configuration: {reason}"),
            #[cfg(windows)]
            Self::Win32 { call, source } => {
                write!(
                    f,
                    "{call} failed: {} ({:#010x})",
                    source.message(),
                    source.code().0
                )
            }
            #[cfg(target_os = "macos")]
            Self::Native { call, message } => write!(f, "{call} failed: {message}"),
        }
    }
}

impl std::error::Error for PlatformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig { .. } => None,
            #[cfg(windows)]
            Self::Win32 { source, .. } => Some(source),
            #[cfg(target_os = "macos")]
            Self::Native { .. } => None,
        }
    }
}

impl From<PlatformError> for EngineError {
    /// Lifts a platform failure into the engine's error type.
    ///
    /// The conversion lives here, not in `engine-core`, because core must not know what a
    /// `PlatformError` is — and not in the application, where the orphan rule forbids it.
    ///
    /// An invalid `WindowConfig` becomes a `Configuration` error rather than a `Platform`
    /// one: nothing failed in the OS, the values were wrong before any call was made.
    fn from(error: PlatformError) -> Self {
        let category = match error {
            PlatformError::InvalidConfig { .. } => ErrorCategory::Configuration,
            #[cfg(windows)]
            PlatformError::Win32 { .. } => ErrorCategory::Platform,
            #[cfg(target_os = "macos")]
            PlatformError::Native { .. } => ErrorCategory::Platform,
        };
        Self::new(category, error.to_string()).with_source(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_config_message_contains_reason() {
        let error = PlatformError::InvalidConfig {
            reason: "width 0 is too small".to_owned(),
        };
        assert!(error.to_string().contains("width 0 is too small"));
    }

    #[cfg(windows)]
    #[test]
    fn win32_message_names_the_call_and_code() {
        let error = PlatformError::win32(
            "CreateWindowExW",
            windows::core::Error::from_hresult(windows::core::HRESULT(-2147024809)),
        );
        let text = error.to_string();
        assert!(text.contains("CreateWindowExW"), "{text}");
        assert!(text.contains("0x80070057"), "{text}");
    }
}
