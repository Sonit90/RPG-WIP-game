//! Frustum culling.
//!
//! One flat pass over the world's bounds array, writing the indices that survive into a
//! buffer the caller keeps across frames. No allocation, no branching on entity type, no
//! knowledge of what an entity is.
//!
//! Threading is deliberately absent. `docs/threading.md` measured the job-system break-even
//! at roughly 12 µs of work per job, and culling measures 3.7 ns per object — so a 10,000
//! object scene is 37 µs, or three chunks. Three jobs to save at most 25 µs is not obviously
//! worth the submission overhead, and it is not a guess: both numbers are measured. The loop
//! reads slices, so chunking later is a change at the call site rather than a rewrite.

use engine_core::math::{Frustum, Sphere};

use engine_render_data::RenderWorld;

/// What one cull pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CullStats {
    /// Drawable entities considered.
    pub considered: u32,
    /// Entities that survived and will be drawn.
    pub visible: u32,
    /// Entities rejected by the frustum.
    pub culled: u32,
}

impl CullStats {
    /// Fraction of considered entities that were rejected, 0 to 1.
    ///
    /// Reported because a culler that rejects nothing is pure cost, and one that rejects
    /// everything is a bug that looks like a fast frame.
    #[must_use]
    pub fn cull_ratio(&self) -> f32 {
        if self.considered == 0 {
            0.0
        } else {
            self.culled as f32 / self.considered as f32
        }
    }
}

/// Selects the entities a camera can see.
#[derive(Debug, Default)]
pub struct Culler {
    /// World slot indices that survived, in ascending order.
    visible: Vec<u32>,
    stats: CullStats,
}

impl Culler {
    /// A culler with no reserved capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A culler sized for `capacity` visible entities.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            visible: Vec::with_capacity(capacity),
            stats: CullStats::default(),
        }
    }

    /// Rebuilds the visible set for `frustum`.
    ///
    /// The output buffer is cleared, not reallocated: after the first few frames it has
    /// grown to the scene's high-water mark and never allocates again.
    pub fn cull(&mut self, world: &RenderWorld, frustum: &Frustum) {
        self.visible.clear();
        let mut considered = 0;

        // Two contiguous arrays, read in step. This is why `RenderWorld` is
        // structure-of-arrays: mesh handles and materials are not touched here, and
        // interleaving them would pull them through the cache for nothing.
        let bounds = world.bounds();
        for (slot, visible) in world.visible_flags().iter().enumerate() {
            if !visible {
                continue;
            }
            considered += 1;
            if frustum.intersects_sphere(&bounds[slot]) {
                self.visible.push(slot as u32);
            }
        }

        self.stats = CullStats {
            considered,
            visible: self.visible.len() as u32,
            culled: considered - self.visible.len() as u32,
        };
    }

    /// Marks every drawable entity visible, without testing the frustum.
    ///
    /// Exists so the benchmark can measure the same frame with culling on and off. Without
    /// it, "does culling help?" would compare two different code paths rather than one
    /// code path with one step removed.
    pub fn accept_all(&mut self, world: &RenderWorld) {
        self.visible.clear();
        for (slot, visible) in world.visible_flags().iter().enumerate() {
            if *visible {
                self.visible.push(slot as u32);
            }
        }
        self.stats = CullStats {
            considered: self.visible.len() as u32,
            visible: self.visible.len() as u32,
            culled: 0,
        };
    }

    /// The surviving world slots.
    #[must_use]
    pub fn visible(&self) -> &[u32] {
        &self.visible
    }

    /// What the last pass did.
    #[must_use]
    pub fn stats(&self) -> CullStats {
        self.stats
    }
}

