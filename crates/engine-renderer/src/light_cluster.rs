//! Clustered local-light culling.
//!
//! The mesh pixel shader used to loop over every local light in the frame for every pixel it
//! shaded. Four brazier lights in the arena cost 0.11 ms of a 0.43 ms frame at 4K with
//! shadows off, and — because a lit pixel also takes a shadow-atlas lookup per shadow-casting
//! light in range — noticeably more with them on. The cost is per pixel and per light and it
//! does not care whether the light is anywhere near the surface: a torch in the far corner of
//! the arena is evaluated, rejected on distance and paid for, on every pixel of the screen.
//!
//! This module is the structure that stops that. The view frustum is divided into a fixed
//! grid of **clusters** — tiles across the screen, slices in depth — and each frame every
//! light is assigned to the clusters it can actually reach. The shader then finds its own
//! cluster from the pixel position and the view depth and evaluates only the lights in it.
//!
//! ```text
//!            near ......................................... far
//!   screen  +----+----+----+          depth slices are exponential: a slice
//!    tiles  |    |    |    |          near the camera is thin, one at the far
//!           +----+----+----+          plane is enormous, and both hold a
//!           |    |    |####|  <- one  similar amount of *screen*
//!           +----+----+----+  cluster
//! ```
//!
//! # Why a bitmask, and why on the CPU
//!
//! A cluster's light set is **one `u32`**: bit `i` means light `i` reaches this cluster. That
//! is only possible because [`MAX_LOCAL_LIGHTS`](crate::shadow::MAX_LOCAL_LIGHTS) is 32, and
//! it is what makes the whole grid 13.5 KB — a `ByteAddressBuffer` written into the upload
//! ring each frame and bound as a **root** SRV, with no descriptor to allocate and no
//! compute pass to dispatch. The shader walks the mask with `firstbitlow`, so a cluster with
//! two lights in it costs two iterations whether the frame has two lights or thirty-two.
//!
//! The grid was in the frame *constant* buffer first, which was simpler still and measurably
//! wrong: it took the buffer from 2.4 KB to 17.8 KB, and a shader whose constant buffer no
//! longer fits the hardware's constant cache pays for that on every read of it, not only on
//! the reads of the new part. That cost 0.34 ms at 4K — more than the lighting the grid was
//! there to save. See docs/performance/shadows/01-local-light-culling.md.
//!
//! Assignment is on the CPU because it is *output-sensitive*: a light touches the clusters
//! its bounding sphere covers, and finding them costs a projection and a walk over that box
//! rather than a test against all 3456 clusters. For the light counts this engine supports
//! that is a few microseconds, measured in `light_cluster_bench`.
//!
//! # What it does not do
//!
//! There is no depth pre-culling: a cluster behind a wall still lists the light in front of
//! it. That would need the depth buffer and a compute pass, and it buys nothing until the
//! grid is the bottleneck rather than the lighting.

use engine_core::math::Mat4;
#[cfg(test)]
use engine_core::math::Vec3;
use engine_render_data::{Camera, LocalLight, Projection};

use crate::shadow::MAX_LOCAL_LIGHTS;

/// Cluster tiles across the screen.
pub const CLUSTERS_X: usize = 16;
/// Cluster tiles down the screen.
///
/// Nine rather than eight: the game is 16:9 and square-ish clusters keep a light's screen
/// footprint from spilling into rows it does not touch.
pub const CLUSTERS_Y: usize = 9;
/// Depth slices, exponentially spaced between the near and far planes.
pub const CLUSTERS_Z: usize = 24;

/// Clusters in the grid.
pub const CLUSTER_COUNT: usize = CLUSTERS_X * CLUSTERS_Y * CLUSTERS_Z;

/// Bytes one frame's masks occupy.
pub const CLUSTER_MASK_BYTES: usize = CLUSTER_COUNT * 4;

/// What one frame's assignment did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightClusterStats {
    /// Lights assigned, after the [`MAX_LOCAL_LIGHTS`] cap.
    pub lights: u32,
    /// Lights dropped because the frame had more than [`MAX_LOCAL_LIGHTS`] of them.
    pub dropped: u32,
    /// Total light-to-cluster assignments: the sum of every cluster's population.
    pub assignments: u32,
    /// Clusters with at least one light.
    pub occupied: u32,
    /// The most lights any one cluster holds. This is the shader's worst-case loop count.
    pub max_per_cluster: u32,
}

