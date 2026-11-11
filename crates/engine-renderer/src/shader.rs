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

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE;

// The generated module: one `pub const NAME: &[u8]` per entry point in `build.rs`'s table.
include!(concat!(env!("OUT_DIR"), "/shaders.rs"));

/// Shader model every entry point targets.
///
/// 6.0 is the first DXIL model. It is supported by every D3D12 driver of interest,
/// including the RX 6700 XT this engine targets.
pub const SHADER_MODEL: &str = "6_0";

#[cfg(windows)]
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

/// Which optional features a mesh pixel shader was compiled with.
///
/// The three axes are independent and each one is a `-D` at build time, not a branch at
/// runtime: a shader compiled without `normal_map` contains no normal fetch and no tangent
/// frame, and one compiled without `shadows` contains no cascade selection, no PCF and no
/// contact march. What that buys, measured at 1440p in
/// `docs/performance/shadows-off-permutation.md`: 0.150 ms for the BRDF, 0.071 ms for normal
/// mapping, 0.023 ms for shadow code that was already switched off at runtime.
///
/// The first two describe a *material* — what it declares and has maps for. The third
/// describes the *frame* — whether the shadow stage produced anything to sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ShaderFeatures {
    /// The metallic-roughness BRDF. Off: a single Lambert lobe.
    pub pbr: bool,
    /// The normal map and the tangent frame. Off: the interpolated geometric normal.
    pub normal_map: bool,
    /// Every shadow lookup. Off: an unshadowed surface.
    pub shadows: bool,
}

impl ShaderFeatures {
    /// The cheapest permutation: Lambert, no normal map, no shadows.
    pub const NONE: Self = Self {
        pbr: false,
        normal_map: false,
        shadows: false,
    };

    /// Everything on. What the offscreen render paths and the pixel tests use, so a test
    /// measures the full shader whatever the game materials declare.
    pub const ALL: Self = Self {
        pbr: true,
        normal_map: true,
        shadows: true,
    };

    /// The same features with shadows forced on or off.
    ///
    /// The material half of the set is per-material and the shadow half is per-frame, so
    /// they are chosen in different places and combined here.
    #[must_use]
    pub const fn with_shadows(self, shadows: bool) -> Self {
        Self { shadows, ..self }
    }
}

#[cfg(windows)]
/// The mesh pixel shader for `features`.
///
/// A `match` over all eight rather than a table indexed by bits: the constants are generated
/// names, and a missing permutation should be a compile error naming the constant, not an
/// index that silently reads the wrong shader.
#[must_use]
pub fn mesh_pixel_shader(features: ShaderFeatures) -> &'static [u8] {
    match (features.pbr, features.normal_map, features.shadows) {
        (false, false, false) => MESH_PS,
        (false, false, true) => MESH_PS_SHADOWS,
        (false, true, false) => MESH_PS_NORMAL_MAP,
        (false, true, true) => MESH_PS_NORMAL_MAP_SHADOWS,
        (true, false, false) => MESH_PS_PBR,
        (true, false, true) => MESH_PS_PBR_SHADOWS,
        (true, true, false) => MESH_PS_PBR_NORMAL_MAP,
        (true, true, true) => MESH_PS_PBR_NORMAL_MAP_SHADOWS,
    }
}

#[cfg(windows)]
/// The instanced mesh pixel shader for `features`.
#[must_use]
pub fn mesh_instanced_pixel_shader(features: ShaderFeatures) -> &'static [u8] {
    match (features.pbr, features.normal_map, features.shadows) {
        (false, false, false) => MESH_INSTANCED_PS,
        (false, false, true) => MESH_INSTANCED_PS_SHADOWS,
        (false, true, false) => MESH_INSTANCED_PS_NORMAL_MAP,
        (false, true, true) => MESH_INSTANCED_PS_NORMAL_MAP_SHADOWS,
        (true, false, false) => MESH_INSTANCED_PS_PBR,
        (true, false, true) => MESH_INSTANCED_PS_PBR_SHADOWS,
        (true, true, false) => MESH_INSTANCED_PS_PBR_NORMAL_MAP,
        (true, true, true) => MESH_INSTANCED_PS_PBR_NORMAL_MAP_SHADOWS,
    }
}

