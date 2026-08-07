//! The Milestone 16 pipeline, end to end on the CPU:
//!
//! ```text
//! RenderWorld  ->  Culler  ->  RenderList
//! ```
//!
//! No device. Everything up to command recording is CPU-only by design, which is exactly
//! what makes it testable here — and is the reason the milestone put the seam where it did.
//!
//! `culling.rs` and `render_list.rs` unit-test their own halves. This tests the join: that
//! the list a cull pass produces holds the right *number* of entries and the right *data*
//! in them, and that the counter and the list can never disagree.

use engine_core::math::{Frustum, Mat4, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{MaterialHandle, MeshHandle, RenderEntityDesc, RenderWorld};
use engine_renderer::{Culler, RenderList};

/// A camera at the origin looking down −Z, 90° vertical, square aspect, far plane at 100.
fn frustum() -> Frustum {
    let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
    let view = Mat4::look_at(Vec3::ZERO, -Vec3::Z, Vec3::Y);
    Frustum::from_view_projection(&(projection * view))
}

/// Ten units in front of the camera: comfortably inside.
const INSIDE: Vec3 = Vec3::new(0.0, 0.0, -10.0);
/// Fifty units behind the camera: rejected by the near plane.
const OUTSIDE: Vec3 = Vec3::new(0.0, 0.0, 50.0);

fn desc(position: Vec3, mesh: u32, material: u32) -> RenderEntityDesc {
    RenderEntityDesc {
        transform: Transform::from_translation(position),
        mesh: MeshHandle::new(mesh, 1),
        material: MaterialHandle::new(material, 1),
        bounds: Sphere::new(Vec3::ZERO, 1.0),
        visible: true,
    }
}

/// Culls `world` and returns the list plus the pass's statistics.
fn cull(world: &RenderWorld, list: &mut RenderList) -> engine_renderer::CullStats {
    let mut culler = Culler::new();
    culler.cull(world, &frustum(), list);
    // The invariant the whole milestone rests on, checked on every single pass this file
    // performs rather than in one test of its own.
    assert_eq!(
        culler.stats().visible as usize,
        list.len(),
        "the cull counter and the render list must be one answer, not two"
    );
    culler.stats()
}

#[test]
fn an_empty_world_produces_an_empty_render_list() {
    let world = RenderWorld::new();
    let mut list = RenderList::new();
    let stats = cull(&world, &mut list);

    assert_eq!(list.len(), 0);
    assert!(list.is_empty());
    assert_eq!(stats.considered, 0);
    assert_eq!(stats.culled, 0);
}

#[test]
fn a_fully_visible_world_produces_one_entry_per_object() {
    let mut world = RenderWorld::new();
    for index in 0..64 {
        world.spawn(desc(
            Vec3::new((index % 8) as f32 - 4.0, 0.0, -20.0),
            index,
            index,
        ));
    }

    let mut list = RenderList::new();
    let stats = cull(&world, &mut list);

    assert_eq!(list.len(), 64);
    assert_eq!(stats.visible, 64);
    assert_eq!(stats.culled, 0);
}

#[test]
fn a_fully_culled_world_produces_an_empty_render_list() {
    let mut world = RenderWorld::new();
    for index in 0..64 {
        world.spawn(desc(OUTSIDE, index, index));
    }

    let mut list = RenderList::new();
    let stats = cull(&world, &mut list);

    assert!(list.is_empty(), "nothing behind the camera may be drawn");
    assert_eq!(stats.considered, 64);
    assert_eq!(stats.culled, 64);
    assert!(list.entry(0).is_none());
}

#[test]
fn mixed_visibility_produces_exactly_the_visible_entries() {
    // Interleaved rather than blocked, so a bug that copies a run of slots rather than the
    // ones that survived would show.
    let mut world = RenderWorld::new();
    let mut expected = Vec::new();
    for index in 0..100_u32 {
        let visible = index % 3 == 0;
        let entity = world.spawn(desc(
            if visible { INSIDE } else { OUTSIDE },
            index,
            index % 7,
        ));
        if visible {
            expected.push(entity);
        }
    }

    let mut list = RenderList::new();
    let stats = cull(&world, &mut list);

    assert_eq!(list.len(), expected.len());
    assert_eq!(stats.considered, 100);
    assert_eq!(stats.visible as usize, expected.len());
    assert_eq!(stats.culled as usize, 100 - expected.len());
    assert_eq!(
        list.entities(),
        expected.as_slice(),
        "the list must hold the survivors, in world order"
    );
}

#[test]
fn an_entry_carries_the_data_its_entity_was_spawned_with() {
    let mut world = RenderWorld::new();
    // One decoy in front so the object under test is not at index zero in either structure.
    world.spawn(desc(INSIDE, 0, 0));
    let transform = Transform::from_translation(Vec3::new(2.0, 3.0, -12.0))
        .with_scale(Vec3::splat(1.5))
        .with_rotation(engine_core::math::Quat::from_axis_angle(Vec3::Y, 0.4));
    let entity = world.spawn(RenderEntityDesc {
        transform,
        mesh: MeshHandle::new(11, 1),
        material: MaterialHandle::new(5, 1),
        bounds: Sphere::new(Vec3::ZERO, 1.0),
        visible: true,
    });

    let mut list = RenderList::new();
    cull(&world, &mut list);

    let Some(read) = list.entry(1) else {
        panic!("both objects are inside the frustum");
    };
    assert_eq!(read.entity, entity, "entity id");
    assert_eq!(read.mesh, MeshHandle::new(11, 1), "mesh handle");
    assert_eq!(read.material, MaterialHandle::new(5, 1), "material handle");
    assert_eq!(
        read.transform, transform,
        "the entry must carry the transform the entity was spawned with"
    );
}

#[test]
fn a_hidden_entity_never_reaches_the_render_list() {
    // Hidden is not culled: it does not reach the frustum test at all, so it must not
    // appear in the list *or* in the considered count.
    let mut world = RenderWorld::new();
    let hidden = world.spawn(desc(INSIDE, 0, 0));
    world.set_visible(hidden, false);
    let shown = world.spawn(desc(INSIDE, 1, 1));

    let mut list = RenderList::new();
    let stats = cull(&world, &mut list);

    assert_eq!(stats.considered, 1);
    assert_eq!(list.entities(), &[shown]);
}

#[test]
fn rebuilding_leaves_no_trace_of_the_previous_frame() {
    // The list is reused across frames, so a stale entry would be last frame's object drawn
    // this frame — the failure this milestone's reuse strategy has to not have.
    let mut world = RenderWorld::new();
    for index in 0..32 {
        world.spawn(desc(INSIDE, index, index));
    }

    let mut list = RenderList::new();
    cull(&world, &mut list);
    assert_eq!(list.len(), 32);

    // Move everything behind the camera and cull again into the same list.
    let local = Sphere::new(Vec3::ZERO, 1.0);
    for slot in 0..world.slot_count() {
        world.set_transform(
            world.entity_at(slot),
            Transform::from_translation(OUTSIDE),
            local,
        );
    }
    cull(&world, &mut list);

    assert!(list.is_empty(), "the previous frame's entries must be gone");
    assert!(list.entry(0).is_none());
    assert!(list.entities().is_empty());
    assert!(list.transforms().is_empty());
    assert!(list.meshes().is_empty());
    assert!(list.materials().is_empty());
}

#[test]
fn repeated_frames_stop_allocating() {
    // The reason the renderer owns one list across frames. If this regresses, every frame
    // allocates four vectors — 240 times a second at the frame rates this engine targets.
    let mut world = RenderWorld::new();
    for index in 0..256 {
        world.spawn(desc(INSIDE, index % 4, index % 4));
    }

    let mut list = RenderList::new();
    let mut culler = Culler::new();
    culler.cull(&world, &frustum(), &mut list);
    let capacity = list.capacity();
    assert!(capacity >= 256);

    for _ in 0..32 {
        culler.cull(&world, &frustum(), &mut list);
    }
    assert_eq!(
        list.capacity(),
        capacity,
        "a steady-state scene must reuse the list, not regrow it"
    );
    assert_eq!(list.len(), 256);
}

#[test]
fn moving_the_camera_changes_what_the_list_holds() {
    // Camera movement and rotation must both re-select. The frustum is rebuilt per frame
    // from the camera, so this is really asserting that nothing caches visibility.
    let mut world = RenderWorld::new();
    world.spawn(desc(Vec3::new(0.0, 0.0, -10.0), 0, 0));
    world.spawn(desc(Vec3::new(0.0, 0.0, 10.0), 1, 1));

    let projection = Mat4::perspective(std::f32::consts::FRAC_PI_2, 1.0, 1.0, 100.0);
    let mut culler = Culler::new();
    let mut list = RenderList::new();

    // Looking down -Z sees the first.
    let forward =
        Frustum::from_view_projection(&(projection * Mat4::look_at(Vec3::ZERO, -Vec3::Z, Vec3::Y)));
    culler.cull(&world, &forward, &mut list);
    assert_eq!(list.entities(), &[world.entity_at(0)]);

    // Turned around, it sees the second and only the second.
    let backward =
        Frustum::from_view_projection(&(projection * Mat4::look_at(Vec3::ZERO, Vec3::Z, Vec3::Y)));
    culler.cull(&world, &backward, &mut list);
    assert_eq!(list.entities(), &[world.entity_at(1)]);

    // Moved far away, it sees neither.
    let distant = Mat4::look_at(
        Vec3::new(0.0, 0.0, 5000.0),
        Vec3::new(0.0, 0.0, 4999.0),
        Vec3::Y,
    );
    culler.cull(
        &world,
        &Frustum::from_view_projection(&(projection * distant)),
        &mut list,
    );
    assert!(list.is_empty());
}

#[test]
fn accepting_everything_bypasses_the_test_but_fills_the_same_list() {
    // The F8 path. It must produce the same *shape* of answer, so the recording loop cannot
    // tell the difference — only a different number of entries.
    let mut world = RenderWorld::new();
    world.spawn(desc(INSIDE, 0, 0));
    world.spawn(desc(OUTSIDE, 1, 1));

    let mut culler = Culler::new();
    let mut list = RenderList::new();

    culler.cull(&world, &frustum(), &mut list);
    assert_eq!(list.len(), 1);

    culler.accept_all(&world, &mut list);
    assert_eq!(list.len(), 2, "nothing is rejected without the test");
    assert_eq!(culler.stats().culled, 0);
    assert_eq!(culler.stats().visible as usize, list.len());
    assert_eq!(
        list.entities(),
        &[world.entity_at(0), world.entity_at(1)],
        "and the data is still gathered correctly"
    );
}

/// Parallel culling has to be the same answer, reached faster — not a different answer.
mod parallel {
    use super::*;
    use engine_core::jobs::JobSystem;

    /// A world large enough that `cull_parallel` actually splits it.
    ///
    /// Below the chunk threshold it falls back to the serial path, which would make every
    /// test below pass without exercising a single worker.
    fn large_world(objects: u32) -> RenderWorld {
        let mut world = RenderWorld::with_capacity(objects as usize);
        for index in 0..objects {
            // A deterministic mix of inside and outside, interleaved rather than blocked so
            // a chunk boundary lands in the middle of a run of survivors.
            let inside = index % 3 != 0;
            world.spawn(desc(
                if inside { INSIDE } else { OUTSIDE },
                index % 7,
                index % 5,
            ));
        }
        world
    }

    fn serial(world: &RenderWorld) -> RenderList {
        let mut list = RenderList::new();
        let mut culler = Culler::new();
        culler.cull(world, &frustum(), &mut list);
        list
    }

    fn parallel(world: &RenderWorld, workers: usize) -> (RenderList, engine_renderer::CullStats) {
        let Ok(jobs) = JobSystem::with_workers(workers) else {
            panic!("{workers} workers must start");
        };
        let mut list = RenderList::new();
        let mut scratch = Vec::new();
        let mut culler = Culler::new();
        let Ok(()) = culler.cull_parallel(world, &frustum(), &jobs, &mut scratch, &mut list) else {
            panic!("no chunk panics");
        };
        (list, culler.stats())
    }

    #[test]
    fn the_parallel_list_is_identical_to_the_serial_one() {
        // Same entries in the same order. Chunk order is world order, so a merge that
        // reordered anything would be a visible rendering change.
        let world = large_world(50_000);
        let expected = serial(&world);

        for workers in [1, 2, 4, 8, 16] {
            let (actual, _) = parallel(&world, workers);
            assert_eq!(
                actual.len(),
                expected.len(),
                "entry count with {workers} workers"
            );
            assert_eq!(
                actual.entities(),
                expected.entities(),
                "entities and their order with {workers} workers"
            );
            assert_eq!(actual.meshes(), expected.meshes(), "meshes, {workers}");
            assert_eq!(
                actual.materials(),
                expected.materials(),
                "materials, {workers}"
            );
            assert_eq!(
                actual.transforms(),
                expected.transforms(),
                "transforms, {workers}"
            );
        }
    }

    #[test]
    fn the_parallel_statistics_match_the_serial_ones() {
        let world = large_world(50_000);
        let mut serial_list = RenderList::new();
        let mut serial_culler = Culler::new();
        serial_culler.cull(&world, &frustum(), &mut serial_list);

        let (_, stats) = parallel(&world, 8);
        assert_eq!(stats, serial_culler.stats());
        assert_eq!(stats.considered, stats.visible + stats.culled);
    }

    #[test]
    fn a_world_below_the_chunk_threshold_still_produces_the_right_list() {
        // Falls back to serial. The answer must be the same either way, and the fallback
        // must not be silently skipped — a small scene is the common case.
        let world = large_world(100);
        let expected = serial(&world);
        let (actual, _) = parallel(&world, 8);
        assert_eq!(actual.entities(), expected.entities());
    }

    #[test]
    fn an_empty_world_is_safe_to_cull_in_parallel() {
        let world = RenderWorld::new();
        let (list, stats) = parallel(&world, 8);
        assert!(list.is_empty());
        assert_eq!(stats.considered, 0);
    }

    #[test]
    fn a_shrinking_scene_does_not_merge_last_frames_chunks() {
        // The scratch chunks are reused across frames. If the merge walked all of them
        // rather than the ones this frame filled, a scene that shrank would keep drawing
        // objects that are gone.
        let Ok(jobs) = JobSystem::with_workers(8) else {
            panic!("eight workers must start");
        };
        let mut list = RenderList::new();
        let mut scratch = Vec::new();
        let mut culler = Culler::new();

        let big = large_world(50_000);
        let Ok(()) = culler.cull_parallel(&big, &frustum(), &jobs, &mut scratch, &mut list) else {
            panic!("no chunk panics");
        };
        let big_len = list.len();
        assert!(big_len > 0);

        let small = large_world(60);
        let Ok(()) = culler.cull_parallel(&small, &frustum(), &jobs, &mut scratch, &mut list)
        else {
            panic!("no chunk panics");
        };
        assert_eq!(list.entities(), serial(&small).entities());
        assert!(list.len() < big_len);
    }

    #[test]
    fn repeated_parallel_frames_stop_allocating() {
        let Ok(jobs) = JobSystem::with_workers(8) else {
            panic!("eight workers must start");
        };
        let world = large_world(50_000);
        let mut list = RenderList::new();
        let mut scratch = Vec::new();
        let mut culler = Culler::new();

        for _ in 0..4 {
            let Ok(()) = culler.cull_parallel(&world, &frustum(), &jobs, &mut scratch, &mut list)
            else {
                panic!("no chunk panics");
            };
        }
        let merged = list.capacity();
        let chunks: Vec<usize> = scratch.iter().map(RenderList::capacity).collect();

        for _ in 0..16 {
            let Ok(()) = culler.cull_parallel(&world, &frustum(), &jobs, &mut scratch, &mut list)
            else {
                panic!("no chunk panics");
            };
        }

        assert_eq!(list.capacity(), merged, "the merged list must be reused");
        assert_eq!(
            scratch.iter().map(RenderList::capacity).collect::<Vec<_>>(),
            chunks,
            "every chunk list must be reused"
        );
    }
}