/// Tests one sphere, for callers that have a single bound rather than a world.
#[must_use]
pub fn is_visible(frustum: &Frustum, bounds: &Sphere) -> bool {
    frustum.intersects_sphere(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{Mat4, Vec3};
    use engine_core::transform::Transform;
    use engine_render_data::{MaterialHandle, MeshHandle, RenderEntityDesc};

    fn frustum() -> Frustum {
        let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
        let view = Mat4::look_at(Vec3::ZERO, -Vec3::Z, Vec3::Y);
        Frustum::from_view_projection(&(projection * view))
    }

    fn entity(position: Vec3) -> RenderEntityDesc {
        RenderEntityDesc {
            transform: Transform::from_translation(position),
            mesh: MeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            visible: true,
        }
    }

    #[test]
    fn only_what_is_in_front_of_the_camera_survives() {
        let mut world = RenderWorld::new();
        world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        world.spawn(entity(Vec3::new(0.0, 0.0, 50.0)));
        world.spawn(entity(Vec3::new(500.0, 0.0, -10.0)));

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());

        assert_eq!(culler.visible(), &[0]);
        assert_eq!(culler.stats().considered, 3);
        assert_eq!(culler.stats().culled, 2);
    }

    #[test]
    fn a_hidden_entity_is_not_even_considered() {
        // Hidden is not the same as culled: it never reaches the frustum test, and counting
        // it as culled would flatter the cull ratio.
        let mut world = RenderWorld::new();
        let hidden = world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        world.set_visible(hidden, false);
        world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());

        assert_eq!(culler.stats().considered, 1);
        assert_eq!(culler.visible(), &[1]);
    }

    #[test]
    fn moving_an_entity_changes_whether_it_survives() {
        let mut world = RenderWorld::new();
        let local = Sphere::new(Vec3::ZERO, 1.0);
        let entity_handle = world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());
        assert_eq!(culler.stats().visible, 1);

        world.set_transform(
            entity_handle,
            Transform::from_translation(Vec3::new(0.0, 0.0, 500.0)),
            local,
        );
        culler.cull(&world, &frustum());
        assert_eq!(culler.stats().visible, 0);
    }

    #[test]
    fn a_large_object_just_off_screen_is_kept() {
        // Its centre is outside and its surface is inside. Culling it would be a visible
        // error; keeping it is only work.
        let mut world = RenderWorld::new();
        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(Vec3::new(0.0, 0.0, 20.0)),
            bounds: Sphere::new(Vec3::ZERO, 40.0),
            ..entity(Vec3::ZERO)
        });

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());
        assert_eq!(culler.stats().visible, 1);
    }

    #[test]
    fn scale_grows_the_bound_and_keeps_the_object() {
        let mut world = RenderWorld::new();
        // Far off to the side, but scaled up enough that its bound reaches the frustum.
        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(Vec3::new(30.0, 0.0, -10.0))
                .with_scale(Vec3::splat(25.0)),
            ..entity(Vec3::ZERO)
        });

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());
        assert_eq!(
            culler.stats().visible,
            1,
            "the scaled bound reaches the view"
        );
    }

    #[test]
    fn accepting_everything_matches_the_drawable_set() {
        let mut world = RenderWorld::new();
        world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        world.spawn(entity(Vec3::new(0.0, 0.0, 500.0)));

        let mut culler = Culler::new();
        culler.accept_all(&world);
        assert_eq!(culler.visible(), &[0, 1]);
        assert_eq!(culler.stats().culled, 0);
    }

    #[test]
    fn the_cull_ratio_reports_what_the_pass_achieved() {
        let mut world = RenderWorld::new();
        for index in 0..4 {
            let z = if index == 0 { -10.0 } else { 500.0 };
            world.spawn(entity(Vec3::new(0.0, 0.0, z)));
        }

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());
        assert!((culler.stats().cull_ratio() - 0.75).abs() < 1e-6);

        // A culler that rejects nothing is pure cost; the number exists so that is visible.
        culler.accept_all(&world);
        assert_eq!(culler.stats().cull_ratio(), 0.0);
    }

    #[test]
    fn culling_does_not_allocate_after_the_first_pass() {
        let mut world = RenderWorld::new();
        for _ in 0..64 {
            world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        }

        let mut culler = Culler::new();
        culler.cull(&world, &frustum());
        let capacity = culler.visible.capacity();

        for _ in 0..8 {
            culler.cull(&world, &frustum());
        }
        assert_eq!(
            culler.visible.capacity(),
            capacity,
            "the visible buffer must be reused, not regrown every frame"
        );
    }
}
