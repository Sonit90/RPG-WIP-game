//! Cascade mathematics: splitting the view, bounding each slice, and building a stable
//! light projection for it.
//!
//! Pure CPU, no device, no `windows` type. That is deliberate — this is the part of the
//! shadow system most likely to be wrong in a way that is invisible in a screenshot and
//! obvious in an assertion, so all of it is testable without a GPU.
//!
//! # The algorithm
//!
//! ```text
//! camera frustum
//!      | split into cascade_count ranges by distance
//!      v
//! frustum slice  ->  8 world-space corners
//!      | bounding sphere (rotation invariant, see below)
//!      v
//! centre + radius
//!      | look from the light through the centre, extruded backwards for casters
//!      v
//! light view  ->  orthographic box of 2*radius
//!      | snap the box origin to whole shadow-map texels
//!      v
//! stable light view-projection
//! ```
//!
//! # Why a sphere and not a box
//!
//! The tight fit for a frustum slice is an oriented box, and it wastes less of the shadow
//! map. It is also the reason shadows crawl: rotate the camera by a degree and the box's
//! extent changes, so every texel in the cascade covers a slightly different patch of world
//! and every shadow edge reshuffles. A bounding **sphere** of the same corners has a radius
//! that depends only on the slice's near and far distances and the projection — not on where
//! the camera is looking — so the cascade's *size* is constant and only its centre moves.
//! Snapping that centre to the texel grid then makes the whole projection stable, which is
//! what §8 asks for. The cost is a shadow map that is about 30% less densely used, and it is
//! the standard trade.

use engine_core::math::{Frustum, Mat4, Sphere, Vec3};
use engine_render_data::{Camera, Projection};

use super::settings::{MAX_CASCADES, ShadowLod, ShadowLodSettings, ShadowSettings};

/// One cascade's depth range along the view direction, in metres from the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeSplit {
    /// Where the cascade starts.
    pub near: f32,
    /// Where it ends. The next cascade starts here.
    pub far: f32,
}

impl CascadeSplit {
    /// How deep the range is.
    #[must_use]
    pub fn depth(&self) -> f32 {
        self.far - self.near
    }
}

/// One cascade, ready to render and ready to sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeView {
    /// The matrix the depth pass renders with and the lighting pass samples with.
    pub view_projection: Mat4,
    /// The slice of the camera's depth range this cascade covers.
    pub split: CascadeSplit,
    /// Centre of the slice's bounding sphere, in world space.
    pub center: Vec3,
    /// Radius of that sphere, in metres. Constant for a given split and projection.
    pub radius: f32,
    /// How much world one shadow-map texel covers, in metres.
    ///
    /// What the normal-offset bias is scaled by: an offset of "one texel" means a different
    /// distance in every cascade, and using the near cascade's number in the far one is what
    /// produces acne at range.
    pub texel_world_size: f32,
    /// How far the light frustum extends behind the sphere, in metres.
    pub extrusion: f32,
}

impl CascadeView {
    /// The frustum to cull this cascade's casters against.
    ///
    /// Built from the same matrix the depth pass renders with, so anything the test keeps is
    /// exactly what the rasterizer could have drawn — including the extruded region behind
    /// the cascade, where a caster is outside the cascade's bounds and still shadows into
    /// it.
    #[must_use]
    pub fn frustum(&self) -> Frustum {
        Frustum::from_view_projection(&self.view_projection)
    }

    /// The sphere this cascade covers.
    #[must_use]
    pub fn sphere(&self) -> Sphere {
        Sphere::new(self.center, self.radius)
    }
}

/// Splits the camera's range into `cascade_count` ranges from the configured distances.
///
/// The first cascade starts at the camera's own near plane rather than at zero: a fragment
/// nearer than the near plane was never rasterized, and starting the cascade there would
/// spend texels on space the camera cannot see.
///
/// Ranges are clamped to the camera's far plane, so a shadow distance configured past the
/// view distance costs nothing rather than rendering geometry that is never shaded.
#[must_use]
pub fn split_cascades(settings: &ShadowSettings, camera: &Camera) -> Vec<CascadeSplit> {
    let (near, far) = camera.projection().depth_range();
    let count = settings.cascade_count.clamp(1, MAX_CASCADES);
    let mut splits = Vec::with_capacity(count);
    let mut start = near;
    for index in 0..count {
        let end = settings.cascade_distances[index].min(far);
        // A cascade whose far plane is already behind its near plane contributes nothing;
        // it is kept as a degenerate sliver rather than dropped so the cascade *count* the
        // shader is told about always matches the number of matrices it is handed.
        let end = end.max(start + 1e-3);
        splits.push(CascadeSplit {
            near: start,
            far: end,
        });
        start = end;
    }
    splits
}

