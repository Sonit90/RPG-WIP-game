//! Frustum culling.
//!
//! One flat pass over the world's bounds array, appending what survives to a
//! [`RenderList`] the caller keeps across frames. No allocation, no branching on entity
//! type, no knowledge of what an entity is.
//!
//! # The test and the copy are one pass
//!
//! Before Milestone 16 the culler wrote surviving *slot indices* into a `Vec<u32>` of its
//! own, and the recording loop then walked that and reached back into `RenderWorld` for each
//! object. Two passes over the same data, and two representations of the same answer.
//!
//! Now the survivor is gathered into the render list as it is found. The frustum test has
//! just touched that slot's bounds, so the object's other fields are in cache when they are
//! read — which is the cheapest moment there is to copy them.
//!
//! # Threading
//!
//! [`Culler::cull_parallel`] chunks the world across the job system's workers, one
//! [`RenderList`] per chunk, concatenated in chunk order afterwards. Chunk order is world
//! order, so the merged list is identical to the serial one — same entries, same order —
//! which a test asserts, because a parallel cull that quietly reordered the scene would be
//! a rendering change dressed as an optimization.
//!
//! It falls back to the serial path below [`MIN_CHUNK`] slots per chunk.
//! `docs/threading.md` measured the job break-even near 12 µs of work, and a cull-and-gather
//! pass measures 2 to 10 ns per object, so a few thousand objects is the smallest chunk
//! worth handing to a worker. Splitting a 1,000-object scene would cost more in submission
//! than the whole pass takes.
//!
//! The serial path is not a legacy path. It is what runs whenever the scene is small, it is
//! what a zero-worker configuration runs, and it shares [`cull_range`] with the parallel
//! one so the two cannot drift apart.

use engine_core::error::EngineError;
use engine_core::jobs::JobSystem;
use engine_core::math::{Frustum, Sphere};

use engine_render_data::RenderWorld;

use crate::render_list::RenderList;

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

/// Selects the entities a camera can see and gathers them into a [`RenderList`].
///
/// Holds no buffer of its own: the render list *is* the visible set, and keeping a second
/// copy of the same answer was what Milestone 16 removed. What is left is the visibility
/// policy and the statistics describing what it did.
#[derive(Debug, Default)]
pub struct Culler {
    stats: CullStats,
}

impl Culler {
    /// A culler that has run no passes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds `list` from the entities of `world` that `frustum` can see.
    ///
    /// The list is cleared first and keeps its allocation, so after the first few frames
    /// this performs no allocation at all.
    ///
    /// Returns nothing; read [`Culler::stats`] for what the pass did, or `list.len()` for
    /// how many survived — the two agree by construction.
    pub fn cull(&mut self, world: &RenderWorld, frustum: &Frustum, list: &mut RenderList) {
        list.clear();
        // Counted separately from the pass rather than inside it, so the serial and the
        // parallel path can share one loop body: a job returns nothing, and threading a
        // per-chunk counter back would need a second output array for a number one pass
        // over the flags already has.
        let considered = world.visible_flags().iter().filter(|flag| **flag).count() as u32;
        cull_range(world, frustum, 0..world.slot_count(), list);

        self.stats = CullStats {
            considered,
            visible: list.len() as u32,
            culled: considered - list.len() as u32,
        };
    }

    /// Gathers every drawable entity into `list`, without testing the frustum.
    ///
    /// Exists so a benchmark can measure the same frame with one step removed. Without it,
    /// "does culling help?" would compare two different code paths rather than one code
    /// path minus the test.
    pub fn accept_all(&mut self, world: &RenderWorld, list: &mut RenderList) {
        list.clear();

        let transforms = world.transforms();
        let meshes = world.meshes();
        let materials = world.materials();
        let styles = world.styles();
        let shadow = world.shadow_casting();

        for (slot, visible) in world.visible_flags().iter().enumerate() {
            if *visible {
                list.push(
                    world.entity_at(slot),
                    transforms[slot],
                    meshes[slot],
                    materials[slot],
                    styles[slot],
                    shadow[slot].receives,
                );
            }
        }

        self.stats = CullStats {
            considered: list.len() as u32,
            visible: list.len() as u32,
            culled: 0,
        };
    }

