//! The render list: what to draw, in what order.
//!
//! Culling answers *whether* an object is drawn. This answers *when*, and the answer is a
//! single 64-bit key per object that a plain sort orders correctly. Sorting by that key
//! groups pipeline changes first, then materials, then meshes — so batching in Milestone 18
//! is a linear scan over the sorted list rather than a separate system.

use engine_core::math::Vec3;

use crate::scene::{MaterialHandle, MeshHandle, RenderWorld};

/// Bits reserved for each field of the sort key, most significant first.
///
/// ```text
/// [ layer 8 | pipeline 12 | material 20 | mesh 16 | depth 8 ]
/// ```
///
/// The order is the cost order of the state changes it avoids. A pipeline change is the
/// most expensive thing a frame can do, a material change is a root argument, a mesh change
/// is two buffer binds, and depth affects nothing but early-Z efficiency — so depth is what
/// gets sacrificed when two objects agree on everything else.
pub const LAYER_BITS: u32 = 8;
/// Bits for the pipeline index.
pub const PIPELINE_BITS: u32 = 12;
/// Bits for the material index.
pub const MATERIAL_BITS: u32 = 20;
/// Bits for the mesh index.
pub const MESH_BITS: u32 = 16;
/// Bits for the quantized depth.
pub const DEPTH_BITS: u32 = 8;

/// Render layers, coarse enough to be a byte.
///
/// One byte because a layer is a broad phase ordering — background, world, effects, UI —
/// not a per-object property. Nothing needs 256 of them; the field is a byte because the
/// key's other fields want the remaining bits more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct RenderLayer(pub u8);

impl RenderLayer {
    /// Ordinary opaque world geometry.
    pub const WORLD: Self = Self(1);
}

/// One thing to draw, and the key that orders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawItem {
    /// The packed sort key.
    pub key: u64,
    /// The world slot to read mesh, material and transform from.
    pub entity: u32,
}

/// Packs a sort key.
///
/// Every field is masked to its width rather than asserted: an index past the field's range
/// is a scene larger than the key can describe, and silently colliding two materials is a
/// worse failure than drawing them in a slightly wrong order. The masking is documented and
/// the limits are reported by [`SortKeyLimits`].
#[must_use]
pub fn sort_key(
    layer: RenderLayer,
    pipeline: u32,
    material: u32,
    mesh: u32,
    depth: u8,
) -> u64 {
    let mask = |value: u64, bits: u32| value & ((1_u64 << bits) - 1);
    let mut key = u64::from(layer.0);
    key = (key << PIPELINE_BITS) | mask(u64::from(pipeline), PIPELINE_BITS);
    key = (key << MATERIAL_BITS) | mask(u64::from(material), MATERIAL_BITS);
    key = (key << MESH_BITS) | mask(u64::from(mesh), MESH_BITS);
    key = (key << DEPTH_BITS) | u64::from(depth);
    key
}

/// The largest index each key field can hold.
pub struct SortKeyLimits;

impl SortKeyLimits {
    /// Distinct pipelines the key can order.
    pub const PIPELINES: u32 = 1 << PIPELINE_BITS;
    /// Distinct materials the key can order.
    pub const MATERIALS: u32 = 1 << MATERIAL_BITS;
    /// Distinct meshes the key can order.
    pub const MESHES: u32 = 1 << MESH_BITS;
}

/// Quantizes a distance to the byte the key carries.
///
/// Front-to-back within a material run, so the nearest geometry fills the depth buffer
/// first and later fragments fail early-Z. 256 buckets over the view distance is coarse,
/// and coarse is correct: the sort only has to get the *order of magnitude* right for
/// early-Z to work, and finer buckets would spend key bits that materials want more.
#[must_use]
pub fn quantize_depth(distance: f32, far: f32) -> u8 {
    if !(distance > 0.0) || far <= 0.0 {
        return 0;
    }
    let normalized = (distance / far).clamp(0.0, 1.0);
    (normalized * 255.0) as u8
}

/// What building and sorting one list did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RenderListStats {
    /// Items in the list.
    pub items: u32,
    /// Runs of consecutive items sharing a material.
    ///
    /// This is the number of material binds a recorder that skips redundant ones will
    /// issue, and it is the number sorting is supposed to reduce.
    pub material_runs: u32,
    /// Runs of consecutive items sharing both mesh and material — the batches Milestone 18
    /// will collapse into instanced draws.
    pub batch_runs: u32,
}

