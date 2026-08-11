//! Shader ABI: the CPU-side mirror of what `shaders/mesh.hlsl` declares.
//!
//! What the *game* produces — [`engine_render_data::RenderWorld`], the handle types,
//! [`engine_render_data::Lighting`] — moved to `engine-render-data` at the Milestone 15
//! checkpoint, so a gameplay crate can build a world without linking D3D12. What is left
//! here is the part that only makes sense next to a shader: two `repr(C)` structs whose
//! field offsets are the HLSL register layout, and the root-parameter indices that address
//! them.
//!
//! Those types stayed rather than moving with the world because they are not data the game
//! authors. Nothing outside the renderer should ever construct a [`FrameConstants`]; it
//! exists to be memcpy'd into a constant buffer whose layout a shader dictates.
//!
//! The D3D12 objects that consume this ABI live in [`crate::mesh_pipeline`].

use engine_core::math::{Mat4, Vec3};
use engine_render_data::Lighting;

/// Root parameter slots, matching `shaders/mesh.hlsl`.
///
/// **The slot index is not the shader register.** Root parameters are ordered by how often
/// they change — frame, then material, then object — because the most volatile argument
/// should be the cheapest thing to rewrite. The shader registers are `b0`, `b1`, `b2` in
/// declaration order, which puts the object buffer at `b1` and the material at `b2`. So the
/// root parameter array is built as `[b0, b2, b1]` and these three constants are the
/// mapping between the two orderings.
///
/// Getting this wrong binds the material constants where the shader expects the model
/// matrix. It produces no validation error and no crash — just geometry at the origin,
/// scaled by a colour. [`crate::mesh_pipeline`] carries the regression test that pins it.
///
/// Frame constants, `b0` in the shader.
pub const ROOT_FRAME_CBV: u32 = 0;
/// Material constants, `b2` in the shader.
pub const ROOT_MATERIAL_CBV: u32 = 1;
/// Per-object constants, `b1` in the shader.
pub const ROOT_OBJECT_CBV: u32 = 2;
/// The material's base colour map, a one-descriptor table at `t0`.
///
/// A table rather than a root SRV: a root descriptor cannot address a texture, only a buffer.
/// `material.rs` predicted this slot before there was anything to put in it — *"when a
/// material gains a texture, this becomes a table into the persistent descriptor region"* —
/// and this is that.
pub const ROOT_MATERIAL_TABLE: u32 = 3;
/// The bone palette, `b3` in `mesh_skinned.hlsl`.
///
/// Unused by the rigid pipelines, which share this root signature. An unused root parameter
/// costs two DWORDs of the 64 available and buys one signature, one material bind path and
/// one set of `ROOT_*` constants across all three pipelines.
pub const ROOT_SKIN_CBV: u32 = 4;

/// Shader register each root parameter binds, indexed by root parameter slot.
///
/// The single source of truth for the `[b0, b2, b1]` ordering: the root signature is built
/// from this array, and the regression test asserts the array against the registers the
/// HLSL declares. Changing the order here changes the signature and fails the test in the
/// same edit, which is the point.
pub const ROOT_PARAMETER_REGISTERS: [u32; 3] = [0, 2, 1];

/// Per-frame constants, matching `cbuffer Frame`.
///
/// `repr(C)`, and padded to whole 16-byte registers for the same reason
/// [`crate::material::MaterialConstants`] is.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameConstants {
    /// `projection * view`, column-major — the layout `Mat4` already has.
    pub view_projection: [f32; 16],
    /// The camera's world position.
    pub camera_position: [f32; 3],
    /// Padding to a whole register.
    pub padding: f32,
    /// The direction the light travels.
    pub light_direction: [f32; 3],
    /// Light strength. Shares a register with the direction.
    pub light_intensity: f32,
    /// Linear light colour.
    pub light_color: [f32; 3],
    /// Ambient strength. Shares a register with the light colour.
    pub ambient_intensity: f32,
    /// Linear ambient colour.
    pub ambient_color: [f32; 3],
    /// Padding to a whole register.
    pub padding_2: f32,
}

impl FrameConstants {
    /// Builds the constants for a camera and a light.
    #[must_use]
    pub fn new(view_projection: Mat4, camera_position: Vec3, lighting: Lighting) -> Self {
        let direction = lighting.direction.normalize();
        Self {
            view_projection: view_projection.to_cols_array(),
            camera_position: [camera_position.x, camera_position.y, camera_position.z],
            padding: 0.0,
            light_direction: [direction.x, direction.y, direction.z],
            light_intensity: lighting.intensity,
            light_color: [lighting.color.x, lighting.color.y, lighting.color.z],
            ambient_intensity: lighting.ambient_intensity,
            ambient_color: [
                lighting.ambient_color.x,
                lighting.ambient_color.y,
                lighting.ambient_color.z,
            ],
            padding_2: 0.0,
        }
    }
}

/// Per-object constants, matching `cbuffer Object`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ObjectConstants {
    /// The local-to-world matrix, column-major.
    pub model: [f32; 16],
    /// Multiplied with the material's base colour.
    ///
    /// The per-object mirror of the instanced path's `INSTANCE_TINT`. Both pipelines have
    /// to shade identically — `tests/instanced_equivalence.rs` asserts they produce the
    /// same image — so a field added to one is a field added to the other.
    pub tint: [f32; 4],
    /// Added to the material's emissive term.
    pub emissive: f32,
    /// Padding to the 16-byte constant-buffer row.
    pub padding: [f32; 3],
}

impl ObjectConstants {
    /// Builds the constants for a world transform and an unstyled surface.
    #[must_use]
    pub fn new(model: Mat4) -> Self {
        Self::styled(model, engine_render_data::InstanceStyle::NEUTRAL)
    }

    /// Builds the constants for a world transform and a visual state.
    #[must_use]
    pub fn styled(model: Mat4, style: engine_render_data::InstanceStyle) -> Self {
        Self {
            model: model.to_cols_array(),
            tint: style.tint,
            emissive: style.emissive,
            padding: [0.0; 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constant_layouts_match_the_shader_registers() {
        assert_eq!(size_of::<FrameConstants>(), 128);
        assert_eq!(std::mem::offset_of!(FrameConstants, camera_position), 64);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_direction), 80);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_color), 96);
        assert_eq!(std::mem::offset_of!(FrameConstants, ambient_color), 112);
        // 96, not 64: the matrix plus the per-object tint and emissive that mirror the
        // instanced pipeline's per-instance stream. The offsets are what the HLSL `Object`
        // constant buffer declares.
        assert_eq!(size_of::<ObjectConstants>(), 96);
        assert_eq!(std::mem::offset_of!(ObjectConstants, tint), 64);
        assert_eq!(std::mem::offset_of!(ObjectConstants, emissive), 80);
    }

    #[test]
    fn frame_constants_keep_the_matrix_column_major() {
        // The shader declares column_major, which is the layout `Mat4` already has, so no
        // transpose happens anywhere. A transpose added "for safety" would break this.
        let view_projection = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let constants = FrameConstants::new(view_projection, Vec3::ZERO, Lighting::default());
        assert_eq!(&constants.view_projection[12..15], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_light_direction_is_normalized_once_on_the_cpu() {
        // Normalizing per pixel would be a square root per fragment for a value that is the
        // same across the whole frame.
        let lighting = Lighting {
            direction: Vec3::new(0.0, -10.0, 0.0),
            ..Lighting::default()
        };
        let constants = FrameConstants::new(Mat4::IDENTITY, Vec3::ZERO, lighting);
        assert_eq!(constants.light_direction, [0.0, -1.0, 0.0]);
    }
}
