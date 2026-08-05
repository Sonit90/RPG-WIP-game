//! Renderer error type.
//!
//! Every DXGI/D3D12 call that can fail is wrapped with the name of the call, so an
//! initialization failure says which API rejected us and with which `HRESULT`.

use std::fmt;

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
}
