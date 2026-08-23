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
use engine_render_data::{Camera, Lighting, LocalLight, LocalLightKind};

use crate::light_cluster::LightClusters;
use crate::shadow::atlas::AtlasRect;
use crate::shadow::cascade::CascadeView;
use crate::shadow::local::{LocalShadowView, SelectedLight};
use crate::shadow::settings::{
    MAX_CASCADES, MAX_LOCAL_LIGHTS, MAX_LOCAL_SHADOW_VIEWS, ShadowSettings,
};

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
/// The shadow atlas, a one-descriptor table at `t1`.
///
/// Its own table rather than another entry in the material's: the material's descriptor
/// changes per batch and this one changes once a frame, and a descriptor table has to be
/// contiguous — putting them together would mean re-allocating the pair beside every
/// material's map.
pub const ROOT_SHADOW_TABLE: u32 = 5;
/// The scene depth buffer the contact-shadow march reads, a one-descriptor table at `t2`.
pub const ROOT_DEPTH_TABLE: u32 = 6;

/// The clustered light grid, at `t3`.
///
/// A **root** SRV: a GPU address written straight into the root arguments, like the constant
/// buffers and unlike the two textures above. A raw buffer can be one where a texture cannot,
/// so the grid costs no descriptor, no heap slot and no range allocator.
pub const ROOT_CLUSTER_SRV: u32 = 7;

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

/// Per-frame constants, matching `cbuffer Frame` in `shaders/scene_frame.hlsli`.
///
/// `repr(C)`, and padded to whole 16-byte registers for the same reason
/// [`crate::material::MaterialConstants`] is.
///
/// # Why one buffer and not several
///
/// The shadow half of this — four cascade matrices, sixteen local-light view matrices, the
/// atlas rectangles and the biases — is about 2.3 KB, written once per frame into the upload
/// ring and bound once as a root CBV. Splitting it into a second buffer would buy a smaller
/// memcpy and cost a second root parameter, a second allocation and a second bind, in a
/// renderer where the whole frame's constants are already a rounding error beside the
/// per-object data. It is one buffer because it changes at one rate: once per frame.
///
/// The layout is asserted field-by-field in this module's tests. Every offset below has a
/// counterpart in the HLSL, and a struct that agrees with the shader today and not tomorrow
/// is the kind of bug that renders as geometry in the wrong place with no error anywhere.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
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

    /// One light view-projection per cascade.
    pub cascade_view_projection: [[f32; 16]; MAX_CASCADES],
    /// Where each cascade lives in the atlas: `[scale_x, scale_y, bias_x, bias_y]`.
    pub cascade_uv_scale_bias: [[f32; 4]; MAX_CASCADES],
    /// The far distance of each cascade, in metres along the view direction.
    pub cascade_far: [f32; 4],
    /// How much world one texel of each cascade covers, in metres.
    pub cascade_texel_world: [f32; 4],
    /// `[cascade count, blend fraction, PCF kernel code, debug mode]`.
    pub shadow_params: [f32; 4],
    /// `[normal offset in texels, its ceiling in metres, shadow strength, unused]`.
    pub shadow_params_2: [f32; 4],
    /// `[atlas width, atlas height, 1 / width, 1 / height]`.
    pub shadow_atlas_size: [f32; 4],
    /// `[camera near, camera far, unused, unused]`.
    pub camera_params: [f32; 4],
    /// `[contact ray length, steps, thickness, max distance]`. A zero max distance is off.
    pub contact_params: [f32; 4],
    /// The inverse of `view_projection`, for reconstructing a world position from depth.
    pub inverse_view_projection: [f32; 16],

    /// `[x, y, z, range]` per local light.
    pub light_position_range: [[f32; 4]; MAX_LOCAL_LIGHTS],
    /// `[r, g, b, intensity]` per local light.
    pub light_color_intensity: [[f32; 4]; MAX_LOCAL_LIGHTS],
    /// `[x, y, z, inner cosine]` per local light.
    pub light_direction_inner: [[f32; 4]; MAX_LOCAL_LIGHTS],
    /// `[outer cosine, first shadow view or -1, view count, 1 for a spot light]`.
    pub light_shadow: [[f32; 4]; MAX_LOCAL_LIGHTS],
    /// `[count, 0, 0, 0]`: how many entries of the four arrays above are live.
    pub local_light_count: [f32; 4],

    /// One view-projection per local shadow view.
    pub local_view_projection: [[f32; 16]; MAX_LOCAL_SHADOW_VIEWS],
    /// Where each local shadow view lives in the atlas.
    pub local_uv_scale_bias: [[f32; 4]; MAX_LOCAL_SHADOW_VIEWS],

    /// `[cluster grid x, y, z, 1 when the grid is live]`.
    pub cluster_params: [f32; 4],
    /// `[depth-slice scale, depth-slice bias, camera near, camera far]`.
    pub cluster_depth: [f32; 4],
}


