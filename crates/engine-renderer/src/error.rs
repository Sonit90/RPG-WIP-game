//! Renderer error type.
//!
//! Every DXGI/D3D12 call that can fail is wrapped with the name of the call, so an
//! initialization failure says which API rejected us and with which `HRESULT`.

use std::fmt;

use engine_core::error::{EngineError, ErrorCategory};

/// Failure modes of the D3D12 renderer.
#[derive(Debug)]
pub enum RendererError {
    /// A DXGI or D3D12 call returned a failing `HRESULT`.
    Api {
        /// Name of the failing function, e.g. `D3D12CreateDevice`.
        call: &'static str,
        /// The `HRESULT` reported by Windows.
        source: windows::core::Error,
    },
    /// No adapter on this machine could create a D3D12 device at the required level.
    NoSuitableAdapter {
        /// How many adapters were enumerated before giving up.
        examined: usize,
    },
    /// A caller-supplied value cannot describe a real swap chain.
    InvalidConfig {
        /// Human-readable explanation.
        reason: String,
    },
    /// An HLSL shader failed to compile.
    ShaderCompilation {
        /// Name the shader was compiled under, as it appears in the messages.
        name: String,
        /// The compiler's own diagnostics.
        messages: String,
    },
}

impl RendererError {
    /// Wraps a `windows` error with the name of the call that produced it.
    pub(crate) fn api(call: &'static str, source: windows::core::Error) -> Self {
        Self::Api { call, source }
    }
}

impl fmt::Display for RendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api { call, source } => write!(
                f,
                "{call} failed: {} ({:#010x})",
                source.message(),
                source.code().0
            ),
            Self::NoSuitableAdapter { examined } => write!(
                f,
                "no Direct3D 12 capable adapter found ({examined} examined); \
                 update the GPU driver or check that the GPU supports feature level 11_0"
            ),
            Self::InvalidConfig { reason } => write!(f, "invalid renderer configuration: {reason}"),
            Self::ShaderCompilation { name, messages } => {
                write!(f, "failed to compile shader {name}:\n{messages}")
            }
        }
    }
}

impl std::error::Error for RendererError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Api { source, .. } => Some(source),
            Self::NoSuitableAdapter { .. }
            | Self::InvalidConfig { .. }
            | Self::ShaderCompilation { .. } => None,
        }
    }
}

impl From<RendererError> for EngineError {
    /// Lifts a renderer failure into the engine's error type.
    ///
    /// The categories are not all `Graphics`: a machine with no D3D12-capable adapter is
    /// an `UnsupportedFeature` — no retry or different call will help — while a bad
    /// swap-chain size is `Configuration`. That distinction is what lets a caller decide
    /// between "tell the user to update the driver" and "fix the value".
    fn from(error: RendererError) -> Self {
        let category = match error {
            RendererError::Api { .. } | RendererError::ShaderCompilation { .. } => {
                ErrorCategory::Graphics
            }
            RendererError::NoSuitableAdapter { .. } => ErrorCategory::UnsupportedFeature,
            RendererError::InvalidConfig { .. } => ErrorCategory::Configuration,
        };
        Self::new(category, error.to_string()).with_source(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_message_names_call_and_code() {
        let error = RendererError::api(
            "D3D12CreateDevice",
            windows::core::Error::from_hresult(windows::core::HRESULT(-2005270523)),
        );
        let text = error.to_string();
        assert!(text.contains("D3D12CreateDevice"), "{text}");
        assert!(text.contains("0x887a0005"), "{text}");
    }

    #[test]
    fn no_adapter_message_reports_how_many_were_examined() {
        let text = RendererError::NoSuitableAdapter { examined: 3 }.to_string();
        assert!(text.contains('3'), "{text}");
    }

    #[test]
    fn conversion_picks_the_category_a_caller_can_act_on() {
        use engine_core::error::ErrorCategory;

        let missing_adapter = EngineError::from(RendererError::NoSuitableAdapter { examined: 2 });
        assert_eq!(
            missing_adapter.category(),
            ErrorCategory::UnsupportedFeature,
            "no retry helps a machine without a D3D12 adapter"
        );

        let bad_config = EngineError::from(RendererError::InvalidConfig {
            reason: "width 0".to_owned(),
        });
        assert_eq!(bad_config.category(), ErrorCategory::Configuration);

        let api = EngineError::from(RendererError::api(
            "D3D12CreateDevice",
            windows::core::Error::from_hresult(windows::core::HRESULT(-2005270523)),
        ));
        assert_eq!(api.category(), ErrorCategory::Graphics);
        assert!(
            api.chain().contains("D3D12CreateDevice"),
            "the original error must survive the conversion: {}",
            api.chain()
        );
    }
}