/// The sorted list of things to draw.
#[derive(Debug, Default)]
pub struct RenderList {
    items: Vec<DrawItem>,
    stats: RenderListStats,
}

impl RenderList {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty list with room for `capacity` items.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::with_capacity(capacity),
            stats: RenderListStats::default(),
        }
    }

    /// Builds the list from a visible set, without sorting it.
    ///
    /// The buffer is cleared rather than reallocated, so after the first few frames this
    /// allocates nothing.
    pub fn build(
        &mut self,
        world: &RenderWorld,
        visible: &[u32],
        camera_position: Vec3,
        far: f32,
    ) {
        self.items.clear();
        self.items.reserve(visible.len());

        for slot in visible {
            let entity = *slot as usize;
            let distance = (world.bounds()[entity].center - camera_position).length();
            self.items.push(DrawItem {
                key: sort_key(
                    RenderLayer::WORLD,
                    0,
                    world.materials()[entity].index() as u32,
                    world.meshes()[entity].index() as u32,
                    quantize_depth(distance, far),
                ),
                entity: *slot,
            });
        }

        self.refresh_stats(world);
    }

    /// Sorts the list by key.
    ///
    /// Unstable sort: the key already contains everything that decides order, so two items
    /// that compare equal are genuinely interchangeable, and a stable sort would pay for a
    /// guarantee nothing needs.
    pub fn sort(&mut self) {
        self.items.sort_unstable_by_key(|item| item.key);
    }

    /// Recomputes the run statistics after the order changes.
    pub fn refresh_stats(&mut self, world: &RenderWorld) {
        let mut material_runs = 0;
        let mut batch_runs = 0;
        let mut previous: Option<(MaterialHandle, MeshHandle)> = None;

        for item in &self.items {
            let entity = item.entity as usize;
            let material = world.materials()[entity];
            let mesh = world.meshes()[entity];
            match previous {
                Some((last_material, last_mesh)) => {
                    if material != last_material {
                        material_runs += 1;
                        batch_runs += 1;
                    } else if mesh != last_mesh {
                        batch_runs += 1;
                    }
                }
                None => {
                    material_runs += 1;
                    batch_runs += 1;
                }
            }
            previous = Some((material, mesh));
        }

        self.stats = RenderListStats {
            items: self.items.len() as u32,
            material_runs,
            batch_runs,
        };
    }

    /// The items, in their current order.
    #[must_use]
    pub fn items(&self) -> &[DrawItem] {
        &self.items
    }

    /// What the last build produced.
    #[must_use]
    pub fn stats(&self) -> RenderListStats {
        self.stats
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Items in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MaterialHandle, MeshHandle, RenderEntityDesc};
    use engine_core::math::Sphere;
    use engine_core::transform::Transform;

    fn world_with(entries: &[(u32, u32, f32)]) -> RenderWorld {
        let mut world = RenderWorld::new();
        for (material, mesh, z) in entries {
            world.spawn(RenderEntityDesc {
                transform: Transform::from_translation(Vec3::new(0.0, 0.0, *z)),
                mesh: MeshHandle::new(*mesh, 1),
                material: MaterialHandle::new(*material, 1),
                bounds: Sphere::new(Vec3::ZERO, 1.0),
                visible: true,
            });
        }
        world
    }

    fn all_slots(world: &RenderWorld) -> Vec<u32> {
        (0..world.slot_count() as u32).collect()
    }

    #[test]
    fn the_key_orders_by_pipeline_then_material_then_mesh() {
        let base = sort_key(RenderLayer::WORLD, 1, 5, 9, 100);
        assert!(base < sort_key(RenderLayer::WORLD, 2, 0, 0, 0), "pipeline wins");
        assert!(base < sort_key(RenderLayer::WORLD, 1, 6, 0, 0), "then material");
        assert!(base < sort_key(RenderLayer::WORLD, 1, 5, 10, 0), "then mesh");
        assert!(base < sort_key(RenderLayer::WORLD, 1, 5, 9, 101), "then depth");
    }

    #[test]
    fn the_layer_outranks_everything_else() {
        let world = sort_key(RenderLayer::WORLD, 4095, 1048575, 65535, 255);
        let later = sort_key(RenderLayer(2), 0, 0, 0, 0);
        assert!(world < later, "a later layer must sort after any world object");
    }

    #[test]
    fn an_index_past_a_field_wraps_rather_than_corrupting_its_neighbours() {
        // Masking is deliberate: colliding two materials orders them oddly, while letting
        // the value overflow into the pipeline field would reorder pipelines.
        let a = sort_key(RenderLayer::WORLD, 0, SortKeyLimits::MATERIALS, 0, 0);
        let b = sort_key(RenderLayer::WORLD, 0, 0, 0, 0);
        assert_eq!(a, b);
    }

    #[test]
    fn depth_quantizes_front_to_back() {
        assert_eq!(quantize_depth(0.0, 100.0), 0);
        assert!(quantize_depth(10.0, 100.0) < quantize_depth(50.0, 100.0));
        assert_eq!(quantize_depth(200.0, 100.0), 255, "clamped, not wrapped");
        assert_eq!(quantize_depth(f32::NAN, 100.0), 0, "NaN must not panic");
    }

    #[test]
    fn sorting_groups_materials_that_arrived_interleaved() {
        // The scene is built worst case: alternating materials, which is what an
        // unsorted spawn order looks like.
        let world = world_with(&[(0, 0, -1.0), (1, 0, -2.0), (0, 0, -3.0), (1, 0, -4.0)]);
        let mut list = RenderList::new();
        list.build(&world, &all_slots(&world), Vec3::ZERO, 100.0);
        assert_eq!(list.stats().material_runs, 4, "interleaved before sorting");

        list.sort();
        list.refresh_stats(&world);
        assert_eq!(list.stats().material_runs, 2, "one run per material after");
    }

    #[test]
    fn sorting_groups_meshes_within_a_material() {
        let world = world_with(&[(0, 0, -1.0), (0, 1, -2.0), (0, 0, -3.0), (0, 1, -4.0)]);
        let mut list = RenderList::new();
        list.build(&world, &all_slots(&world), Vec3::ZERO, 100.0);
        assert_eq!(list.stats().batch_runs, 4);

        list.sort();
        list.refresh_stats(&world);
        assert_eq!(list.stats().batch_runs, 2, "two mesh runs inside one material");
        assert_eq!(list.stats().material_runs, 1);
    }

    #[test]
    fn within_a_batch_the_nearest_object_is_drawn_first() {
        // Front-to-back so early-Z rejects the far fragments. Depth is the least significant
        // field, so this only holds once material and mesh agree — which is the point.
        let world = world_with(&[(0, 0, -50.0), (0, 0, -5.0), (0, 0, -20.0)]);
        let mut list = RenderList::new();
        list.build(&world, &all_slots(&world), Vec3::ZERO, 100.0);
        list.sort();

        let order: Vec<u32> = list.items().iter().map(|item| item.entity).collect();
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn only_the_visible_set_reaches_the_list() {
        let world = world_with(&[(0, 0, -1.0), (1, 0, -2.0), (2, 0, -3.0)]);
        let mut list = RenderList::new();
        list.build(&world, &[2], Vec3::ZERO, 100.0);
        assert_eq!(list.len(), 1);
        assert_eq!(list.items()[0].entity, 2);
    }

    #[test]
    fn building_does_not_allocate_after_the_first_frame() {
        let world = world_with(&vec![(0, 0, -1.0); 64]);
        let visible = all_slots(&world);
        let mut list = RenderList::new();
        list.build(&world, &visible, Vec3::ZERO, 100.0);
        let capacity = list.items.capacity();

        for _ in 0..8 {
            list.build(&world, &visible, Vec3::ZERO, 100.0);
            list.sort();
        }
        assert_eq!(list.items.capacity(), capacity);
    }

    #[test]
    fn an_empty_visible_set_produces_an_empty_list() {
        let world = world_with(&[(0, 0, -1.0)]);
        let mut list = RenderList::new();
        list.build(&world, &[], Vec3::ZERO, 100.0);
        assert!(list.is_empty());
        assert_eq!(list.stats().material_runs, 0);
    }
}