/// Written out rather than derived: `#[derive(Default)]` does not reach an array longer than
/// thirty-two, and the cluster mask array is eight hundred and sixty-four `uint4`s. Every
/// field is zero, which is what the shader reads as "no shadows, no lights, no grid".
impl Default for FrameConstants {
    fn default() -> Self {
        Self {
            view_projection: [0.0; 16],
            camera_position: [0.0; 3],
            padding: 0.0,
            light_direction: [0.0; 3],
            light_intensity: 0.0,
            light_color: [0.0; 3],
            ambient_intensity: 0.0,
            ambient_color: [0.0; 3],
            padding_2: 0.0,
            cascade_view_projection: [[0.0; 16]; MAX_CASCADES],
            cascade_uv_scale_bias: [[0.0; 4]; MAX_CASCADES],
            cascade_far: [0.0; 4],
            cascade_texel_world: [0.0; 4],
            shadow_params: [0.0; 4],
            shadow_params_2: [0.0; 4],
            shadow_atlas_size: [0.0; 4],
            camera_params: [0.0; 4],
            contact_params: [0.0; 4],
            inverse_view_projection: [0.0; 16],
            light_position_range: [[0.0; 4]; MAX_LOCAL_LIGHTS],
            light_color_intensity: [[0.0; 4]; MAX_LOCAL_LIGHTS],
            light_direction_inner: [[0.0; 4]; MAX_LOCAL_LIGHTS],
            light_shadow: [[0.0; 4]; MAX_LOCAL_LIGHTS],
            local_light_count: [0.0; 4],
            local_view_projection: [[0.0; 16]; MAX_LOCAL_SHADOW_VIEWS],
            local_uv_scale_bias: [[0.0; 4]; MAX_LOCAL_SHADOW_VIEWS],
            cluster_params: [0.0; 4],
            cluster_depth: [0.0; 4],
        }
    }
}