/// The grid, and this frame's assignment into it.
///
/// Owned by the renderer and reused every frame: the mask array is 13.5 KB and allocating it
/// per frame would cost more than filling it does.
#[derive(Debug, Clone)]
pub struct LightClusters {
    masks: Vec<u32>,
    depth: [f32; 4],
    enabled: bool,
    stats: LightClusterStats,
}

impl Default for LightClusters {
    fn default() -> Self {
        Self::new()
    }
}

impl LightClusters {
    /// An empty grid: no lights anywhere, and clustering switched off until the first build.
    #[must_use]
    pub fn new() -> Self {
        Self {
            masks: vec![0; CLUSTER_COUNT],
            depth: [0.0; 4],
            enabled: false,
            stats: LightClusterStats::default(),
        }
    }

    /// The per-cluster light masks, one `u32` each, in cluster index order.
    #[must_use]
    pub fn masks(&self) -> &[u32] {
        &self.masks
    }

    /// `[grid x, grid y, grid z, 1 when the grid is live]`.
    ///
    /// The last component is what the shader branches on. Zero means "no grid was built for
    /// this frame" and the shader falls back to the light list itself, which is what the
    /// offscreen helpers and the older tests produce.
    #[must_use]
    pub fn params(&self) -> [f32; 4] {
        [
            CLUSTERS_X as f32,
            CLUSTERS_Y as f32,
            CLUSTERS_Z as f32,
            f32::from(u8::from(self.enabled)),
        ]
    }

    /// `[z scale, z bias, near, far]` for the exponential depth slicing.
    ///
    /// `slice = floor(log(depth) * scale + bias)`, which is the standard exponential
    /// distribution: it spends slices where perspective puts the geometry rather than
    /// spreading them evenly over a range whose far half is a handful of pixels.
    #[must_use]
    pub fn depth_params(&self) -> [f32; 4] {
        self.depth
    }

    /// What the last [`LightClusters::build`] produced.
    #[must_use]
    pub fn stats(&self) -> LightClusterStats {
        self.stats
    }

    /// Whether the last build produced a usable grid.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Empties the grid and marks it unusable, so the shader takes the unclustered path.
    pub fn disable(&mut self) {
        self.masks.fill(0);
        self.depth = [0.0; 4];
        self.enabled = false;
        self.stats = LightClusterStats::default();
    }

    /// Assigns `lights` to the clusters of `camera`'s frustum.
    ///
    /// Lights past [`MAX_LOCAL_LIGHTS`] are dropped, and counted in the statistics rather
    /// than silently: a scene that trips the cap should be visible in the overlay.
    ///
    /// An orthographic camera disables the grid rather than being handled: the depth slicing
    /// is a property of a perspective frustum, the only orthographic cameras in the engine
    /// are debug ones, and "every light in every cluster" is the correct fallback anyway.
    pub fn build(&mut self, lights: &[LocalLight], camera: &Camera) {
        self.masks.fill(0);
        self.stats = LightClusterStats::default();

        let Projection::Perspective {
            fov_y,
            aspect,
            near,
            far,
        } = camera.projection()
        else {
            self.disable();
            return;
        };
        if !(near > 0.0 && far > near) {
            self.disable();
            return;
        }

        let count = lights.len().min(MAX_LOCAL_LIGHTS);
        self.stats.lights = count as u32;
        self.stats.dropped = (lights.len() - count) as u32;

        let slices = CLUSTERS_Z as f32;
        let log_ratio = (far / near).ln();
        let z_scale = slices / log_ratio;
        let z_bias = -z_scale * near.ln();
        self.depth = [z_scale, z_bias, near, far];
        self.enabled = true;

        // Half-extents of the frustum one metre in front of the camera. Every view-space
        // bound below is one of these multiplied by a depth.
        let tan_y = (fov_y * 0.5).tan();
        let tan_x = tan_y * aspect;

        let view = camera.view();
        for (index, light) in lights.iter().take(count).enumerate() {
            self.assign(
                index, light, &view, tan_x, tan_y, near, far, z_scale, z_bias,
            );
        }

        let (mut occupied, mut assignments, mut worst) = (0_u32, 0_u32, 0_u32);
        for &mask in &self.masks {
            if mask != 0 {
                occupied += 1;
                assignments += mask.count_ones();
                worst = worst.max(mask.count_ones());
            }
        }
        self.stats.occupied = occupied;
        self.stats.assignments = assignments;
        self.stats.max_per_cluster = worst;
    }