    /// What the last pass did.
    #[must_use]
    pub fn stats(&self) -> CullStats {
        self.stats
    }

    /// Culls `world` across `jobs`' workers, merging the result into `list`.
    ///
    /// ```text
    /// RenderWorld
    ///     |  split into chunks of at least MIN_CHUNK slots
    ///     v
    /// chunk 0     chunk 1     chunk 2   ...     one job each
    ///     |           |           |
    /// scratch[0]  scratch[1]  scratch[2]        thread-local render lists
    ///      \          |          /
    ///       `-------> list <----'                concatenated in chunk order
    /// ```
    ///
    /// Chunk order is world order, so the merged list is *identical* to what
    /// [`Culler::cull`] would have produced — same entries, same order. A parallel cull
    /// that reordered the scene would be a rendering change dressed as an optimization.
    ///
    /// `scratch` is the caller's, kept across frames for the same reason `list` is: after
    /// the first few frames neither allocates. It is resized to the chunk count here.
    ///
    /// Falls back to the serial path when the work is too small to be worth splitting —
    /// `docs/threading.md` puts the job break-even near 12 µs, and a cull of a few hundred
    /// objects is nowhere near it.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] if a chunk panicked.
    pub fn cull_parallel(
        &mut self,
        world: &RenderWorld,
        frustum: &Frustum,
        jobs: &JobSystem,
        scratch: &mut Vec<RenderList>,
        list: &mut RenderList,
    ) -> Result<(), EngineError> {
        let slots = world.slot_count();
        let chunk_count = chunk_count_for(slots, jobs.worker_count());
        if chunk_count <= 1 {
            self.cull(world, frustum, list);
            return Ok(());
        }

        let chunk_size = slots.div_ceil(chunk_count);
        if scratch.len() < chunk_count {
            scratch.resize_with(chunk_count, RenderList::new);
        }

        // Only the chunks that will actually be used, so a scene that shrank does not merge
        // last frame's leftovers.
        let active = &mut scratch[..chunk_count];
        jobs.map_chunks(active, |chunk, output| {
            output.clear();
            let start = chunk * chunk_size;
            let end = (start + chunk_size).min(slots);
            cull_range(world, frustum, start..end, output);
        })?;

        list.clear();
        let visible: usize = active.iter().map(RenderList::len).sum();
        list.reserve(visible);
        for chunk in active.iter() {
            list.append(chunk);
        }

        // `considered` is recomputed rather than accumulated across chunks: a job returns
        // nothing, and threading a counter back would need a second output array for a
        // number a single pass over the flags already has.
        let considered = world.visible_flags().iter().filter(|flag| **flag).count() as u32;
        self.stats = CullStats {
            considered,
            visible: visible as u32,
            culled: considered - visible as u32,
        };
        Ok(())
    }
}

/// Smallest chunk worth giving to a worker.
///
/// `docs/threading.md` measured the job break-even near 12 µs of work, and a cull-and-gather
/// pass measures 2 to 10 ns per object depending on how much survives. Four thousand
/// objects is 8 to 40 µs — the low end of worthwhile, and deliberately so: chunks much
/// larger than this leave workers idle at the tail of the pass.
const MIN_CHUNK: usize = 4096;

/// How many chunks to split `slots` into across `workers`.
///
/// Never more than one chunk per worker — extra chunks only add submission overhead once
/// every worker already has one — and never smaller than [`MIN_CHUNK`]. Returns 1 when the
/// work is too small to split, which is the caller's signal to stay serial.
#[must_use]
fn chunk_count_for(slots: usize, workers: usize) -> usize {
    if workers <= 1 || slots < MIN_CHUNK * 2 {
        return 1;
    }
    (slots / MIN_CHUNK).min(workers).max(1)
}