/// The eight world-space corners of the camera's frustum between `near` and `far`.
///
/// Built from the camera's basis and its field of view rather than by inverting the
/// view-projection matrix: the inverse of a projection matrix is numerically poor near the
/// far plane, and this needs no inverse at all.
#[must_use]
pub fn frustum_slice_corners(camera: &Camera, near: f32, far: f32) -> [Vec3; 8] {
    let position = camera.position();
    let forward = camera.forward();
    let right = camera.right();
    let up = camera.up();

    let mut corners = [Vec3::ZERO; 8];
    for (index, distance) in [near, far].into_iter().enumerate() {
        let (half_width, half_height) = match camera.projection() {
            Projection::Perspective { fov_y, aspect, .. } => {
                let half_height = (fov_y * 0.5).tan() * distance;
                (half_height * aspect, half_height)
            }
            Projection::Orthographic {
                half_height,
                aspect,
                ..
            } => (half_height * aspect, half_height),
        };
        let center = position + forward * distance;
        let base = index * 4;
        corners[base] = center - right * half_width - up * half_height;
        corners[base + 1] = center + right * half_width - up * half_height;
        corners[base + 2] = center - right * half_width + up * half_height;
        corners[base + 3] = center + right * half_width + up * half_height;
    }
    corners
}

/// The bounding sphere of a frustum slice's corners.
///
/// The centroid and the largest distance from it. For a symmetric frustum slice the centroid
/// lies on the view axis and every corner distance depends only on the slice's near and far
/// distances and the projection — which is what makes the radius independent of where the
/// camera is pointing, and therefore what makes the cascade stable.
#[must_use]
pub fn slice_bounding_sphere(corners: &[Vec3; 8]) -> Sphere {
    let mut center = Vec3::ZERO;
    for corner in corners {
        center += *corner;
    }
    center *= 1.0 / 8.0;

    let mut radius_squared = 0.0_f32;
    for corner in corners {
        radius_squared = radius_squared.max((*corner - center).length_squared());
    }
    Sphere::new(center, radius_squared.sqrt().max(1e-3))
}

/// An up vector that is not parallel to `direction`.
///
/// A sun straight overhead makes the world up vector useless as a reference and produces a
/// view matrix full of `NaN`. The fallback is +Z, which cannot also be parallel to a
/// direction that is parallel to +Y.
#[must_use]
fn stable_up(direction: Vec3) -> Vec3 {
    if direction.normalize().dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    }
}