impl FrameConstants {
    /// Builds the constants for a camera and a light, with no shadows.
    ///
    /// Shadow fields stay zero, and a zero `shadow_params.x` — no cascades — is what the
    /// shader reads as "this frame has no shadow maps". That is the state a caller that
    /// knows nothing about shadows produces, which is what keeps every existing test and the
    /// offscreen paths working unchanged.
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
            ..Self::default()
        }
    }

    /// Records what the camera can see, for the effects that need to undo the projection.
    ///
    /// The contact-shadow march reconstructs a world distance from the depth buffer, which
    /// needs the near and far planes; nothing else reads these.
    pub fn set_camera(&mut self, camera: &Camera) {
        let (near, far) = camera.projection().depth_range();
        // z and w keep whatever `set_viewport` put there: the two are written from different
        // places — a camera is gameplay's, a viewport is the swap chain's — and a camera
        // change must not wipe the size out from under the debug view that reads it.
        self.camera_params = [near, far, self.camera_params[2], self.camera_params[3]];
        self.inverse_view_projection = camera
            .view_projection()
            .inverse()
            .unwrap_or(Mat4::IDENTITY)
            .to_cols_array();
    }

    /// Records how large the frame being drawn is, in pixels.
    ///
    /// Only the shadow-atlas debug view reads it, and it reads it to decide where on the
    /// screen the preview goes. It lives in the frame constants rather than in a debug-only
    /// buffer because the alternative — a second constant buffer bound only when a debug
    /// mode is on — is a second root argument and a second upload on every frame, paid for
    /// by every player, so that a developer can press F7.
    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.camera_params[2] = width as f32;
        self.camera_params[3] = height as f32;
    }

    /// Records the cascades and the settings the lighting pass samples them with.
    ///
    /// `rects` are the atlas rectangles the cascades were rendered into, in the same order.
    /// A cascade with no rectangle is not written, so it is never sampled.
    pub fn set_directional_shadows(
        &mut self,
        cascades: &[CascadeView],
        rects: &[AtlasRect],
        atlas_size: (u32, u32),
        settings: &ShadowSettings,
    ) {
        let count = cascades.len().min(rects.len()).min(MAX_CASCADES);
        for index in 0..count {
            self.cascade_view_projection[index] = cascades[index].view_projection.to_cols_array();
            self.cascade_uv_scale_bias[index] =
                rects[index].uv_scale_bias(atlas_size.0, atlas_size.1);
            self.cascade_far[index] = cascades[index].split.far;
            self.cascade_texel_world[index] = cascades[index].texel_world_size;
        }
        self.shadow_params = [
            count as f32,
            settings.blend_fraction,
            settings.pcf.shader_code() as f32,
            self.shadow_params[3],
        ];
        self.shadow_params_2 = [
            settings.bias.normal_texels,
            settings.bias.normal_max_meters,
            // Strength is the one switch the shader reads to skip every shadow lookup, so it
            // is zero whenever there is nothing to sample rather than only when the feature
            // is off.
            if count == 0 { 0.0 } else { 1.0 },
            settings.filter_radius_texels,
        ];
        self.set_atlas_size(atlas_size);
    }

    /// Records the atlas dimensions the UV rectangles were computed against.
    pub fn set_atlas_size(&mut self, atlas_size: (u32, u32)) {
        let (width, height) = (atlas_size.0.max(1) as f32, atlas_size.1.max(1) as f32);
        self.shadow_atlas_size = [width, height, 1.0 / width, 1.0 / height];
    }

    /// Records the contact-shadow march's parameters, or switches it off.
    pub fn set_contact_shadows(&mut self, settings: &crate::shadow::ContactShadowSettings) {
        self.contact_params = if settings.enabled {
            [
                settings.length,
                settings.steps as f32,
                settings.thickness,
                settings.max_distance,
            ]
        } else {
            // A zero max distance is the shader's "skip the march" case; the other three are
            // left as they are so a frame that toggles the effect does not also change its
            // tuning.
            [
                settings.length,
                settings.steps as f32,
                settings.thickness,
                0.0,
            ]
        };
    }

    /// Records this frame's local lights and which of them have shadow maps.
    ///
    /// Lights past [`MAX_LOCAL_LIGHTS`] are dropped: the shader loops over a fixed array and
    /// there is no clustering, so the count is a per-pixel cost rather than a scene budget.
    /// They are dropped from the *end* of the list the caller supplied, so a caller that
    /// cares about which ones survive sorts them first.
    pub fn set_local_lights(
        &mut self,
        lights: &[LocalLight],
        selected: &[SelectedLight],
        views: &[LocalShadowView],
        rects: &[AtlasRect],
        atlas_size: (u32, u32),
    ) {
        let count = lights.len().min(MAX_LOCAL_LIGHTS);
        self.local_light_count = [count as f32, 0.0, 0.0, 0.0];
        for (index, light) in lights.iter().take(count).enumerate() {
            self.light_position_range[index] = [
                light.position.x,
                light.position.y,
                light.position.z,
                light.range,
            ];
            self.light_color_intensity[index] =
                [light.color.x, light.color.y, light.color.z, light.intensity];
            let direction = if light.direction.length_squared() > 1e-6 {
                light.direction.normalize()
            } else {
                Vec3::new(0.0, -1.0, 0.0)
            };
            self.light_direction_inner[index] =
                [direction.x, direction.y, direction.z, light.inner_cos];
            // -1 until a shadow map is assigned below: a light with no map is the common
            // case, and the shader tests this one value rather than a parallel flag.
            self.light_shadow[index] = [
                light.outer_cos,
                -1.0,
                0.0,
                f32::from(u8::from(light.kind == LocalLightKind::Spot)),
            ];
        }

        for entry in selected {
            if entry.light >= count {
                continue;
            }
            self.light_shadow[entry.light][1] = entry.first_view as f32;
            self.light_shadow[entry.light][2] = entry.view_count as f32;
        }

        let view_count = views.len().min(rects.len()).min(MAX_LOCAL_SHADOW_VIEWS);
        for index in 0..view_count {
            self.local_view_projection[index] = views[index].view_projection.to_cols_array();
            self.local_uv_scale_bias[index] =
                rects[index].uv_scale_bias(atlas_size.0, atlas_size.1);
        }
    }

    /// Records the clustered light grid this frame was built with.
    ///
    /// A frame that never calls this leaves `cluster_params.w` at zero, and the shader then
    /// walks the whole light list instead — which is the right answer for the offscreen
    /// helpers, whose scenes have at most a handful of lights and no camera to cluster
    /// against.
    pub fn set_light_clusters(&mut self, clusters: &LightClusters) {
        self.cluster_params = clusters.params();
        self.cluster_depth = clusters.depth_params();
    }

    /// Sets the shader's debug mode: 0 none, 1 cascade tint, 2 shadow factor, 3 the atlas.
    pub fn set_debug_mode(&mut self, mode: u32) {
        self.shadow_params[3] = mode as f32;
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
    /// 1 when the lighting pass may darken this object where a shadow falls on it.
    ///
    /// A float in what used to be padding, rather than a new register: the constant buffer
    /// is padded to sixteen bytes either way, and a `bool` in a constant buffer is four
    /// bytes in HLSL regardless.
    pub receives_shadow: f32,
    /// Padding to the 16-byte constant-buffer row.
    pub padding: [f32; 2],
}