/// Culls one half-open range of world slots into `list`.
///
/// The body both [`Culler::cull`] and each parallel chunk run, so the two cannot drift
/// apart — a serial pass and a one-chunk parallel pass are the same code over the same
/// range.
fn cull_range(
    world: &RenderWorld,
    frustum: &Frustum,
    range: std::ops::Range<usize>,
    list: &mut RenderList,
) {
    let bounds = world.bounds();
    let transforms = world.transforms();
    let meshes = world.meshes();
    let materials = world.materials();
    let styles = world.styles();
    let shadow = world.shadow_casting();
    let flags = world.visible_flags();

    for slot in range {
        if !flags[slot] {
            continue;
        }
        if frustum.intersects_sphere(&bounds[slot]) {
            list.push(
                world.entity_at(slot),
                transforms[slot],
                meshes[slot],
                materials[slot],
                styles[slot],
                shadow[slot].receives,
            );
        }
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
    use engine_render_data::{InstanceStyle, MaterialHandle, MeshHandle, RenderEntityDesc};

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
            style: InstanceStyle::NEUTRAL,
            shadow: engine_render_data::ShadowCasting::CASTS_AND_RECEIVES,
        }
    }

    #[test]
    fn only_what_is_in_front_of_the_camera_survives() {
        let mut world = RenderWorld::new();
        world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        world.spawn(entity(Vec3::new(0.0, 0.0, 50.0)));
        world.spawn(entity(Vec3::new(500.0, 0.0, -10.0)));

        let mut culler = Culler::new();
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);

        assert_eq!(list.len(), 1);
        assert_eq!(list.entities(), &[world.entity_at(0)]);
        assert_eq!(culler.stats().considered, 3);
        assert_eq!(culler.stats().culled, 2);
        assert_eq!(
            culler.stats().visible as usize,
            list.len(),
            "the counter and the list must be the same answer"
        );
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
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);

        assert_eq!(culler.stats().considered, 1);
        assert_eq!(list.entities(), &[world.entity_at(1)]);
    }

    #[test]
    fn moving_an_entity_changes_whether_it_survives() {
        let mut world = RenderWorld::new();
        let local = Sphere::new(Vec3::ZERO, 1.0);
        let entity_handle = world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));

        let mut culler = Culler::new();
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);
        assert_eq!(culler.stats().visible, 1);

        world.set_transform(
            entity_handle,
            Transform::from_translation(Vec3::new(0.0, 0.0, 500.0)),
            local,
        );
        culler.cull(&world, &frustum(), &mut list);
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
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);
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
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);
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
        let mut list = RenderList::new();
        culler.accept_all(&world, &mut list);
        assert_eq!(
            list.entities(),
            &[world.entity_at(0), world.entity_at(1)],
            "accepting everything keeps world order"
        );
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
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);
        assert!((culler.stats().cull_ratio() - 0.75).abs() < 1e-6);

        // A culler that rejects nothing is pure cost; the number exists so that is visible.
        culler.accept_all(&world, &mut list);
        assert_eq!(culler.stats().cull_ratio(), 0.0);
    }

    #[test]
    fn culling_does_not_allocate_after_the_first_pass() {
        let mut world = RenderWorld::new();
        for _ in 0..64 {
            world.spawn(entity(Vec3::new(0.0, 0.0, -10.0)));
        }

        let mut culler = Culler::new();
        let mut list = RenderList::new();
        culler.cull(&world, &frustum(), &mut list);
        let capacity = list.capacity();
        assert!(capacity >= 64);

        for _ in 0..8 {
            culler.cull(&world, &frustum(), &mut list);
        }
        assert_eq!(
            list.capacity(),
            capacity,
            "the render list must be reused, not regrown every frame"
        );
    }
}
