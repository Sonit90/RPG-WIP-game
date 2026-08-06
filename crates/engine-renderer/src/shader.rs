//! Compiled shader bytecode.
//!
//! Every entry point is compiled to DXIL by `build.rs` and embedded in the binary. There is
//! no runtime compiler, no `d3dcompiler_47.dll`, no shader cache and no hot reload:
//!
//! * A shader error is a build error, with `dxc`'s file, line and message.
//! * Startup does no compilation work at all.
//! * The shipped binary needs no shader files beside it.
//!
//! The cost is that iterating on a shader means a rebuild. That is a few seconds on this
//! workspace, and it buys the guarantee that a shader which is in the binary is a shader
//! that compiled.

use windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE;

// The generated module: one `pub const NAME: &[u8]` per entry point in `build.rs`'s table.
include!(concat!(env!("OUT_DIR"), "/shaders.rs"));

/// Shader model every entry point targets.
///
/// 6.0 is the first DXIL model. It is supported by every D3D12 driver of interest,
/// including the RX 6700 XT this engine targets.
pub const SHADER_MODEL: &str = "6_0";

/// Describes embedded bytecode to D3D12.
///
/// The returned descriptor borrows `bytecode`. Every caller passes a `&'static [u8]` from
/// the generated module, so the buffer outlives any pipeline state built from it — which is
/// the whole reason for compiling offline rather than juggling `ID3DBlob` lifetimes.
#[must_use]
pub fn bytecode(bytecode: &'static [u8]) -> D3D12_SHADER_BYTECODE {
    D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytecode.as_ptr().cast(),
        BytecodeLength: bytecode.len(),
    }
}

/// Whether a blob starts with the DXIL container magic `DXBC`.
///
/// A signed DXIL container is a DXBC container whose first part is `DXIL`. An unsigned or
/// truncated blob is rejected by the driver at pipeline creation with an unhelpful
/// `E_INVALIDARG`, so it is worth being able to check it directly.
#[must_use]
pub fn is_dxil_container(blob: &[u8]) -> bool {
    blob.len() > 32 && blob.starts_with(b"DXBC")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry point the renderer uses, so the loop below covers all of them.
    const ALL: &[(&str, &[u8])] = &[
        ("TRIANGLE_VS", TRIANGLE_VS),
        ("TRIANGLE_PS", TRIANGLE_PS),
        ("INSTANCED_VS", INSTANCED_VS),
        ("INSTANCED_PS", INSTANCED_PS),
        ("MESH_VS", MESH_VS),
        ("MESH_PS", MESH_PS),
    ];

    #[test]
    fn every_shader_is_a_signed_dxil_container() {
        for (name, blob) in ALL {
            assert!(
                is_dxil_container(blob),
                "{name} is not a DXIL container: {} bytes starting {:?}",
                blob.len(),
                &blob[..blob.len().min(4)]
            );
        }
    }

    #[test]
    fn the_container_hash_is_not_all_zero() {
        // Bytes 4..20 of a DXBC container hold the digest dxc writes when it signs with
        // dxil.dll. An unsigned blob leaves them zero, and the driver refuses it — a
        // failure that otherwise appears only as a pipeline-creation error.
        for (name, blob) in ALL {
            assert!(
                blob[4..20].iter().any(|byte| *byte != 0),
                "{name} is unsigned: dxil.dll was missing beside dxc.exe at build time"
            );
        }
    }

    #[test]
    fn the_descriptor_points_at_the_blob() {
        let descriptor = bytecode(TRIANGLE_VS);
        assert_eq!(descriptor.BytecodeLength, TRIANGLE_VS.len());
        assert_eq!(
            descriptor.pShaderBytecode.cast::<u8>(),
            TRIANGLE_VS.as_ptr()
        );
    }

    #[test]
    fn a_truncated_blob_is_not_mistaken_for_a_container() {
        assert!(!is_dxil_container(b"DXBC"));
        assert!(!is_dxil_container(&[]));
    }
}