impl ObjectConstants {
    /// Builds the constants for a world transform and an unstyled surface.
    #[must_use]
    pub fn new(model: Mat4) -> Self {
        Self::styled(model, engine_render_data::InstanceStyle::NEUTRAL)
    }

    /// Builds the constants for a world transform and a visual state.
    ///
    /// Receives shadows: that is what ordinary geometry does, and a caller that has an
    /// opinion uses [`ObjectConstants::shadowed`].
    #[must_use]
    pub fn styled(model: Mat4, style: engine_render_data::InstanceStyle) -> Self {
        Self::shadowed(model, style, true)
    }

    /// Builds the constants for a world transform, a visual state and a shadow flag.
    #[must_use]
    pub fn shadowed(
        model: Mat4,
        style: engine_render_data::InstanceStyle,
        receives_shadow: bool,
    ) -> Self {
        Self {
            model: model.to_cols_array(),
            tint: style.tint,
            emissive: style.emissive,
            receives_shadow: f32::from(u8::from(receives_shadow)),
            padding: [0.0; 2],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constant_layouts_match_the_shader_registers() {
        // Every offset here has a counterpart in `shaders/scene_frame.hlsli`, in the order
        // the fields are declared there. HLSL packs a constant buffer into 16-byte registers
        // and will not straddle one, so these numbers are the register layout — not a
        // description of it, the thing itself.
        assert_eq!(std::mem::offset_of!(FrameConstants, camera_position), 64);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_direction), 80);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_color), 96);
        assert_eq!(std::mem::offset_of!(FrameConstants, ambient_color), 112);

        // The shadow half begins where the pre-shadow buffer used to end, which is what
        // keeps every offset above unchanged.
        assert_eq!(
            std::mem::offset_of!(FrameConstants, cascade_view_projection),
            128
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, cascade_uv_scale_bias),
            384
        );
        assert_eq!(std::mem::offset_of!(FrameConstants, cascade_far), 448);
        assert_eq!(
            std::mem::offset_of!(FrameConstants, cascade_texel_world),
            464
        );
        assert_eq!(std::mem::offset_of!(FrameConstants, shadow_params), 480);
        assert_eq!(std::mem::offset_of!(FrameConstants, shadow_params_2), 496);
        assert_eq!(std::mem::offset_of!(FrameConstants, shadow_atlas_size), 512);
        assert_eq!(std::mem::offset_of!(FrameConstants, camera_params), 528);
        assert_eq!(std::mem::offset_of!(FrameConstants, contact_params), 544);
        assert_eq!(
            std::mem::offset_of!(FrameConstants, inverse_view_projection),
            560
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, light_position_range),
            624
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, light_color_intensity),
            1136
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, light_direction_inner),
            1648
        );
        assert_eq!(std::mem::offset_of!(FrameConstants, light_shadow), 2160);
        assert_eq!(
            std::mem::offset_of!(FrameConstants, local_light_count),
            2672
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, local_view_projection),
            2688
        );
        assert_eq!(
            std::mem::offset_of!(FrameConstants, local_uv_scale_bias),
            3712
        );
        assert_eq!(std::mem::offset_of!(FrameConstants, cluster_params), 3968);
        assert_eq!(std::mem::offset_of!(FrameConstants, cluster_depth), 3984);
        assert_eq!(size_of::<FrameConstants>(), 4000);
        // Kept small on purpose. The cluster masks live in their own buffer rather than
        // here: putting 13.5 KB of them in this one cost 0.34 ms a frame at 4K, because a
        // constant buffer that no longer fits the hardware's constant cache is slower to
        // read *everywhere*, not only in the part that grew.
        assert!(size_of::<FrameConstants>() <= 8 * 1024);
        // Whole registers, or the upload's padding to 256 bytes would leave the shader
        // reading a partial one.
        assert_eq!(size_of::<FrameConstants>() % 16, 0);

        // 96, not 64: the matrix plus the per-object tint, emissive and shadow flag that
        // mirror the instanced pipeline's per-instance stream. The offsets are what the HLSL
        // `Object` constant buffer declares.
        assert_eq!(size_of::<ObjectConstants>(), 96);
        assert_eq!(std::mem::offset_of!(ObjectConstants, tint), 64);
        assert_eq!(std::mem::offset_of!(ObjectConstants, emissive), 80);
        assert_eq!(std::mem::offset_of!(ObjectConstants, receives_shadow), 84);
    }

    #[test]
    fn a_frame_with_no_shadows_tells_the_shader_so() {
        // The unshadowed path is what every caller that knows nothing about shadows
        // produces — the offscreen helpers, the benchmarks, the older tests — and it has to
        // mean "no shadow lookups" rather than "sample cascade zero, which is a zero matrix".
        let constants = FrameConstants::new(Mat4::IDENTITY, Vec3::ZERO, Lighting::default());
        assert_eq!(constants.shadow_params[0], 0.0, "no cascades");
        assert_eq!(constants.shadow_params_2[2], 0.0, "no shadow strength");
        assert_eq!(constants.contact_params[3], 0.0, "no contact shadows");
        assert_eq!(constants.local_light_count[0], 0.0, "no local lights");
    }

    #[test]
    fn an_object_receives_shadows_unless_it_says_otherwise() {
        let ordinary = ObjectConstants::new(Mat4::IDENTITY);
        assert_eq!(ordinary.receives_shadow, 1.0);
        let decal = ObjectConstants::shadowed(
            Mat4::IDENTITY,
            engine_render_data::InstanceStyle::NEUTRAL,
            false,
        );
        assert_eq!(decal.receives_shadow, 0.0);
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