    /// Marks every cluster one light reaches.
    #[allow(
        clippy::too_many_arguments,
        reason = "the frustum's description, threaded from the one caller that has it"
    )]
    fn assign(
        &mut self,
        index: usize,
        light: &LocalLight,
        view: &Mat4,
        tan_x: f32,
        tan_y: f32,
        near: f32,
        far: f32,
        z_scale: f32,
        z_bias: f32,
    ) {
        let radius = light.range;
        if !radius.is_finite() || radius <= 0.0 {
            return;
        }
        let center = view.transform_point(light.position);
        // The view looks down -Z, so a point in front of the camera has a negative z and its
        // distance is the negation. `clip.w` in the shader is this same quantity.
        let depth = -center.z;
        let (near_depth, far_depth) = (depth - radius, depth + radius);
        if far_depth <= near {
            return;
        }
        if near_depth >= far {
            return;
        }

        let low = near_depth.max(near);
        let high = far_depth.min(far);
        let first_slice = slice_of(low, z_scale, z_bias);
        let last_slice = slice_of(high, z_scale, z_bias);

        // The tile range the sphere can possibly touch. Conservative on purpose: the exact
        // sphere-against-cluster test below is what removes what this over-selects, and this
        // only has to be a superset that is much smaller than the whole grid.
        let (tile_x0, tile_x1, tile_y0, tile_y1) = if near_depth <= near {
            // The camera is inside the light. Its footprint is the whole screen.
            (0, CLUSTERS_X - 1, 0, CLUSTERS_Y - 1)
        } else {
            let (x0, x1) = ndc_span(center.x, radius, low, high, tan_x);
            let (y0, y1) = ndc_span(center.y, radius, low, high, tan_y);
            (
                tile_of(x0, CLUSTERS_X),
                tile_of(x1, CLUSTERS_X),
                tile_of(-y1, CLUSTERS_Y),
                tile_of(-y0, CLUSTERS_Y),
            )
        };

        let bit = 1_u32 << index;
        for slice in first_slice..=last_slice {
            let (slice_near, slice_far) = slice_depths(slice, near, far);
            for tile_y in tile_y0..=tile_y1 {
                // Screen rows run down and view-space Y runs up, so row 0 is the *top* of
                // the frustum. Getting this the wrong way round lights the mirror image of
                // the scene, which is a bug that looks like nothing at all in a symmetric
                // test scene.
                let ndc_top = 1.0 - 2.0 * (tile_y as f32) / CLUSTERS_Y as f32;
                let ndc_bottom = 1.0 - 2.0 * ((tile_y + 1) as f32) / CLUSTERS_Y as f32;
                for tile_x in tile_x0..=tile_x1 {
                    let ndc_left = 2.0 * (tile_x as f32) / CLUSTERS_X as f32 - 1.0;
                    let ndc_right = 2.0 * ((tile_x + 1) as f32) / CLUSTERS_X as f32 - 1.0;

                    let x_low = (ndc_left * slice_near * tan_x).min(ndc_left * slice_far * tan_x);
                    let x_high =
                        (ndc_right * slice_near * tan_x).max(ndc_right * slice_far * tan_x);
                    let y_low =
                        (ndc_bottom * slice_near * tan_y).min(ndc_bottom * slice_far * tan_y);
                    let y_high = (ndc_top * slice_near * tan_y).max(ndc_top * slice_far * tan_y);

                    let closest_x = center.x.clamp(x_low, x_high);
                    let closest_y = center.y.clamp(y_low, y_high);
                    let closest_z = depth.clamp(slice_near, slice_far);
                    let dx = center.x - closest_x;
                    let dy = center.y - closest_y;
                    let dz = depth - closest_z;
                    if dx * dx + dy * dy + dz * dz > radius * radius {
                        continue;
                    }

                    let cluster = tile_x + tile_y * CLUSTERS_X + slice * CLUSTERS_X * CLUSTERS_Y;
                    self.masks[cluster] |= bit;
                }
            }
        }
    }
}