#[cfg(windows)]
/// The skinned mesh pixel shader for `features`.
#[must_use]
pub fn mesh_skinned_pixel_shader(features: ShaderFeatures) -> &'static [u8] {
    match (features.pbr, features.normal_map, features.shadows) {
        (false, false, false) => MESH_SKINNED_PS,
        (false, false, true) => MESH_SKINNED_PS_SHADOWS,
        (false, true, false) => MESH_SKINNED_PS_NORMAL_MAP,
        (false, true, true) => MESH_SKINNED_PS_NORMAL_MAP_SHADOWS,
        (true, false, false) => MESH_SKINNED_PS_PBR,
        (true, false, true) => MESH_SKINNED_PS_PBR_SHADOWS,
        (true, true, false) => MESH_SKINNED_PS_PBR_NORMAL_MAP,
        (true, true, true) => MESH_SKINNED_PS_PBR_NORMAL_MAP_SHADOWS,
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

#[cfg(windows)]
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
        ("MESH_PS_SHADOWS", MESH_PS_SHADOWS),
        ("MESH_PS_NORMAL_MAP", MESH_PS_NORMAL_MAP),
        ("MESH_PS_NORMAL_MAP_SHADOWS", MESH_PS_NORMAL_MAP_SHADOWS),
        ("MESH_PS_PBR", MESH_PS_PBR),
        ("MESH_PS_PBR_SHADOWS", MESH_PS_PBR_SHADOWS),
        ("MESH_PS_PBR_NORMAL_MAP", MESH_PS_PBR_NORMAL_MAP),
        (
            "MESH_PS_PBR_NORMAL_MAP_SHADOWS",
            MESH_PS_PBR_NORMAL_MAP_SHADOWS,
        ),
        ("MESH_INSTANCED_PS", MESH_INSTANCED_PS),
        ("MESH_SKINNED_PS", MESH_SKINNED_PS),
    ];

    /// The eight permutations, in the order the feature bits count up.
    const PERMUTATIONS: [ShaderFeatures; 8] = [
        ShaderFeatures::NONE,
        ShaderFeatures {
            shadows: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures {
            normal_map: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures {
            normal_map: true,
            shadows: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures {
            pbr: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures {
            pbr: true,
            shadows: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures {
            pbr: true,
            normal_map: true,
            ..ShaderFeatures::NONE
        },
        ShaderFeatures::ALL,
    ];

    #[test]
    fn every_permutation_is_a_distinct_signed_container() {
        for features in PERMUTATIONS {
            for blob in [
                mesh_pixel_shader(features),
                mesh_instanced_pixel_shader(features),
                mesh_skinned_pixel_shader(features),
            ] {
                assert!(
                    is_dxil_container(blob),
                    "{features:?} is not a DXIL container"
                );
                assert!(
                    blob[4..20].iter().any(|byte| *byte != 0),
                    "{features:?} is unsigned"
                );
            }
        }

        // No two permutations may be the same bytes: identical blobs would mean a define
        // never reached dxc, or that two rows collided on one output file - both of which
        // leave the renderer binding a shader containing work it asked to have removed.
        for (index, features) in PERMUTATIONS.iter().enumerate() {
            for other in &PERMUTATIONS[index + 1..] {
                assert_ne!(
                    mesh_pixel_shader(*features),
                    mesh_pixel_shader(*other),
                    "{features:?} and {other:?} compiled to the same bytes"
                );
            }
        }
    }

    #[test]
    fn switching_a_feature_off_makes_the_shader_smaller() {
        // Not a performance assertion - it is the cheap structural check that the defines
        // are doing something: each feature adds code, so the all-off permutation is the
        // smallest blob and the all-on permutation the largest.
        let none = mesh_pixel_shader(ShaderFeatures::NONE).len();
        let all = mesh_pixel_shader(ShaderFeatures::ALL).len();
        assert!(none < all, "all-off is {none} bytes, all-on is {all}");
        for features in PERMUTATIONS {
            let size = mesh_pixel_shader(features).len();
            assert!(
                (none..=all).contains(&size),
                "{features:?} is {size} bytes, outside [{none}, {all}]"
            );
        }
    }

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
