//! HLSL compilation.
//!
//! Shaders are compiled at startup with `D3DCompile` from `d3dcompiler_47.dll`, which
//! ships with Windows. This is not a shader system: there is no cache, no reflection, no
//! hot reload and no offline pipeline. Phase 0 needs exactly two entry points, and a
//! compile error must produce the compiler's own message rather than a bare `HRESULT`.

use windows::Win32::Graphics::Direct3D::Fxc::{
    D3DCOMPILE_DEBUG, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCOMPILE_SKIP_OPTIMIZATION,
    D3DCOMPILE_WARNINGS_ARE_ERRORS, D3DCompile,
};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::core::PCSTR;

use crate::error::RendererError;

/// Compiles one HLSL entry point.
///
/// `entry_point` and `target` must be null-terminated, e.g. `s!("VSMain")` and
/// `s!("vs_5_1")`. `name` appears in compiler diagnostics.
///
/// # Errors
///
/// Returns [`RendererError::ShaderCompilation`] carrying the compiler's message, which is
/// the only thing that makes an HLSL error fixable.
pub fn compile_hlsl(
    source: &str,
    name: &str,
    entry_point: PCSTR,
    target: PCSTR,
) -> Result<ID3DBlob, RendererError> {
    // Debug builds keep the shader debuggable and treat warnings as errors; release
    // builds optimize. The cost is startup time only — there are two shaders.
    let flags = if cfg!(debug_assertions) {
        D3DCOMPILE_DEBUG | D3DCOMPILE_SKIP_OPTIMIZATION | D3DCOMPILE_WARNINGS_ARE_ERRORS
    } else {
        D3DCOMPILE_OPTIMIZATION_LEVEL3
    };

    let source_name = std::ffi::CString::new(name).map_err(|_| RendererError::InvalidConfig {
        reason: format!("shader name {name:?} contains an interior nul byte"),
    })?;

    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;

    // SAFETY: FFI call. `source` and `source_name` outlive the call, the size matches the
    // slice, and both out-parameters are valid. D3DCompile fills `errors` on failure and
    // may also fill it with warnings on success.
    let result = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR(source_name.as_ptr().cast()),
            None,
            None,
            entry_point,
            target,
            flags,
            0,
            &mut code,
            Some(&mut errors),
        )
    };

    let messages = errors.as_ref().map(blob_to_string).unwrap_or_default();

    match result {
        Ok(()) => {
            if !messages.trim().is_empty() {
                log::warn!("shader {name} compiled with messages: {}", messages.trim());
            }
            code.ok_or_else(|| RendererError::ShaderCompilation {
                name: name.to_owned(),
                messages: "compiler reported success but produced no bytecode".to_owned(),
            })
        }
        Err(error) => Err(RendererError::ShaderCompilation {
            name: name.to_owned(),
            messages: if messages.trim().is_empty() {
                error.message()
            } else {
                messages.trim().to_owned()
            },
        }),
    }
}

/// Reads a blob's bytes as a lossy UTF-8 string. Compiler messages are ASCII.
fn blob_to_string(blob: &ID3DBlob) -> String {
    // SAFETY: FFI calls returning the blob's buffer and its length. The slice is only
    // read while `blob` is alive, and the pointer is non-null for a live blob.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_owned()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use windows::core::s;

    const TRIVIAL: &str = "float4 VSMain() : SV_POSITION { return float4(0, 0, 0, 1); }";

    #[test]
    fn compiles_a_trivial_vertex_shader() {
        let blob = compile_hlsl(TRIVIAL, "trivial.hlsl", s!("VSMain"), s!("vs_5_1"))
            .expect("a trivial shader must compile");
        // SAFETY: FFI call on a live blob returned by a successful compile.
        let size = unsafe { blob.GetBufferSize() };
        assert!(size > 0, "bytecode must not be empty");
    }

    #[test]
    fn reports_the_compiler_message_for_broken_source() {
        let error = compile_hlsl(
            "this is not hlsl",
            "broken.hlsl",
            s!("VSMain"),
            s!("vs_5_1"),
        )
        .expect_err("broken source must not compile");
        let text = error.to_string();
        assert!(text.contains("broken.hlsl"), "{text}");
        assert!(
            text.to_lowercase().contains("error"),
            "the compiler message must be included: {text}"
        );
    }

    #[test]
    fn reports_a_missing_entry_point() {
        let error = compile_hlsl(TRIVIAL, "trivial.hlsl", s!("NoSuchEntry"), s!("vs_5_1"))
            .expect_err("a missing entry point must fail");
        assert!(error.to_string().contains("trivial.hlsl"));
    }
}