/// Builds one cascade's light view-projection.
///
/// `light_direction` is the direction the light *travels*, the same convention
/// [`Lighting`](engine_render_data::Lighting) uses.
///
/// The projection is snapped to the shadow map's own texel grid when
/// [`ShadowSettings::stabilize`] is set: the sphere's centre is expressed in light space,
/// its x and y are floored to whole texels, and the orthographic box is built around the
/// snapped position. Because the box's *size* never changes, snapping its origin means every
/// texel keeps covering the same patch of world as the camera moves, and shadow edges stop
/// crawling.
#[must_use]
pub fn build_cascade(
    split: CascadeSplit,
    corners: &[Vec3; 8],
    light_direction: Vec3,
    settings: &ShadowSettings,
) -> CascadeView {
    let sphere = slice_bounding_sphere(corners);
    let direction = light_direction.normalize();
    let up = stable_up(direction);
    let radius = sphere.radius;
    let extrusion = settings.caster_extrusion.max(0.0);

    // **The light view's eye is the world origin, not the cascade's centre.** That is the
    // whole trick, and getting it wrong is the classic way to write a stabilization that
    // does nothing: if the eye follows the cascade, light space itself slides with the
    // camera and snapping a box inside a sliding space still resamples the world every
    // frame. With the eye pinned, light space is a fixed rotation of world space, its texel
    // grid is anchored in the world, and snapping the box to that grid is what actually
    // holds the shadows still.
    let view = Mat4::look_at(Vec3::ZERO, direction, up);

    let diameter = radius * 2.0;
    let texel = diameter / settings.resolution.max(1) as f32;
    let center_light_space = view.transform_point(sphere.center);
    let snap = |value: f32| {
        if settings.stabilize {
            (value / texel).floor() * texel
        } else {
            value
        }
    };
    let (offset_x, offset_y, offset_z) = (
        snap(center_light_space.x),
        snap(center_light_space.y),
        snap(center_light_space.z),
    );

    // The view looks down -Z, so a cascade in front of the origin has a negative z and its
    // distance is the negation. The near plane is pulled back by the extrusion, which is
    // where a caster standing between the sun and the cascade lives; clipping that away is
    // exactly the bug the extrusion exists to prevent.
    let center_distance = -offset_z;
    let projection = Mat4::orthographic(
        offset_x - radius,
        offset_x + radius,
        offset_y - radius,
        offset_y + radius,
        center_distance - radius - extrusion,
        center_distance + radius,
    );

    CascadeView {
        view_projection: projection * view,
        split,
        center: sphere.center,
        radius,
        texel_world_size: texel,
        extrusion,
    }
}

/// Builds every cascade for one frame.
///
/// Reuses `out`'s allocation: this runs every frame and must not allocate once the scene has
/// settled, exactly like the render list.
pub fn build_cascades(
    settings: &ShadowSettings,
    camera: &Camera,
    light_direction: Vec3,
    out: &mut Vec<CascadeView>,
) {
    out.clear();
    if !settings.enabled {
        return;
    }
    for split in split_cascades(settings, camera) {
        let corners = frustum_slice_corners(camera, split.near, split.far);
        out.push(build_cascade(split, &corners, light_direction, settings));
    }
}

/// Which cascade covers a point `view_depth` metres in front of the camera.
///
/// Returns `None` past the last cascade, which is the lighting shader's "not shadowed at
/// all" case rather than an error: shadows have a range, and beyond it the sun is unoccluded.
#[must_use]
pub fn select_cascade(view_depth: f32, splits: &[CascadeSplit]) -> Option<usize> {
    splits
        .iter()
        .position(|split| view_depth < split.far)
        .or_else(|| {
            // Inside the first cascade's near plane — nearer than the camera's near plane,
            // which a shaded fragment cannot be, but a caller computing depth another way
            // can produce. It belongs to cascade 0.
            (!splits.is_empty() && view_depth < splits[0].near).then_some(0)
        })
}

/// Which two cascades a point falls between, and how far it has crossed.
///
/// ```text
/// |<---------------- cascade i ---------------->|<--- cascade i+1
/// |                                  |<-blend ->|
/// t = 0 .............................0 ........ 1
/// ```
///
/// `t` is 0 everywhere except in the blend region at the end of a cascade, where it rises to
/// 1 at the boundary. The lighting shader samples both cascades there and mixes by `t`,
/// which is what removes the hard line §14 describes. Outside every cascade the result is
/// `None` and the fragment is lit unshadowed.
#[must_use]
pub fn cascade_blend(
    view_depth: f32,
    splits: &[CascadeSplit],
    blend_fraction: f32,
) -> Option<(usize, usize, f32)> {
    let index = select_cascade(view_depth, splits)?;
    let split = splits[index];
    let last = index + 1 >= splits.len();
    let width = split.depth() * blend_fraction.clamp(0.0, 0.5);
    if last || width <= 0.0 {
        return Some((index, index, 0.0));
    }
    let blend_start = split.far - width;
    if view_depth <= blend_start {
        return Some((index, index, 0.0));
    }
    let t = ((view_depth - blend_start) / width).clamp(0.0, 1.0);
    Some((index, index + 1, t))
}