/// Which depth slice a distance falls in.
fn slice_of(depth: f32, z_scale: f32, z_bias: f32) -> usize {
    let raw = depth.max(1e-4).ln().mul_add(z_scale, z_bias);
    (raw.floor().max(0.0) as usize).min(CLUSTERS_Z - 1)
}

/// The near and far distances of one depth slice.
fn slice_depths(slice: usize, near: f32, far: f32) -> (f32, f32) {
    let ratio = far / near;
    let step = |index: usize| near * ratio.powf(index as f32 / CLUSTERS_Z as f32);
    (step(slice), step(slice + 1))
}

/// The NDC span a sphere's extent covers between two depths, on one axis.
///
/// Conservative: the extreme of `(centre ± radius) / (depth * tan)` over the depth range,
/// which is a superset of the true projected extent and never a subset.
fn ndc_span(center: f32, radius: f32, low: f32, high: f32, tan: f32) -> (f32, f32) {
    let scale_low = 1.0 / (low * tan);
    let scale_high = 1.0 / (high * tan);
    let minimum = ((center - radius) * scale_low).min((center - radius) * scale_high);
    let maximum = ((center + radius) * scale_low).max((center + radius) * scale_high);
    (minimum, maximum)
}

/// Which tile an NDC coordinate falls in, clamped to the grid.
fn tile_of(ndc: f32, tiles: usize) -> usize {
    let normalized = (ndc * 0.5 + 0.5) * tiles as f32;
    (normalized.floor().max(0.0) as usize).min(tiles - 1)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use engine_render_data::LocalLightKind;

    use super::*;

    /// A camera at the origin looking down -Z, which is the identity view matrix.
    fn camera() -> Camera {
        Camera::new(Projection::Perspective {
            fov_y: std::f32::consts::FRAC_PI_3,
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 1000.0,
        })
        .expect("a valid perspective projection")
    }

    fn point_light(position: Vec3, range: f32) -> LocalLight {
        LocalLight {
            kind: LocalLightKind::Point,
            position,
            range,
            ..LocalLight::default()
        }
    }

    fn mask_at(clusters: &LightClusters, cluster: usize) -> u32 {
        clusters.masks()[cluster]
    }

    fn cluster_index(x: usize, y: usize, z: usize) -> usize {
        x + y * CLUSTERS_X + z * CLUSTERS_X * CLUSTERS_Y
    }

    #[test]
    fn no_lights_means_no_cluster_holds_one() {
        let mut clusters = LightClusters::new();
        clusters.build(&[], &camera());
        assert!(clusters.enabled(), "an empty frame still has a usable grid");
        assert_eq!(clusters.stats().assignments, 0);
        assert_eq!(clusters.stats().occupied, 0);
    }

    #[test]
    fn a_light_reaches_some_clusters_and_not_most_of_them() {
        // The whole point of the structure: a light in one corner of the view must not be
        // listed in the cluster on the opposite side of the screen.
        let mut clusters = LightClusters::new();
        clusters.build(&[point_light(Vec3::new(0.0, 0.0, -20.0), 4.0)], &camera());
        let stats = clusters.stats();
        assert!(stats.occupied > 0, "the light reaches nothing at all");
        assert!(
            (stats.occupied as usize) < CLUSTER_COUNT / 4,
            "a 4 m light occupies {} of {CLUSTER_COUNT} clusters, which is not culling",
            stats.occupied
        );
        assert_eq!(stats.max_per_cluster, 1);
    }

    #[test]
    fn a_light_is_listed_in_the_cluster_its_own_centre_falls_in() {
        // The lookup the shader does, done here on the CPU: the cluster containing the
        // light's own position must list it, or the light is invisible where it is
        // brightest. This is the assertion that catches an inverted row or a slice off by
        // one, both of which leave the general shape of the grid looking correct.
        let mut clusters = LightClusters::new();
        let position = Vec3::new(1.5, -0.5, -12.0);
        clusters.build(&[point_light(position, 6.0)], &camera());

        let depth = 12.0_f32;
        let [scale, bias, ..] = clusters.depth_params();
        let slice = slice_of(depth, scale, bias);
        let tan_y = (std::f32::consts::FRAC_PI_3 * 0.5).tan();
        let tan_x = tan_y * (16.0 / 9.0);
        let ndc_x = position.x / (depth * tan_x);
        let ndc_y = position.y / (depth * tan_y);
        let tile_x = tile_of(ndc_x, CLUSTERS_X);
        let tile_y = tile_of(-ndc_y, CLUSTERS_Y);

        let mask = mask_at(&clusters, cluster_index(tile_x, tile_y, slice));
        assert_eq!(mask & 1, 1, "the light's own cluster does not list it");
    }

    #[test]
    fn a_light_behind_the_camera_reaches_nothing() {
        let mut clusters = LightClusters::new();
        clusters.build(&[point_light(Vec3::new(0.0, 0.0, 40.0), 5.0)], &camera());
        assert_eq!(clusters.stats().assignments, 0);
    }

    #[test]
    fn a_light_past_the_far_plane_reaches_nothing() {
        let mut clusters = LightClusters::new();
        clusters.build(&[point_light(Vec3::new(0.0, 0.0, -2000.0), 5.0)], &camera());
        assert_eq!(clusters.stats().assignments, 0);
    }

    #[test]
    fn a_light_the_camera_stands_inside_covers_every_tile() {
        // The near-plane case, which the conservative projection cannot handle and which is
        // why there is a branch for it: a sphere straddling the eye projects to infinity.
        let mut clusters = LightClusters::new();
        clusters.build(&[point_light(Vec3::ZERO, 30.0)], &camera());
        for tile_y in 0..CLUSTERS_Y {
            for tile_x in 0..CLUSTERS_X {
                let mask = mask_at(&clusters, cluster_index(tile_x, tile_y, 0));
                assert_eq!(
                    mask & 1,
                    1,
                    "tile {tile_x},{tile_y} of the near slice is dark"
                );
            }
        }
    }

    #[test]
    fn two_lights_that_do_not_overlap_never_share_a_cluster() {
        let mut clusters = LightClusters::new();
        clusters.build(
            &[
                point_light(Vec3::new(-30.0, 0.0, -40.0), 5.0),
                point_light(Vec3::new(30.0, 0.0, -40.0), 5.0),
            ],
            &camera(),
        );
        assert_eq!(clusters.stats().max_per_cluster, 1);
        assert!(clusters.stats().occupied > 0);
    }

    #[test]
    fn lights_past_the_cap_are_dropped_and_counted() {
        let lights: Vec<LocalLight> = (0..MAX_LOCAL_LIGHTS + 3)
            .map(|index| point_light(Vec3::new(index as f32, 0.0, -20.0), 2.0))
            .collect();
        let mut clusters = LightClusters::new();
        clusters.build(&lights, &camera());
        assert_eq!(clusters.stats().lights, MAX_LOCAL_LIGHTS as u32);
        assert_eq!(clusters.stats().dropped, 3);
    }

    #[test]
    fn an_orthographic_camera_disables_the_grid_rather_than_guessing() {
        let camera = Camera::new(Projection::Orthographic {
            half_height: 10.0,
            aspect: 1.0,
            near: 0.1,
            far: 100.0,
        })
        .expect("a valid orthographic projection");
        let mut clusters = LightClusters::new();
        clusters.build(&[point_light(Vec3::new(0.0, 0.0, -5.0), 5.0)], &camera);
        assert!(!clusters.enabled());
        assert_eq!(clusters.params()[3], 0.0);
    }

    #[test]
    fn the_depth_slices_cover_the_whole_range_without_a_gap() {
        let mut clusters = LightClusters::new();
        clusters.build(&[], &camera());
        let [scale, bias, near, far] = clusters.depth_params();
        assert_eq!(slice_of(near, scale, bias), 0);
        assert_eq!(slice_of(far, scale, bias), CLUSTERS_Z - 1);
        for slice in 0..CLUSTERS_Z {
            let (low, high) = slice_depths(slice, near, far);
            assert!(high > low, "slice {slice} is empty");
            let middle = (low + high) * 0.5;
            assert_eq!(
                slice_of(middle, scale, bias),
                slice,
                "the middle of slice {slice} does not resolve to it"
            );
        }
    }

    #[test]
    fn the_mask_array_covers_every_cluster() {
        let clusters = LightClusters::new();
        assert_eq!(clusters.masks().len(), CLUSTER_COUNT);
        assert_eq!(CLUSTER_MASK_BYTES, CLUSTER_COUNT * 4);
        const { assert!(MAX_LOCAL_LIGHTS <= 32, "a cluster's light set is one u32") };
    }
}