/// Which geometry a caster contributes to a cascade.
///
/// Two rules, in order:
///
/// 1. **Size.** A caster whose shadow would cover fewer than `cull_texels` texels of this
///    cascade is dropped entirely — [`ShadowLod::None`], the fourth level. That is what
///    keeps a field of pebbles from costing a thousand draw calls to change nothing.
/// 2. **Cascade.** Each cascade beyond the first steps the level down by
///    [`ShadowLodSettings::levels_per_cascade`], because a cascade covering three times the
///    distance has texels three times as wide and cannot resolve the detail anyway.
///
/// The level is a *request*: what it resolves to is whatever coarser mesh the entity
/// actually offers, which is [`ShadowCasting::mesh_for_level`](engine_render_data::ShadowCasting::mesh_for_level).
#[must_use]
pub fn shadow_lod_for(
    caster_radius: f32,
    cascade: &CascadeView,
    lod: &ShadowLodSettings,
    cascade_index: usize,
) -> ShadowLod {
    let texels = (caster_radius * 2.0) / cascade.texel_world_size.max(1e-6);
    if texels < lod.cull_texels {
        return ShadowLod::None;
    }
    if !lod.enabled {
        return ShadowLod::Full;
    }
    ShadowLod::Full.coarser(cascade_index as u32 * lod.levels_per_cascade)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::settings::ShadowPreset;
    use engine_core::math::Quat;

    fn camera_at(position: Vec3, yaw: f32) -> Camera {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid aspect");
        camera.set_position(position);
        camera.set_rotation(Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0));
        camera
    }

    #[test]
    fn the_cascades_tile_the_configured_range_without_gaps() {
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let splits = split_cascades(&settings, &camera);

        assert_eq!(splits.len(), 3);
        assert_eq!(splits[0].near, engine_render_data::DEFAULT_NEAR);
        for pair in splits.windows(2) {
            assert_eq!(
                pair[0].far, pair[1].near,
                "a gap between cascades is a band of the world with no shadows in it"
            );
        }
        assert_eq!(splits[2].far, settings.max_distance());
    }

    #[test]
    fn a_shadow_distance_past_the_camera_is_clamped_to_what_is_drawn() {
        let mut settings = ShadowPreset::High.settings();
        settings.cascade_distances = [15.0, 50.0, 5_000.0, 6_000.0];
        let camera = camera_at(Vec3::ZERO, 0.0);
        let (_, far) = camera.projection().depth_range();

        let splits = split_cascades(&settings, &camera);
        assert_eq!(splits.last().expect("three cascades").far, far);
    }

    #[test]
    fn a_frustum_slice_is_symmetric_about_the_view_axis() {
        let camera = camera_at(Vec3::new(3.0, 2.0, -4.0), 0.7);
        let corners = frustum_slice_corners(&camera, 10.0, 40.0);
        let sphere = slice_bounding_sphere(&corners);

        // The centroid must sit on the view axis: the offset from the camera is a pure
        // multiple of the forward vector. If it is not, the slice was built with a wrong
        // basis and every cascade after it is off-centre.
        let offset = sphere.center - camera.position();
        let along = camera.forward() * offset.dot(camera.forward());
        assert!(
            (offset - along).length() < 1e-3,
            "the slice centre is off the view axis by {}",
            (offset - along).length()
        );
    }

    #[test]
    fn the_cascade_radius_does_not_change_when_the_camera_turns() {
        // This is the property the whole stabilization rests on. If the radius moves with
        // the camera's orientation, no amount of snapping keeps the texels still.
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };
        let mut radii = Vec::new();
        for step in 0..24 {
            let yaw = step as f32 * 0.26;
            let camera = camera_at(Vec3::new(0.0, 1.8, 0.0), yaw);
            let corners = frustum_slice_corners(&camera, split.near, split.far);
            radii.push(slice_bounding_sphere(&corners).radius);
        }
        let first = radii[0];
        for radius in &radii {
            assert!(
                (radius - first).abs() < 1e-3,
                "the cascade radius changed with the camera's yaw: {radii:?}"
            );
        }
    }

    #[test]
    fn a_stabilized_cascade_only_moves_in_whole_texels() {
        // Creep the camera forward by a fraction of a texel at a time. The projection's
        // origin must move in steps, not continuously — that is what stops the shadow map
        // resampling the world every frame, which is what shimmering *is*.
        let settings = ShadowPreset::High.settings();
        let light = Vec3::new(-0.4, -1.0, -0.6);
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };

        let mut origins = Vec::new();
        for step in 0..64 {
            let camera = camera_at(Vec3::new(step as f32 * 0.002, 1.8, 0.0), 0.0);
            let corners = frustum_slice_corners(&camera, split.near, split.far);
            let cascade = build_cascade(split, &corners, light, &settings);
            // Where the world origin lands in the cascade's shadow map, in texels.
            let projected = cascade.view_projection.transform_point(Vec3::ZERO);
            origins.push(projected.x * 0.5 * settings.resolution as f32);
        }

        let moved: Vec<f32> = origins.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let still = moved.iter().filter(|delta| delta.abs() < 1e-3).count();
        assert!(
            still > moved.len() / 2,
            "a stabilized cascade moved on {} of {} sub-texel steps",
            moved.len() - still,
            moved.len()
        );
    }

    #[test]
    fn an_unstabilized_cascade_moves_on_every_step() {
        // The control for the test above: with snapping off, the same camera motion moves
        // the projection every single frame. Without this, that test could pass because the
        // camera was not actually moving.
        let settings = ShadowSettings {
            stabilize: false,
            ..ShadowPreset::High.settings()
        };
        let light = Vec3::new(-0.4, -1.0, -0.6);
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };

        let mut origins = Vec::new();
        for step in 0..16 {
            let camera = camera_at(Vec3::new(step as f32 * 0.002, 1.8, 0.0), 0.0);
            let corners = frustum_slice_corners(&camera, split.near, split.far);
            let cascade = build_cascade(split, &corners, light, &settings);
            origins.push(
                cascade.view_projection.transform_point(Vec3::ZERO).x
                    * 0.5
                    * settings.resolution as f32,
            );
        }
        let moved = origins
            .windows(2)
            .filter(|pair| (pair[1] - pair[0]).abs() > 1e-6)
            .count();
        assert_eq!(
            moved,
            origins.len() - 1,
            "without snapping every step should move the projection"
        );
    }

    #[test]
    fn everything_in_the_slice_is_inside_its_own_cascade() {
        // The projection has to actually contain what it was built for, or geometry at the
        // edge of a cascade is missing from its shadow map.
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::new(2.0, 1.8, -3.0), 1.1);
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };
        let corners = frustum_slice_corners(&camera, split.near, split.far);
        let cascade = build_cascade(split, &corners, Vec3::new(-0.4, -1.0, -0.6), &settings);

        for corner in &corners {
            let clip = cascade.view_projection.transform_point(*corner);
            assert!(
                clip.x.abs() <= 1.001 && clip.y.abs() <= 1.001 && (0.0..=1.001).contains(&clip.z),
                "a frustum corner landed outside its cascade at {clip:?}"
            );
        }
    }

    #[test]
    fn a_caster_standing_behind_the_cascade_still_reaches_it() {
        // The extrusion, and the reason for it: a wall between the sun and the cascade is
        // outside the cascade's bounds and must still be rendered into its depth map.
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let light = Vec3::new(0.0, -1.0, 0.0);
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };
        let corners = frustum_slice_corners(&camera, split.near, split.far);
        let cascade = build_cascade(split, &corners, light, &settings);

        // Straight above the cascade centre, well above anything in the slice.
        let overhead = cascade.center + Vec3::new(0.0, 40.0, 0.0);
        let clip = cascade.view_projection.transform_point(overhead);
        assert!(
            (0.0..=1.0).contains(&clip.z),
            "a caster 40 m above the cascade was clipped away at z = {}",
            clip.z
        );
        assert!(cascade.frustum().contains_point(overhead));
    }

    #[test]
    fn a_light_straight_overhead_still_produces_a_usable_matrix() {
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let split = CascadeSplit {
            near: 0.1,
            far: 15.0,
        };
        let corners = frustum_slice_corners(&camera, split.near, split.far);
        let cascade = build_cascade(split, &corners, Vec3::new(0.0, -1.0, 0.0), &settings);
        for value in cascade.view_projection.to_cols_array() {
            assert!(value.is_finite(), "a vertical sun produced a NaN matrix");
        }
    }

    #[test]
    fn cascade_selection_follows_the_splits() {
        let splits = [
            CascadeSplit {
                near: 0.1,
                far: 15.0,
            },
            CascadeSplit {
                near: 15.0,
                far: 50.0,
            },
            CascadeSplit {
                near: 50.0,
                far: 150.0,
            },
        ];
        assert_eq!(select_cascade(1.0, &splits), Some(0));
        assert_eq!(select_cascade(14.99, &splits), Some(0));
        assert_eq!(select_cascade(15.01, &splits), Some(1));
        assert_eq!(select_cascade(149.0, &splits), Some(2));
        assert_eq!(
            select_cascade(151.0, &splits),
            None,
            "past the last cascade"
        );
    }

    #[test]
    fn blending_is_zero_except_at_a_boundary() {
        let splits = [
            CascadeSplit {
                near: 0.1,
                far: 15.0,
            },
            CascadeSplit {
                near: 15.0,
                far: 50.0,
            },
        ];
        let blend = 0.1;

        assert_eq!(cascade_blend(5.0, &splits, blend), Some((0, 0, 0.0)));
        // The last tenth of cascade 0 is 13.5 to 15.
        let Some((near, far, t)) = cascade_blend(14.25, &splits, blend) else {
            panic!("14.25 m is inside the cascades");
        };
        assert_eq!((near, far), (0, 1));
        assert!((t - 0.5).abs() < 0.01, "half way across the blend, got {t}");

        // The last cascade has nothing to blend into and must not try.
        assert_eq!(cascade_blend(49.9, &splits, blend), Some((1, 1, 0.0)));
        assert_eq!(cascade_blend(60.0, &splits, blend), None);
    }

    #[test]
    fn a_zero_blend_fraction_is_a_hard_boundary() {
        let splits = [
            CascadeSplit {
                near: 0.1,
                far: 15.0,
            },
            CascadeSplit {
                near: 15.0,
                far: 50.0,
            },
        ];
        assert_eq!(cascade_blend(14.99, &splits, 0.0), Some((0, 0, 0.0)));
    }

    #[test]
    fn a_distant_cascade_asks_for_coarser_geometry_and_drops_small_casters() {
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let mut cascades = Vec::new();
        build_cascades(
            &settings,
            &camera,
            Vec3::new(-0.4, -1.0, -0.6),
            &mut cascades,
        );

        // A character-sized caster: full detail near, coarser far, never dropped.
        assert_eq!(
            shadow_lod_for(0.5, &cascades[0], &settings.lod, 0),
            ShadowLod::Full
        );
        assert_eq!(
            shadow_lod_for(0.5, &cascades[1], &settings.lod, 1),
            ShadowLod::Reduced
        );
        assert_eq!(
            shadow_lod_for(0.5, &cascades[2], &settings.lod, 2),
            ShadowLod::Simplified
        );

        // A pebble in the last cascade covers a fraction of a texel.
        assert_eq!(
            shadow_lod_for(0.02, &cascades[2], &settings.lod, 2),
            ShadowLod::None
        );
        // The same pebble under the character's feet is worth drawing.
        assert_eq!(
            shadow_lod_for(0.02, &cascades[0], &settings.lod, 0),
            ShadowLod::Full
        );
    }

    #[test]
    fn building_cascades_reuses_its_allocation() {
        // Runs every frame; a Vec that reallocated every frame would be a per-frame
        // allocation in the frame path, which the renderer does not do.
        let settings = ShadowPreset::High.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let mut cascades = Vec::new();
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);
        let capacity = cascades.capacity();
        for _ in 0..8 {
            build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);
        }
        assert_eq!(cascades.capacity(), capacity);
        assert_eq!(cascades.len(), settings.cascade_count);
    }

    #[test]
    fn shadows_switched_off_build_no_cascades() {
        let settings = ShadowPreset::Off.settings();
        let camera = camera_at(Vec3::ZERO, 0.0);
        let mut cascades = vec![];
        build_cascades(&settings, &camera, Vec3::new(0.0, -1.0, 0.0), &mut cascades);
        assert!(cascades.is_empty());
    }
}
